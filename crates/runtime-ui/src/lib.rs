//! Single-screen graphical installer UI, built on GPUI Kit.
//!
//! One window carries the whole flow: options/confirmation, progress, then
//! finish/error. There is no wizard with multiple pages. Layout mirrors and
//! text right-aligns for RTL languages (`inst_i18n::Direction`).

use std::fmt::Display;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{ActiveTheme as _, Root, Theme, ThemeMode};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use inst_i18n::Language;
use inst_runtime::app::{App as InstallerApp, Field, FieldKind};
use inst_runtime::context::InstallOptions;
use inst_runtime::error::exit;
use inst_runtime::events::{Event, EventSink, Outcome};
use inst_runtime::{Msg, Session};

fn t(msg: Msg, lang: Language) -> &'static str {
    msg.text(lang)
}

fn tf(msg: Msg, lang: Language, args: &[&dyn Display]) -> String {
    let mut out = String::new();
    let _ = inst_i18n::format_into(&mut out, msg.text(lang), args);
    out
}

/// Runs the installer or uninstaller UI and returns the process exit code
/// (before platform mapping, same convention as `console::run`).
pub fn run(session: Session) -> i32 {
    let session = Arc::new(session);
    let exit_code = Arc::new(AtomicI32::new(exit::USER_CANCELLED));
    let title = if session.cli.uninstall {
        format!("{} Uninstall", session.app.product.name)
    } else {
        tf(
            Msg::SetupTitle,
            session.language,
            &[&session.app.product.name],
        )
    };

    let exit_code_for_app = exit_code.clone();
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);

        let bounds = WindowBounds::centered(size(px(620.), px(460.)), cx);
        let window_options = WindowOptions {
            window_bounds: Some(bounds),
            titlebar: Some(TitlebarOptions {
                title: Some(title.clone().into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let session = session.clone();
        let exit_code = exit_code_for_app;
        cx.spawn(async move |cx| {
            let exit_code_for_view = exit_code.clone();
            let opened = cx.open_window(window_options, move |window, cx| {
                let view = cx.new(|cx| InstallerView::new(session, exit_code_for_view, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            });
            if let Err(e) = opened {
                // No GPU-capable rendering surface (e.g. a bare X server with
                // no software rasterizer): fail cleanly instead of a raw panic.
                eprintln!("error: could not open the installer window: {e}");
                exit_code.store(exit::FATAL, Ordering::SeqCst);
                cx.update(|cx| cx.quit());
            }
        })
        .detach();

        cx.on_window_closed(|cx, _id| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
    });

    exit_code.load(Ordering::SeqCst)
}

/// Progress shared between the background install/uninstall thread and the
/// UI, which polls it on a timer (GPUI entities are not `Send`).
#[derive(Default)]
struct Progress2 {
    step_index: Option<usize>,
    fraction: f32,
    detail: Option<String>,
    rolling_back: bool,
    outcome: Option<Outcome>,
}

fn lock(m: &Mutex<Progress2>) -> MutexGuard<'_, Progress2> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

struct SharedSink(Arc<Mutex<Progress2>>);

impl EventSink for SharedSink {
    fn send(&self, event: Event) {
        let mut s = lock(&self.0);
        match event {
            Event::StepStarted(i) => {
                s.step_index = Some(i);
                s.fraction = 0.0;
                s.detail = None;
            }
            Event::StepProgress { step, fraction } => {
                if s.step_index == Some(step) {
                    s.fraction = fraction;
                }
            }
            Event::StepFinished(i) => {
                if s.step_index == Some(i) {
                    s.fraction = 1.0;
                }
            }
            Event::StepSkipped(_) => {}
            Event::Detail(d) => s.detail = Some(d),
            Event::RollingBack => s.rolling_back = true,
            Event::Finished(outcome) => s.outcome = Some(*outcome),
        }
    }
}

enum Widget {
    Text(Entity<InputState>),
    Password(Entity<InputState>),
    Checkbox(bool),
    Select(String),
    Number(Entity<InputState>),
}

struct FieldRow {
    field: &'static Field,
    widget: Widget,
}

enum Phase {
    /// Options / confirmation screen, before starting.
    Options,
    Running(Arc<Mutex<Progress2>>),
    Done(Outcome),
}

struct InstallerView {
    session: Arc<Session>,
    exit_code: Arc<AtomicI32>,
    lang: Language,
    dir: Entity<InputState>,
    fields: Vec<FieldRow>,
    desktop_shortcut: bool,
    show_advanced: bool,
    phase: Phase,
    error_message: Option<String>,
}

impl InstallerView {
    fn new(
        session: Arc<Session>,
        exit_code: Arc<AtomicI32>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let lang = session.language;
        let default_dir = session.default_dir.to_string_lossy().into_owned();
        let dir = cx.new(|cx| InputState::new(window, cx).default_value(default_dir));
        let fields = session
            .app
            .settings
            .fields
            .iter()
            .map(|field| {
                let widget =
                    match field.kind {
                        FieldKind::Text { default } => Widget::Text(
                            cx.new(|cx| InputState::new(window, cx).default_value(default)),
                        ),
                        FieldKind::Password => {
                            Widget::Password(cx.new(|cx| InputState::new(window, cx).masked(true)))
                        }
                        FieldKind::Checkbox { default } => Widget::Checkbox(default),
                        FieldKind::Select { default, .. } => Widget::Select(default.to_owned()),
                        FieldKind::Number { default, .. } => Widget::Number(cx.new(|cx| {
                            InputState::new(window, cx).default_value(default.to_string())
                        })),
                    };
                FieldRow { field, widget }
            })
            .collect();
        InstallerView {
            desktop_shortcut: session.app.settings.integration.desktop_shortcut,
            session,
            exit_code,
            lang,
            dir,
            fields,
            show_advanced: false,
            phase: Phase::Options,
            error_message: None,
        }
    }

    fn build_options(&self, cx: &App) -> InstallOptions {
        let mut opts = self.session.default_options();
        opts.install_dir = PathBuf::from(self.dir.read(cx).value().to_string());
        opts.desktop_shortcut = self.desktop_shortcut;
        opts.language = self.lang;
        for row in &self.fields {
            let value = match &row.widget {
                Widget::Text(s) | Widget::Password(s) => s.read(cx).value().to_string(),
                Widget::Checkbox(b) => b.to_string(),
                Widget::Select(v) => v.clone(),
                Widget::Number(s) => {
                    let raw = s.read(cx).value().to_string();
                    let (min, max) = match row.field.kind {
                        FieldKind::Number { min, max, .. } => (min, max),
                        _ => (i64::MIN, i64::MAX),
                    };
                    raw.trim()
                        .parse::<i64>()
                        .map(|n| n.clamp(min, max))
                        .unwrap_or_default()
                        .to_string()
                }
            };
            opts.inputs.retain(|(k, _)| k != row.field.id);
            opts.inputs.push((row.field.id.to_owned(), value));
        }
        opts
    }

    fn start_install(&mut self, cx: &mut Context<'_, Self>) {
        let options = self.build_options(cx);
        if let Err(msg) = self.session.validate(&options) {
            self.error_message = Some(msg);
            cx.notify();
            return;
        }
        self.error_message = None;
        let progress = Arc::new(Mutex::new(Progress2::default()));
        self.phase = Phase::Running(progress.clone());
        self.run_in_background(cx, progress, move |session, sink| {
            session.install(&options, sink)
        });
    }

    fn start_uninstall(&mut self, cx: &mut Context<'_, Self>) {
        let progress = Arc::new(Mutex::new(Progress2::default()));
        self.phase = Phase::Running(progress.clone());
        self.run_in_background(cx, progress, |session, sink| session.uninstall(sink));
    }

    fn run_in_background(
        &mut self,
        cx: &mut Context<'_, Self>,
        progress: Arc<Mutex<Progress2>>,
        run: impl FnOnce(&Session, &SharedSink) -> Outcome + Send + 'static,
    ) {
        let session = self.session.clone();
        cx.spawn(async move |this, cx| {
            let sink = SharedSink(progress.clone());
            let install_task = cx
                .background_executor()
                .spawn(async move { run(&session, &sink) });
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(60))
                    .await;
                let done = lock(&progress).outcome.is_some();
                if this.update(cx, |_, cx| cx.notify()).is_err() || done {
                    break;
                }
            }
            let _ = install_task.await;
            let _ = this.update(cx, |view, cx| {
                view.poll_progress();
                cx.notify();
            });
        })
        .detach();
    }

    fn poll_progress(&mut self) {
        let Phase::Running(progress) = &self.phase else {
            return;
        };
        let outcome = lock(progress).outcome.clone();
        if let Some(outcome) = outcome {
            let code = match &outcome {
                Ok(s) if s.reboot_required => exit::REBOOT_REQUIRED,
                Ok(_) => exit::SUCCESS,
                Err(f) => f.error.exit_code(),
            };
            self.exit_code.store(code, Ordering::SeqCst);
            self.phase = Phase::Done(outcome);
        }
    }

    fn quit(&mut self, window: &mut Window, _cx: &mut Context<'_, Self>) {
        window.remove_window();
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let dir_entity = self.dir.clone();
        let window_handle = window.window_handle();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t(Msg::ChooseLocation, self.lang).into()),
        });
        cx.spawn(async move |_this, cx| {
            if let Ok(Ok(Some(mut paths))) = receiver.await
                && let Some(path) = paths.pop()
            {
                let _ = cx.update_window(window_handle, move |_, window, cx| {
                    dir_entity.update(cx, |state, cx| {
                        state.set_value(path.to_string_lossy().into_owned(), window, cx);
                    });
                });
            }
        })
        .detach();
    }
}

impl Render for InstallerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        if matches!(self.phase, Phase::Running(_)) {
            self.poll_progress();
        }
        let rtl = self.lang.direction().is_rtl();
        let theme = cx.theme().clone();

        let body = match &self.phase {
            Phase::Options if self.session.cli.uninstall => self.render_uninstall_confirm(cx),
            Phase::Options => self.render_install_options(cx),
            Phase::Running(progress) => {
                let snapshot = lock(progress);
                self.render_progress(&snapshot, &theme)
            }
            Phase::Done(outcome) => {
                let outcome = outcome.clone();
                self.render_done(outcome, cx, &theme)
            }
        };

        v_flex()
            .id("installer-root")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                v_flex()
                    .size_full()
                    .p_6()
                    .gap_4()
                    .when(rtl, |d| d.text_right())
                    .child(body),
            )
    }
}

impl InstallerView {
    fn header(&self) -> impl IntoElement {
        let lang = self.lang;
        let product = self.session.app.product;
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .child(product.name.to_owned()),
            )
            .child(div().text_sm().text_color(rgba(0x999999ff)).child(format!(
                "{} {}",
                t(Msg::Version, lang),
                product.version
            )))
    }

    fn render_install_options(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let lang = self.lang;
        let product = self.session.app.product;
        let allow_change = self.session.app.settings.allow_change_location;

        let mut column = v_flex()
            .gap_4()
            .child(self.header())
            .child(div().text_sm().child(product.description(lang).to_owned()));

        for row in &self.fields {
            column = column.child(render_field(row, lang));
        }

        column = column.child(
            Checkbox::new("desktop-shortcut")
                .label(t(Msg::DesktopShortcut, lang))
                .checked(self.desktop_shortcut)
                .on_click(cx.listener(|view, checked: &bool, _, cx| {
                    view.desktop_shortcut = *checked;
                    cx.notify();
                })),
        );

        column = column.child(
            Button::new("advanced-toggle")
                .ghost()
                .compact()
                .label(t(Msg::AdvancedOptions, lang))
                .on_click(cx.listener(|view, _, _, cx| {
                    view.show_advanced = !view.show_advanced;
                    cx.notify();
                })),
        );

        if self.show_advanced {
            let mut advanced = v_flex().gap_2();
            if allow_change {
                advanced = advanced.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_sm()
                                .w(px(160.))
                                .child(t(Msg::InstallLocation, lang)),
                        )
                        .child(Input::new(&self.dir).id("install-dir"))
                        .child(
                            Button::new("change-dir")
                                .ghost()
                                .label(t(Msg::Change, lang))
                                .on_click(
                                    cx.listener(|view, _, window, cx| view.browse(window, cx)),
                                ),
                        ),
                );
            }
            if self.session.app.settings.language_selector {
                advanced = advanced.child(self.render_language_row(cx));
            }
            column = column.child(advanced);
        }

        if let Some(err) = &self.error_message {
            column = column.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(err.clone()),
            );
        }

        column = column.child(
            h_flex()
                .gap_2()
                .justify_end()
                .child(
                    Button::new("cancel-install")
                        .ghost()
                        .label(t(Msg::Cancel, lang))
                        .on_click(cx.listener(|view, _, window, cx| view.quit(window, cx))),
                )
                .child(
                    Button::new("install")
                        .primary()
                        .label(t(Msg::Install, lang))
                        .on_click(cx.listener(|view, _, _, cx| view.start_install(cx))),
                ),
        );

        column.into_any_element()
    }

    fn render_uninstall_confirm(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let lang = self.lang;
        v_flex()
            .gap_4()
            .child(self.header())
            .child(div().text_sm().child(tf(
                Msg::UninstallConfirm,
                lang,
                &[&self.session.app.product.name],
            )))
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("cancel-uninstall")
                            .ghost()
                            .label(t(Msg::Cancel, lang))
                            .on_click(cx.listener(|view, _, window, cx| view.quit(window, cx))),
                    )
                    .child(
                        Button::new("uninstall")
                            .danger()
                            .label(t(Msg::Uninstall, lang))
                            .on_click(cx.listener(|view, _, _, cx| view.start_uninstall(cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_language_row(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let lang = self.lang;
        h_flex()
            .gap_2()
            .items_center()
            .flex_wrap()
            .child(
                div()
                    .text_sm()
                    .w(px(160.))
                    .child(t(Msg::LanguageLabel, lang)),
            )
            .children(self.session.app.settings.languages.iter().map(|l| {
                let selected = *l == lang;
                let l = *l;
                Button::new(SharedString::from(format!("lang-{}", l.code())))
                    .compact()
                    .when(selected, |b| b.primary())
                    .label(l.native_name())
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.lang = l;
                        cx.notify();
                    }))
            }))
    }

    fn render_progress(&self, progress: &Progress2, theme: &Theme) -> AnyElement {
        let lang = self.lang;
        let label = if self.session.cli.uninstall {
            t(Msg::Uninstalling, lang).to_owned()
        } else {
            progress
                .step_index
                .and_then(|i| self.session.app.steps.get(i))
                .map_or_else(
                    || t(Msg::Installing, lang).to_owned(),
                    |s| s.label.text(lang).to_owned(),
                )
        };
        let fraction = if self.session.cli.uninstall {
            progress.fraction * 100.0
        } else {
            overall_fraction(self.session.app, progress)
        };
        let mut column = v_flex()
            .gap_4()
            .child(self.header())
            .child(div().text_sm().child(label))
            .child(Progress::new("progress").value(fraction));
        if progress.rolling_back {
            column = column.child(
                div()
                    .text_sm()
                    .text_color(theme.danger)
                    .child(t(Msg::RollbackIncomplete, lang)),
            );
        }
        if let Some(detail) = &progress.detail {
            column = column.child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(detail.clone()),
            );
        }
        column.into_any_element()
    }

    fn render_done(
        &self,
        outcome: Outcome,
        cx: &mut Context<'_, Self>,
        theme: &Theme,
    ) -> AnyElement {
        let lang = self.lang;
        let product = self.session.app.product;
        let close_button = |cx: &mut Context<'_, Self>| {
            Button::new("close")
                .primary()
                .label(t(Msg::Close, lang))
                .on_click(cx.listener(|view, _, window, cx| view.quit(window, cx)))
        };
        match outcome {
            Ok(_) if self.session.cli.uninstall => v_flex()
                .gap_4()
                .child(self.header())
                .child(div().text_color(theme.success).child(tf(
                    Msg::UninstallCompleted,
                    lang,
                    &[&product.name],
                )))
                .child(h_flex().justify_end().child(close_button(cx)))
                .into_any_element(),
            Ok(success) => {
                let mut actions = h_flex().gap_2().justify_end();
                if let Some((exe, args, cwd)) = success.launch {
                    actions = actions.child(
                        Button::new("launch")
                            .ghost()
                            .label(t(Msg::OpenApplication, lang))
                            .on_click(move |_, _, _| {
                                let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                                let _ = inst_runtime::platform::launch_detached(&exe, &refs, &cwd);
                            }),
                    );
                }
                actions = actions.child(close_button(cx));
                v_flex()
                    .gap_4()
                    .child(self.header())
                    .child(
                        div()
                            .text_color(theme.success)
                            .child(t(Msg::InstallCompleted, lang)),
                    )
                    .child(actions)
                    .into_any_element()
            }
            Err(failure) => {
                let mut column = v_flex().gap_4().child(self.header()).child(
                    div()
                        .text_color(theme.danger)
                        .child(failure.error.message(lang)),
                );
                if let Some(rollback) = &failure.rollback {
                    let msg = if rollback.is_complete() {
                        t(Msg::ChangesReverted, lang)
                    } else {
                        t(Msg::RollbackIncomplete, lang)
                    };
                    column = column.child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(msg),
                    );
                }
                let mut actions = h_flex().gap_2().justify_end();
                if failure.error.is_retryable() {
                    let retry_uninstall = self.session.cli.uninstall;
                    actions = actions.child(
                        Button::new("retry")
                            .ghost()
                            .label(t(Msg::Retry, lang))
                            .on_click(cx.listener(move |view, _, _, cx| {
                                if retry_uninstall {
                                    view.start_uninstall(cx);
                                } else {
                                    view.start_install(cx);
                                }
                            })),
                    );
                }
                actions = actions.child(close_button(cx));
                column.child(actions).into_any_element()
            }
        }
    }
}

fn render_field(row: &FieldRow, lang: Language) -> impl IntoElement {
    let label = row.field.label(lang);
    let control: AnyElement = match &row.widget {
        Widget::Text(s) => Input::new(s)
            .id(SharedString::from(row.field.id))
            .into_any_element(),
        Widget::Password(s) => Input::new(s)
            .id(SharedString::from(row.field.id))
            .mask_toggle()
            .into_any_element(),
        Widget::Checkbox(checked) => Checkbox::new(SharedString::from(row.field.id))
            .checked(*checked)
            .into_any_element(),
        Widget::Select(current) => {
            let FieldKind::Select { options, .. } = row.field.kind else {
                unreachable!("a select widget always pairs with a select field kind")
            };
            h_flex()
                .gap_1()
                .children(options.iter().map(|(value, label)| {
                    let selected = value == current;
                    Button::new(SharedString::from(format!("{}-{value}", row.field.id)))
                        .compact()
                        .when(selected, |b| b.primary())
                        .label(*label)
                }))
                .into_any_element()
        }
        Widget::Number(s) => Input::new(s)
            .id(SharedString::from(row.field.id))
            .into_any_element(),
    };
    h_flex()
        .gap_2()
        .items_center()
        .child(div().text_sm().w(px(160.)).child(label))
        .child(control)
}

fn overall_fraction(app: &'static InstallerApp, progress: &Progress2) -> f32 {
    let total: u32 = app.steps.iter().map(|s| s.weight.max(1)).sum();
    if total == 0 {
        return 0.0;
    }
    let mut done = 0.0f32;
    for (i, step) in app.steps.iter().enumerate() {
        let weight = step.weight.max(1) as f32;
        match progress.step_index {
            Some(cur) if i < cur => done += weight,
            Some(cur) if i == cur => done += weight * progress.fraction,
            _ => {}
        }
    }
    (done / total as f32 * 100.0).clamp(0.0, 100.0)
}
