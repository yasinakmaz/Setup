//! The installation context passed to the generated graph function.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use inst_i18n::Language;
use inst_log::{Logger, log};

use crate::app::{App, Scope};
use crate::error::{ErrorKind, InstallError};
use crate::events::{Event, EventSink, Mode};
use crate::journal::{Journal, Undo};
use crate::manifest::{External, Manifest, ManifestFile};
use crate::platform::{self, Folder};
use crate::spec::{Secret, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    X64,
    Arm64,
}

impl Os {
    pub const fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

impl Arch {
    pub const fn current() -> Arch {
        if cfg!(target_arch = "aarch64") {
            Arch::Arm64
        } else {
            Arch::X64
        }
    }
}

/// Facts about the machine, fixed for the duration of an installation.
#[derive(Clone, Debug)]
pub struct Env {
    pub os: Os,
    pub arch: Arch,
    pub elevated: bool,
}

impl Env {
    pub fn detect() -> Env {
        Env {
            os: Os::current(),
            arch: Arch::current(),
            elevated: platform::is_elevated(),
        }
    }
}

/// Choices made by the user (UI) or the command line.
#[derive(Clone, Debug)]
pub struct InstallOptions {
    pub install_dir: PathBuf,
    pub language: Language,
    /// Input field values by id. Secret values are registered with the
    /// log redactor before the installation starts.
    pub inputs: Vec<(String, String)>,
    pub desktop_shortcut: bool,
    pub mode: Mode,
    pub previous: Option<Manifest>,
}

impl InstallOptions {
    pub fn input(&self, id: &str) -> Option<&str> {
        self.inputs
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, v)| v.as_str())
    }
}

/// State shared by install and uninstall contexts.
pub(crate) struct Common<'a> {
    pub app: &'static App,
    pub env: Env,
    pub logger: Logger,
    pub events: &'a dyn EventSink,
    pub cancel: &'a AtomicBool,
    pub install_dir: PathBuf,
    pub language: Language,
    pub captured: Vec<(String, String)>,
}

impl Common<'_> {
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn machine(&self) -> bool {
        self.app.settings.scope == Scope::Machine
    }

    pub fn folder(&self, f: Folder, rel: &str) -> PathBuf {
        let base = platform::known_folder(f, self.machine(), &self.install_dir);
        if rel.is_empty() {
            base
        } else {
            let mut p = base;
            for c in rel.split('/') {
                p.push(c);
            }
            p
        }
    }
}

/// Context of an installation (fresh, upgrade or repair).
pub struct Install<'a> {
    pub(crate) c: Common<'a>,
    pub(crate) options: &'a InstallOptions,
    pub(crate) exe: PathBuf,
    pub(crate) payload: inst_payload::Payload,
    pub(crate) journal: Option<Journal>,
    pub(crate) files: Vec<ManifestFile>,
    pub(crate) dirs: Vec<String>,
    pub(crate) external: Vec<External>,
    pub(crate) prereq_missing: Vec<bool>,
    pub(crate) prereq_files: Vec<Option<PathBuf>>,
    pub(crate) prereqs_installed: Vec<(String, String)>,
    pub(crate) reboot_required: bool,
    pub(crate) current_step: usize,
    last_progress: Option<(Instant, f32)>,
}

impl<'a> Install<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        app: &'static App,
        options: &'a InstallOptions,
        logger: Logger,
        events: &'a dyn EventSink,
        cancel: &'a AtomicBool,
        exe: PathBuf,
        payload: inst_payload::Payload,
    ) -> Self {
        Install {
            c: Common {
                app,
                env: Env::detect(),
                logger,
                events,
                cancel,
                install_dir: options.install_dir.clone(),
                language: options.language,
                captured: Vec::new(),
            },
            options,
            exe,
            payload,
            journal: None,
            files: Vec::new(),
            dirs: Vec::new(),
            external: Vec::new(),
            prereq_missing: vec![false; app.prerequisites.len()],
            prereq_files: vec![None; app.prerequisites.len()],
            prereqs_installed: Vec::new(),
            reboot_required: false,
            current_step: 0,
            last_progress: None,
        }
    }

    // ------------------------------------------------------------ steps

    /// Runs step `index` of the progress list.
    pub fn step(
        &mut self,
        index: usize,
        f: impl FnOnce(&mut Self) -> Result<(), InstallError>,
    ) -> Result<(), InstallError> {
        if self.c.is_cancelled() {
            return Err(InstallError::cancelled());
        }
        self.current_step = index;
        self.last_progress = None;
        let id = self.c.app.steps.get(index).map_or("?", |s| s.id);
        log!(self.c.logger, Info, "step", "step started"; "step" => id);
        self.c.events.send(Event::StepStarted(index));
        let started = Instant::now();
        let result = f(self);
        match &result {
            Ok(()) => {
                log!(self.c.logger, Info, "step", "step finished";
                    "step" => id, "ms" => started.elapsed().as_millis() as u64);
                self.c.events.send(Event::StepFinished(index));
            }
            Err(e) => {
                log!(self.c.logger, Error, "step", "step failed"; "step" => id, "error" => &e.to_string());
            }
        }
        result
    }

    /// Marks step `index` as skipped (its condition was false).
    pub fn skip(&mut self, index: usize) {
        self.c.events.send(Event::StepSkipped(index));
    }

    /// Reports progress inside the current step (throttled).
    pub fn progress(&mut self, done: u64, total: u64) {
        let fraction = if total == 0 {
            1.0
        } else {
            (done as f64 / total as f64) as f32
        };
        let now = Instant::now();
        if let Some((t, f)) = self.last_progress
            && now.duration_since(t).as_millis() < 50
            && fraction - f < 0.01
            && fraction < 1.0
        {
            return;
        }
        self.last_progress = Some((now, fraction));
        self.c.events.send(Event::StepProgress {
            step: self.current_step,
            fraction,
        });
    }

    pub fn detail(&self, text: impl Into<String>) {
        self.c.events.send(Event::Detail(text.into()));
    }

    // ------------------------------------------------------- conditions

    pub fn env(&self) -> &Env {
        &self.c.env
    }

    pub fn elevated(&self) -> bool {
        self.c.env.elevated
    }

    pub fn is_fresh_install(&self) -> bool {
        self.options.mode == Mode::Fresh
    }

    pub fn is_upgrade(&self) -> bool {
        matches!(self.options.mode, Mode::Upgrade { .. })
    }

    pub fn is_repair(&self) -> bool {
        self.options.mode == Mode::Repair
    }

    /// A checkbox option of the installer UI.
    pub fn option(&self, id: &str) -> bool {
        matches!(self.options.input(id), Some("true" | "1" | "yes" | "on"))
    }

    pub fn prerequisite_missing(&self, index: usize) -> bool {
        self.prereq_missing.get(index).copied().unwrap_or(false)
    }

    pub fn file_exists(&self, folder: Folder, rel: &str) -> bool {
        self.c.folder(folder, rel).exists()
    }

    pub fn env_var_set(&self, name: &str) -> bool {
        std::env::var_os(name).is_some_and(|v| !v.is_empty())
    }

    pub fn env_var_equals(&self, name: &str, value: &str) -> bool {
        std::env::var(name).is_ok_and(|v| v == value)
    }

    pub fn command_available(&self, name: &str) -> bool {
        crate::detect::find_on_path(name).is_some()
    }

    pub fn captured_equals(&self, name: &str, value: &str) -> bool {
        self.c.captured.iter().any(|(k, v)| k == name && v == value)
    }

    pub fn windows_build_at_least(&self, build: u32) -> bool {
        crate::detect::windows_build().is_some_and(|b| b >= build)
    }

    // ----------------------------------------------------------- values

    pub fn install_dir(&self) -> &Path {
        &self.c.install_dir
    }

    pub fn language(&self) -> Language {
        self.c.language
    }

    pub fn logger(&self) -> &Logger {
        &self.c.logger
    }

    pub(crate) fn resolve(&self, v: &Value) -> Result<String, InstallError> {
        resolve_value(&self.c, Some(self.options), v)
    }

    /// The transaction journal, created on first use (requires the
    /// installation directory to be resolved).
    pub(crate) fn journal(&mut self) -> Result<&mut Journal, InstallError> {
        if self.journal.is_none() {
            let journal = Journal::create(&self.c.install_dir, self.c.app.product.id)
                .map_err(|e| InstallError::io("creating the transaction journal", &e))?;
            log!(self.c.logger, Debug, "journal", "journal created";
                "dir" => &journal.dir().to_string_lossy().into_owned());
            self.journal = Some(journal);
        }
        Ok(self.journal.as_mut().expect("journal was just created"))
    }

    pub(crate) fn record(&mut self, undo: Undo) -> Result<(), InstallError> {
        self.journal()?
            .record(undo)
            .map_err(|e| InstallError::io("writing the transaction journal", &e))
    }
}

/// Context of uninstall hooks (`before-uninstall`, `after-uninstall`).
pub struct Uninstall<'a> {
    pub(crate) c: Common<'a>,
}

impl Uninstall<'_> {
    pub fn env(&self) -> &Env {
        &self.c.env
    }

    pub fn install_dir(&self) -> &Path {
        &self.c.install_dir
    }

    pub fn elevated(&self) -> bool {
        self.c.env.elevated
    }

    pub fn file_exists(&self, folder: Folder, rel: &str) -> bool {
        self.c.folder(folder, rel).exists()
    }

    pub fn env_var_set(&self, name: &str) -> bool {
        std::env::var_os(name).is_some_and(|v| !v.is_empty())
    }

    pub fn env_var_equals(&self, name: &str, value: &str) -> bool {
        std::env::var(name).is_ok_and(|v| v == value)
    }

    pub fn command_available(&self, name: &str) -> bool {
        crate::detect::find_on_path(name).is_some()
    }

    pub(crate) fn resolve(&self, v: &Value) -> Result<String, InstallError> {
        resolve_value(&self.c, None, v)
    }
}

pub(crate) fn resolve_value(
    c: &Common<'_>,
    options: Option<&InstallOptions>,
    v: &Value,
) -> Result<String, InstallError> {
    let missing = |what: &str| {
        InstallError::new(
            ErrorKind::Unexpected,
            format!("no value for {what} (pass it with --set in silent mode)"),
        )
    };
    Ok(match v {
        Value::Lit(s) => (*s).to_owned(),
        Value::Input(id) => options
            .and_then(|o| o.input(id))
            .map(str::to_owned)
            .ok_or_else(|| missing(id))?,
        Value::Secret(Secret::Input(id)) => {
            let value = options
                .and_then(|o| o.input(id))
                .map(str::to_owned)
                .ok_or_else(|| missing(id))?;
            c.logger.redactor().register(&value);
            value
        }
        Value::Secret(Secret::Env(var)) => {
            let value = std::env::var(var).map_err(|_| missing(var))?;
            c.logger.redactor().register(&value);
            value
        }
        Value::Path(folder, rel) => c.folder(*folder, rel).to_string_lossy().into_owned(),
        Value::ProductVersion => c.app.product.version.to_owned(),
        Value::ProductName => c.app.product.name.to_owned(),
        Value::Captured(name) => c
            .captured
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .ok_or_else(|| missing(name))?,
    })
}
