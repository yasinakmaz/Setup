//! Events from the engine to a frontend (GUI or console).

use std::path::PathBuf;

use crate::error::InstallError;
use crate::journal::RollbackReport;

/// What kind of installation is running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Fresh,
    Upgrade { from: String },
    Repair,
    Downgrade { from: String },
}

#[derive(Clone, Debug)]
pub struct Success {
    pub install_dir: PathBuf,
    pub reboot_required: bool,
    /// Main executable, arguments and working directory for "Open application".
    pub launch: Option<(PathBuf, Vec<String>, PathBuf)>,
    pub log: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Failure {
    pub error: InstallError,
    /// `None` when nothing had to be reverted.
    pub rollback: Option<RollbackReport>,
    pub log: Option<PathBuf>,
}

pub type Outcome = Result<Success, Box<Failure>>;

#[derive(Clone, Debug)]
pub enum Event {
    StepStarted(usize),
    /// Progress inside the current step, 0.0 ..= 1.0.
    StepProgress {
        step: usize,
        fraction: f32,
    },
    StepFinished(usize),
    StepSkipped(usize),
    /// A short status line (e.g. download progress).
    Detail(String),
    RollingBack,
    Finished(Box<Outcome>),
}

pub trait EventSink: Send + Sync {
    fn send(&self, event: Event);
}

/// Discards events.
pub struct NullSink;
impl EventSink for NullSink {
    fn send(&self, _: Event) {}
}

impl<F: Fn(Event) + Send + Sync> EventSink for F {
    fn send(&self, event: Event) {
        self(event);
    }
}
