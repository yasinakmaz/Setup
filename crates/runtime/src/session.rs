//! A setup session: everything a frontend needs before, during and after
//! an installation or uninstallation.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use inst_i18n::Language;
use inst_log::{Format, Level, Logger, log};

use crate::app::{App, Scope};
use crate::cli::CliOptions;
use crate::context::{Common, Env, Install, InstallOptions};
use crate::error::{ErrorKind, InstallError};
use crate::events::{Event, EventSink, Failure, Mode, Outcome, Success};
use crate::manifest::{METADATA_DIR, Manifest};
use crate::platform;

pub struct Session {
    pub app: &'static App,
    pub cli: CliOptions,
    pub logger: Logger,
    pub log_path: Option<PathBuf>,
    pub language: Language,
    /// Previously installed version, if detected.
    pub previous: Option<Manifest>,
    pub mode: Mode,
    pub default_dir: PathBuf,
    pub exe: PathBuf,
    /// File holding the payload (`$APPIMAGE` inside an AppImage).
    pub payload_file: PathBuf,
    /// The payload, or why it could not be read (damaged setup).
    pub payload: Result<inst_payload::Payload, InstallError>,
    pub cancel: Arc<AtomicBool>,
}

impl Session {
    pub fn new(app: &'static App, cli: CliOptions) -> Session {
        let settings = app.settings;
        let language = match cli.language.as_deref() {
            Some(tag) => Language::negotiate(Some(tag), settings.languages),
            None => inst_i18n::detect_system_language(settings.languages),
        };
        let language = if settings.languages.contains(&language) {
            language
        } else {
            settings.fallback_language
        };

        let (logger, log_path) = make_logger(app, &cli);
        log!(logger, Info, "setup", "{} {}", inst_brand::RUNTIME_NAME, env!("CARGO_PKG_VERSION");
            "product" => app.product.id, "version" => app.product.version, "language" => language.code());

        let exe = std::env::current_exe().unwrap_or_default();
        let payload_file = appimage_file().unwrap_or_else(|| exe.clone());
        let payload = if cli.uninstall {
            Err(InstallError::new(
                ErrorKind::Unexpected,
                "uninstaller has no payload",
            ))
        } else {
            std::fs::File::open(&payload_file)
                .map_err(|e| InstallError::io("opening the setup file", &e))
                .and_then(|mut f| inst_payload::Payload::locate(&mut f).map_err(InstallError::from))
        };
        if let Err(e) = &payload
            && !cli.uninstall
        {
            log!(logger, Error, "setup", "payload unavailable"; "error" => &e.to_string());
        }

        let machine = settings.scope == Scope::Machine;
        let previous = if settings.policy.detect_existing_version {
            platform::find_installed(app.product.id, machine)
                .and_then(|dir| Manifest::load(&dir).ok())
                .filter(|m| m.product_id == app.product.id)
        } else {
            None
        };
        let mode = Install::classify(previous.as_ref(), app.product.version);
        let default_dir = cli
            .dir
            .clone()
            .or_else(|| previous.as_ref().map(|m| m.install_dir.clone()))
            .unwrap_or_else(|| {
                platform::install_base(machine)
                    .unwrap_or_else(|_| std::env::temp_dir())
                    .join(settings.folder)
            });
        if let Some(p) = &previous {
            log!(logger, Info, "setup", "existing installation found";
                "version" => p.version.as_str(), "dir" => &p.install_dir.to_string_lossy().into_owned());
        }
        Session {
            app,
            cli,
            logger,
            log_path,
            language,
            previous,
            mode,
            default_dir,
            exe,
            payload_file,
            payload,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Options pre-filled from defaults and the command line.
    pub fn default_options(&self) -> InstallOptions {
        let mut inputs: Vec<(String, String)> = Vec::new();
        for f in self.app.settings.fields {
            let default = match f.kind {
                crate::app::FieldKind::Text { default } => default.to_owned(),
                crate::app::FieldKind::Checkbox { default } => default.to_string(),
                crate::app::FieldKind::Select { default, .. } => default.to_owned(),
                crate::app::FieldKind::Number { default, .. } => default.to_string(),
                crate::app::FieldKind::Password => continue,
            };
            inputs.push((f.id.to_owned(), default));
        }
        for (k, v) in &self.cli.sets {
            inputs.retain(|(ik, _)| ik != k);
            inputs.push((k.clone(), v.clone()));
        }
        InstallOptions {
            install_dir: self.default_dir.clone(),
            language: self.language,
            inputs,
            desktop_shortcut: self
                .cli
                .desktop_shortcut
                .unwrap_or(self.app.settings.integration.desktop_shortcut),
            mode: self.mode.clone(),
            previous: self.previous.clone(),
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Validates required input fields before starting.
    pub fn validate(&self, options: &InstallOptions) -> Result<(), String> {
        for f in self.app.settings.fields {
            if f.required && options.input(f.id).is_none_or(str::is_empty) {
                return Err(format!("{} is required", f.label(options.language)));
            }
        }
        if !options.install_dir.is_absolute() {
            return Err("the installation folder must be an absolute path".into());
        }
        Ok(())
    }

    /// Runs the installation synchronously (call from a worker thread when
    /// a UI is shown). Sends `Finished` at the end.
    pub fn install(&self, options: &InstallOptions, events: &dyn EventSink) -> Outcome {
        self.cancel.store(false, Ordering::Relaxed);
        let outcome = self.install_inner(options, events);
        self.logger.flush();
        events.send(Event::Finished(Box::new(outcome.clone())));
        outcome
    }

    fn install_inner(&self, options: &InstallOptions, events: &dyn EventSink) -> Outcome {
        let fail = |error: InstallError, rollback| {
            Box::new(Failure {
                error,
                rollback,
                log: self.log_path.clone(),
            })
        };
        for f in self.app.settings.fields.iter().filter(|f| f.is_secret()) {
            if let Some(v) = options.input(f.id) {
                self.logger.redactor().register(v);
            }
        }
        let payload = match &self.payload {
            Ok(p) => p.clone(),
            Err(e) => return Err(fail(e.clone(), None)),
        };
        if let Err(msg) = self.validate(options) {
            return Err(fail(InstallError::new(ErrorKind::Unexpected, msg), None));
        }
        log!(self.logger, Info, "install", "installation started";
            "dir" => &options.install_dir.to_string_lossy().into_owned(), "mode" => &format!("{:?}", options.mode));

        let mut cx = Install::new(
            self.app,
            options,
            self.logger.clone(),
            events,
            &self.cancel,
            self.exe.clone(),
            self.payload_file.clone(),
            payload,
        );
        let result = (self.app.install)(&mut cx).and_then(|()| cx.commit());
        match result {
            Ok(()) => {
                let dir = options.install_dir.clone();
                let launch = self.app.product.main_executable.and_then(|m| {
                    let exe = inst_fsx::RelPath::new(m).ok()?.to_path_under(&dir);
                    exe.exists().then(|| {
                        (
                            exe,
                            self.app
                                .product
                                .arguments
                                .iter()
                                .map(|s| (*s).to_owned())
                                .collect(),
                            dir.clone(),
                        )
                    })
                });
                log!(self.logger, Info, "install", "installation completed";
                    "reboot" => cx.reboot_required);
                let log = self.persist_log(&dir);
                Ok(Success {
                    install_dir: dir,
                    reboot_required: cx.reboot_required,
                    launch,
                    log,
                })
            }
            Err(error) => {
                log!(self.logger, Error, "install", "installation failed"; "error" => &error.to_string());
                let rollback = match cx.journal.take() {
                    Some(journal) if self.app.settings.policy.rollback => {
                        events.send(Event::RollingBack);
                        Some(journal.rollback(&platform::Platform, &self.logger))
                    }
                    Some(journal) => {
                        let _ = journal.commit();
                        None
                    }
                    None => None,
                };
                Err(fail(error, rollback))
            }
        }
    }

    /// Copies the log into the installation's metadata folder.
    fn persist_log(&self, install_dir: &Path) -> Option<PathBuf> {
        let src = self.log_path.as_ref()?;
        self.logger.flush();
        let dest = install_dir.join(METADATA_DIR).join("install.log");
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::copy(src, &dest).ok().map(|_| dest)
    }

    /// Runs the uninstallation synchronously.
    pub fn uninstall(&self, events: &dyn EventSink) -> Outcome {
        let result = self.uninstall_inner(events);
        self.logger.flush();
        events.send(Event::Finished(Box::new(result.clone())));
        result
    }

    fn uninstall_inner(&self, events: &dyn EventSink) -> Outcome {
        let (dir, manifest) =
            crate::uninstall::locate(self.app, self.cli.dir.as_deref()).map_err(|error| {
                Box::new(Failure {
                    error,
                    rollback: None,
                    log: self.log_path.clone(),
                })
            })?;
        let c = Common {
            app: self.app,
            env: Env::detect(),
            logger: self.logger.clone(),
            events,
            cancel: &self.cancel,
            install_dir: dir.clone(),
            language: self.language,
            captured: Vec::new(),
        };
        if self.app.settings.scope == Scope::Machine && !c.env.elevated {
            return Err(Box::new(Failure {
                error: InstallError::new(
                    ErrorKind::RequiresAdmin,
                    "uninstalling a machine-wide installation",
                ),
                rollback: None,
                log: self.log_path.clone(),
            }));
        }
        crate::uninstall::run(c, &manifest, events)
            .map(|_| Success {
                install_dir: dir,
                reboot_required: false,
                launch: None,
                log: self.log_path.clone(),
            })
            .map_err(|error| {
                Box::new(Failure {
                    error,
                    rollback: None,
                    log: self.log_path.clone(),
                })
            })
    }

    /// Verifies every payload block (for `--verify`).
    pub fn verify(&self) -> Result<(), InstallError> {
        let payload = self.payload.as_ref().map_err(Clone::clone)?;
        let mut f = std::fs::File::open(&self.payload_file)
            .map_err(|e| InstallError::io("opening the setup file", &e))?;
        inst_payload::read::verify_blocks(payload, &mut f, |_| {}).map_err(InstallError::from)
    }
}

/// The AppImage file when running inside one (payload lives there).
fn appimage_file() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let p = PathBuf::from(std::env::var_os("APPIMAGE")?);
    (p.is_absolute() && p.is_file()).then_some(p)
}

fn make_logger(app: &'static App, cli: &CliOptions) -> (Logger, Option<PathBuf>) {
    if !app.settings.policy.logging && cli.log.is_none() {
        return (Logger::disabled(), None);
    }
    let path = cli.log.clone().unwrap_or_else(|| {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let kind = if cli.uninstall { "uninstall" } else { "setup" };
        std::env::temp_dir().join(format!("{}-{kind}-{secs}.log", app.product.id))
    });
    match inst_log::file_sink(&path, Level::Debug, Format::Text) {
        Ok(sink) => (
            Logger::builder().sink(sink, Format::Text).build(),
            Some(path),
        ),
        Err(_) => (Logger::disabled(), None),
    }
}
