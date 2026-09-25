//! Studio application state: the open project, its derived diagnostics, and
//! the actions (new/open/save/build/doctor/analyze) that mutate it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use gpui_kit::base::ResizableState;
use gpui_kit::component::input::InputState;
use gpui_kit::*;

use inst_catalog::Catalog;
use inst_i18n::Language;
use inst_model::platform::Target;
use inst_model::project::Project;
use inst_model::validate::{self, Diagnostic};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Product,
    Install,
    Ui,
    Policy,
    Targets,
    Compression,
    Signing,
    Update,
}

impl Section {
    pub const ALL: &'static [Section] = &[
        Section::Product,
        Section::Install,
        Section::Ui,
        Section::Policy,
        Section::Targets,
        Section::Compression,
        Section::Signing,
        Section::Update,
    ];

    /// Sections shown in Simple mode; Advanced mode shows all of them.
    pub const SIMPLE: &'static [Section] = &[
        Section::Product,
        Section::Install,
        Section::Ui,
        Section::Policy,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BottomTab {
    Output,
    Problems,
    Build,
    Doctor,
}

/// Live text-editing widgets for the fields the property editor binds to.
/// Values are pulled from these into the project on every render, so there
/// is no per-keystroke subscription plumbing.
pub struct ProductFields {
    pub name: Entity<InputState>,
    pub publisher: Entity<InputState>,
    pub version: Entity<InputState>,
    pub description: Entity<InputState>,
}

pub struct UpdateFields {
    pub feed_url: Entity<InputState>,
}

pub struct OpenProject {
    pub project: Project,
    pub file: Option<PathBuf>,
    pub dirty: bool,
    pub doctor: Option<inst_doctor::Report>,
    pub product_fields: ProductFields,
    pub update_fields: UpdateFields,
    pub version_error: Option<String>,
    pub last_analysis: Option<String>,
}

impl OpenProject {
    fn new(
        project: Project,
        file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut App,
    ) -> OpenProject {
        let version = project.product.version.to_string();
        let description = project.product.description.get(Language::En).to_owned();
        OpenProject {
            product_fields: ProductFields {
                name: cx.new(|cx| {
                    InputState::new(window, cx).default_value(project.product.name.clone())
                }),
                publisher: cx.new(|cx| {
                    InputState::new(window, cx).default_value(project.product.publisher.clone())
                }),
                version: cx.new(|cx| InputState::new(window, cx).default_value(version)),
                description: cx.new(|cx| InputState::new(window, cx).default_value(description)),
            },
            update_fields: UpdateFields {
                feed_url: cx.new(|cx| {
                    InputState::new(window, cx)
                        .default_value(project.update.feed_url.clone().unwrap_or_default())
                }),
            },
            project,
            file,
            dirty: false,
            doctor: None,
            version_error: None,
            last_analysis: None,
        }
    }
}

/// Background build/doctor/analyze progress, polled on a timer since GPUI
/// entities are not `Send`.
#[derive(Default)]
pub struct TaskProgress {
    pub lines: Vec<String>,
    pub done: bool,
    pub ok: bool,
}

pub fn lock_progress(m: &Mutex<TaskProgress>) -> MutexGuard<'_, TaskProgress> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct StudioView {
    pub catalog: Catalog,
    pub project: Option<OpenProject>,
    pub diagnostics: Vec<Diagnostic>,
    pub section: Section,
    pub bottom_tab: BottomTab,
    pub palette_open: bool,
    pub palette_query: Entity<InputState>,
    pub lang: Language,
    pub busy: Option<Arc<Mutex<TaskProgress>>>,
    pub cancel: Arc<AtomicBool>,
    pub h_split: Entity<ResizableState>,
    pub v_split: Entity<ResizableState>,
    pub show_explorer: bool,
    pub show_inspector: bool,
    pub show_bottom: bool,
}

impl StudioView {
    pub fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> StudioView {
        let lang = inst_i18n::detect_system_language(&inst_i18n::studio::SUPPORTED_LANGUAGES);
        StudioView {
            catalog: Catalog::builtin(),
            project: None,
            diagnostics: Vec::new(),
            section: Section::Product,
            bottom_tab: BottomTab::Output,
            palette_open: false,
            palette_query: cx.new(|cx| InputState::new(window, cx)),
            lang,
            busy: None,
            cancel: Arc::new(AtomicBool::new(false)),
            h_split: cx.new(|_| ResizableState::default()),
            v_split: cx.new(|_| ResizableState::default()),
            show_explorer: true,
            show_inspector: true,
            show_bottom: true,
        }
    }

    pub fn visible_sections(&self) -> &'static [Section] {
        match self.project.as_ref().map(|p| p.project.studio.mode) {
            Some(inst_model::project::EditorMode::Advanced) => Section::ALL,
            _ => Section::SIMPLE,
        }
    }

    pub fn is_advanced(&self) -> bool {
        matches!(
            self.project.as_ref().map(|p| p.project.studio.mode),
            Some(inst_model::project::EditorMode::Advanced)
        )
    }

    pub fn set_advanced(&mut self, advanced: bool) {
        if let Some(p) = &mut self.project {
            p.project.studio.mode = if advanced {
                inst_model::project::EditorMode::Advanced
            } else {
                inst_model::project::EditorMode::Simple
            };
            if !advanced && !Section::SIMPLE.contains(&self.section) {
                self.section = Section::Product;
            }
        }
    }

    /// Pulls the current text of every bound input into the project, and
    /// revalidates. Called once per render.
    pub fn sync_and_validate(&mut self, cx: &App) {
        let Some(p) = &mut self.project else {
            self.diagnostics.clear();
            return;
        };
        let name = p.product_fields.name.read(cx).value().to_string();
        if name != p.project.product.name {
            p.project.product.name = name;
            p.dirty = true;
        }
        let publisher = p.product_fields.publisher.read(cx).value().to_string();
        if publisher != p.project.product.publisher {
            p.project.product.publisher = publisher;
            p.dirty = true;
        }
        let description = p.product_fields.description.read(cx).value().to_string();
        if description != p.project.product.description.get(Language::En) {
            p.project.product.description.set(Language::En, description);
            p.dirty = true;
        }
        let version_text = p.product_fields.version.read(cx).value().to_string();
        if version_text != p.project.product.version.to_string() {
            match inst_model::Version::parse(&version_text) {
                Ok(v) => {
                    p.project.product.version = v;
                    p.dirty = true;
                    p.version_error = None;
                }
                Err(e) => p.version_error = Some(e.to_string()),
            }
        } else {
            p.version_error = None;
        }
        let feed = p.update_fields.feed_url.read(cx).value().to_string();
        let feed = if feed.trim().is_empty() {
            None
        } else {
            Some(feed)
        };
        if feed != p.project.update.feed_url {
            p.project.update.feed_url = feed;
            p.dirty = true;
        }
        self.diagnostics = validate::validate(&p.project, &self.catalog);
    }

    pub fn start_task(
        &mut self,
        cx: &mut Context<'_, Self>,
        run: impl FnOnce(&Arc<Mutex<TaskProgress>>, &Arc<AtomicBool>) + Send + 'static,
    ) {
        self.cancel.store(false, Ordering::Relaxed);
        let progress = Arc::new(Mutex::new(TaskProgress::default()));
        self.busy = Some(progress.clone());
        let cancel = self.cancel.clone();
        cx.spawn(async move |this, cx| {
            let task_progress = progress.clone();
            let bg = cx
                .background_executor()
                .spawn(async move { run(&task_progress, &cancel) });
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(80))
                    .await;
                let done = lock_progress(&progress).done;
                if this.update(cx, |_, cx| cx.notify()).is_err() || done {
                    break;
                }
            }
            bg.await;
            let _ = this.update(cx, |view, cx| {
                view.busy = None;
                cx.notify();
            });
        })
        .detach();
    }

    pub fn run_doctor(&mut self, cx: &mut Context<'_, Self>) {
        let Some(p) = &self.project else { return };
        let report = inst_doctor::examine(
            &p.project,
            p.file.as_deref().unwrap_or(std::path::Path::new(".")),
            &self.catalog,
        );
        if let Some(p) = &mut self.project {
            p.doctor = Some(report);
        }
        self.bottom_tab = BottomTab::Doctor;
        cx.notify();
    }

    pub fn apply_safe_fixes(&mut self, cx: &mut Context<'_, Self>) {
        let Some(p) = &mut self.project else { return };
        if let Some(report) = &p.doctor {
            report.fix_all_safe(&mut p.project);
            p.dirty = true;
        }
        self.run_doctor(cx);
    }

    pub fn run_analyze(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(p) = &self.project else { return };
        let dir = p
            .project
            .source_dir(p.file.as_deref().unwrap_or(std::path::Path::new(".")));
        match inst_analyzer::analyze(&dir) {
            Ok(a) => {
                let text = a.to_string();
                if let Some(p) = &mut self.project {
                    p.last_analysis = Some(text);
                }
            }
            Err(e) => {
                if let Some(p) = &mut self.project {
                    p.last_analysis = Some(format!("error: {e}"));
                }
            }
        }
        self.bottom_tab = BottomTab::Output;
        let _ = window;
        cx.notify();
    }

    pub fn run_build(&mut self, cx: &mut Context<'_, Self>) {
        let Some(p) = &self.project else { return };
        let project = p.project.clone();
        let project_file = p.file.clone().unwrap_or_else(|| {
            project
                .source_dir(std::path::Path::new("."))
                .join("project.instproj")
        });
        let catalog = self.catalog.clone();
        self.bottom_tab = BottomTab::Build;
        self.start_task(cx, move |progress, cancel| {
            struct Observer<'a>(&'a Mutex<TaskProgress>, &'a AtomicBool);
            impl inst_builder::BuildObserver for Observer<'_> {
                fn stage(&self, target: Option<Target>, stage: inst_builder::Stage) {
                    let mut p = lock_progress(self.0);
                    match target {
                        Some(t) => p.lines.push(format!("» [{t}] {}", stage.label())),
                        None => p.lines.push(format!("» {}", stage.label())),
                    }
                }
                fn log(&self, line: &str) {
                    lock_progress(self.0).lines.push(format!("  {line}"));
                }
                fn cancelled(&self) -> bool {
                    self.1.load(Ordering::Relaxed)
                }
            }
            let mut req = inst_builder::BuildRequest::new(project_file, project);
            req.targets = vec![Target::LINUX_X64];
            let observer = Observer(progress, cancel);
            let result = inst_builder::build(&req, &catalog, &observer);
            let mut guard = lock_progress(progress);
            match result {
                Ok(reports) => {
                    for r in reports {
                        guard.lines.push(String::new());
                        guard.lines.push(r.to_string());
                    }
                    guard.ok = true;
                }
                Err(e) => {
                    guard.lines.push(format!("error: {e}"));
                    guard.ok = false;
                }
            }
            guard.done = true;
        });
    }

    pub fn new_project_from(
        &mut self,
        dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match inst_analyzer::analyze(&dir) {
            Ok(analysis) => {
                let project_dir = dir.clone();
                let project = inst_analyzer::to_project(&analysis, &project_dir);
                let file = project_dir.join(format!("project.{}", inst_brand::PROJECT_EXTENSION));
                self.project = Some(OpenProject::new(project, Some(file), window, cx));
                if let Some(p) = &mut self.project {
                    p.last_analysis = Some(analysis.to_string());
                    p.dirty = true;
                }
                self.section = Section::Product;
                self.bottom_tab = BottomTab::Output;
            }
            Err(e) => {
                if let Some(p) = &mut self.project {
                    p.last_analysis = Some(format!("error: {e}"));
                }
            }
        }
        cx.notify();
    }

    pub fn open_project_from(
        &mut self,
        file: PathBuf,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match std::fs::read_to_string(&file)
            .map_err(|e| e.to_string())
            .and_then(|text| inst_model::io::from_toml(&text).map_err(|e| e.to_string()))
        {
            Ok(project) => {
                self.project = Some(OpenProject::new(project, Some(file), window, cx));
                self.section = Section::Product;
                self.bottom_tab = BottomTab::Output;
            }
            Err(e) => {
                self.project = None;
                eprintln!("error opening project: {e}");
            }
        }
        cx.notify();
    }

    pub fn save_project(&mut self, cx: &mut Context<'_, Self>) {
        let Some(p) = &mut self.project else { return };
        let Some(file) = p.file.clone() else { return };
        match inst_model::io::to_toml(&p.project) {
            Ok(text) => match inst_fsx::write_atomic(&file, text.as_bytes(), false) {
                Ok(()) => p.dirty = false,
                Err(e) => eprintln!("error saving project: {e}"),
            },
            Err(e) => eprintln!("error serializing project: {e}"),
        }
        cx.notify();
    }

    pub fn pick_new_project(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let window_handle = window.window_handle();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = receiver.await
                && let Some(dir) = paths.pop()
            {
                let _ = cx.update_window(window_handle, move |_, window, cx| {
                    let _ = this.update(cx, |view, cx| view.new_project_from(dir, window, cx));
                });
            }
        })
        .detach();
    }

    pub fn pick_open_project(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let window_handle = window.window_handle();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = receiver.await
                && let Some(file) = paths.pop()
            {
                let _ = cx.update_window(window_handle, move |_, window, cx| {
                    let _ = this.update(cx, |view, cx| view.open_project_from(file, window, cx));
                });
            }
        })
        .detach();
    }

    pub fn run_palette_action(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match index {
            0 => self.pick_new_project(window, cx),
            1 => self.pick_open_project(window, cx),
            2 => self.save_project(cx),
            3 => self.run_build(cx),
            4 => self.run_doctor(cx),
            5 => self.run_analyze(window, cx),
            _ => {}
        }
        cx.notify();
    }
}
