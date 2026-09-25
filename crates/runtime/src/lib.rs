//! Installer Runtime.
//!
//! This crate is compiled into every generated installer, together with the
//! code generated from the project (static descriptors + the installation
//! graph as a Rust function). It contains no designer, compiler or
//! interpreter code.
//!
//! Entry point for generated code:
//!
//! ```ignore
//! fn main() {
//!     inst_runtime::main(&APP, Some(inst_runtime_ui::run));
//! }
//! ```

pub mod actions;
pub mod app;
pub mod cli;
pub mod context;
pub mod detect;
pub mod error;
pub mod events;
pub mod journal;
pub mod manifest;
pub mod ops;
pub mod platform;
pub mod prereq;
pub mod script;
pub mod services;
pub mod session;
pub mod spec;
pub mod uninstall;

use std::io::Write;

pub use inst_i18n::{Language, installer::Msg};
pub use session::Session;

/// Everything generated code needs.
pub mod prelude {
    pub use crate::app::*;
    pub use crate::context::{Arch, Install, Os, Uninstall};
    pub use crate::detect::Detection;
    pub use crate::error::InstallError;
    pub use crate::platform::{Folder, HKCU, HKLM};
    pub use crate::prereq::{Arg, InstallKind, Prerequisite, Source};
    pub use crate::services::StartMode;
    pub use crate::spec::*;
    pub use inst_i18n::Language;
    pub use inst_i18n::installer::Msg;
}

/// A graphical frontend: runs the UI and returns the process exit code.
pub type Gui = fn(Session) -> i32;

/// Maps an MSI-style exit code to the platform convention. Windows keeps
/// the MSI codes; POSIX exit statuses are 8 bits wide.
pub fn platform_exit_code(code: i32) -> i32 {
    if cfg!(windows) {
        return code;
    }
    use error::exit;
    match code {
        exit::SUCCESS => 0,
        exit::REBOOT_REQUIRED => 0,
        exit::INVALID_ARGUMENTS => 2,
        exit::USER_CANCELLED => 3,
        exit::DAMAGED_PACKAGE => 4,
        exit::ACCESS_DENIED => 5,
        exit::DISK_FULL => 6,
        exit::ANOTHER_VERSION => 7,
        exit::UNSUPPORTED_PLATFORM => 8,
        _ => 1,
    }
}

/// Runs the installer and exits the process.
pub fn main(app: &'static app::App, gui: Option<Gui>) -> ! {
    let cli = match cli::parse(std::env::args_os().skip(1)) {
        Ok(c) => c,
        Err(msg) => {
            let _ = writeln!(std::io::stderr(), "{msg}\n\n{}", cli::USAGE);
            std::process::exit(platform_exit_code(error::exit::INVALID_ARGUMENTS));
        }
    };
    if cli.help {
        let _ = writeln!(
            std::io::stdout(),
            "{} {}\n\n{}",
            app.product.name,
            app.product.version,
            cli::USAGE
        );
        std::process::exit(0);
    }
    let session = Session::new(app, cli);
    let code = if session.cli.verify_only {
        console::verify(&session)
    } else {
        match gui {
            Some(gui) if !session.cli.silent && platform::has_display() => gui(session),
            _ => console::run(session),
        }
    };
    std::process::exit(platform_exit_code(code));
}

pub mod console {
    //! Silent / text-mode frontend.

    use std::io::Write;

    use crate::error::exit;
    use crate::events::{Event, Outcome};
    use crate::session::Session;

    /// Installs or uninstalls without UI. Progress goes to stderr unless
    /// `--silent`; details always go to the log file.
    pub fn run(session: Session) -> i32 {
        let quiet = session.cli.silent;
        let lang = session.language;
        let steps = session.app.steps;
        let sink = move |e: Event| {
            if quiet {
                return;
            }
            let mut err = std::io::stderr();
            match e {
                Event::StepStarted(i) => {
                    if let Some(s) = steps.get(i) {
                        let _ = writeln!(err, "» {}", s.label.text(lang));
                    }
                }
                Event::Detail(d) => {
                    let _ = writeln!(err, "  {d}");
                }
                Event::RollingBack => {
                    let _ = writeln!(err, "» rolling back");
                }
                _ => {}
            }
        };
        let policy = session.app.settings.policy;
        let outcome: Outcome = if session.cli.uninstall {
            if quiet && !policy.silent_uninstall {
                return exit::INVALID_ARGUMENTS;
            }
            session.uninstall(&sink)
        } else {
            if quiet && !policy.silent_install {
                return exit::INVALID_ARGUMENTS;
            }
            let options = session.default_options();
            session.install(&options, &sink)
        };
        match outcome {
            Ok(s) => {
                if !quiet {
                    let _ = writeln!(
                        std::io::stderr(),
                        "✓ {}",
                        crate::Msg::InstallCompleted.text(lang)
                    );
                }
                if session.cli.launch
                    && let Some((exe, args, cwd)) = &s.launch
                {
                    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                    let _ = crate::platform::launch_detached(exe, &refs, cwd);
                }
                if s.reboot_required {
                    exit::REBOOT_REQUIRED
                } else {
                    exit::SUCCESS
                }
            }
            Err(f) => {
                let mut err = std::io::stderr();
                let _ = writeln!(err, "✗ {}", f.error.message(lang));
                let _ = writeln!(
                    err,
                    "  {}",
                    session.logger.redactor().scrubbed(&f.error.details)
                );
                if let Some(r) = &f.rollback {
                    let msg = if r.is_complete() {
                        crate::Msg::ChangesReverted
                    } else {
                        crate::Msg::RollbackIncomplete
                    };
                    let _ = writeln!(err, "  {}", msg.text(lang));
                }
                if let Some(log) = &f.log {
                    let _ = writeln!(err, "  log: {}", log.display());
                }
                f.error.exit_code()
            }
        }
    }

    pub fn verify(session: &Session) -> i32 {
        match session.verify() {
            Ok(()) => {
                let _ = writeln!(std::io::stdout(), "OK");
                exit::SUCCESS
            }
            Err(e) => {
                let _ = writeln!(
                    std::io::stderr(),
                    "{}: {}",
                    e.message(session.language),
                    e.details
                );
                e.exit_code()
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod engine_tests;
