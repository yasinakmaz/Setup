//! Actionable installer errors.
//!
//! Every error has a translated, user-facing sentence (no raw error codes)
//! and separate technical details for "Show details" / "Copy error" and the
//! log. Exit codes follow Windows Installer conventions so administrators
//! can script around them.

use std::fmt;

use inst_i18n::Language;
use inst_i18n::installer::Msg;

/// MSI-compatible process exit codes.
pub mod exit {
    pub const SUCCESS: i32 = 0;
    pub const INVALID_ARGUMENTS: i32 = 87;
    pub const USER_CANCELLED: i32 = 1602;
    pub const FATAL: i32 = 1603;
    pub const DAMAGED_PACKAGE: i32 = 1620;
    pub const ANOTHER_VERSION: i32 = 1638;
    pub const UNSUPPORTED_PLATFORM: i32 = 1633;
    pub const DISK_FULL: i32 = 112;
    pub const ACCESS_DENIED: i32 = 5;
    pub const REBOOT_REQUIRED: i32 = 3010;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The setup file failed integrity verification.
    Damaged,
    DiskSpace {
        required: u64,
        available: u64,
    },
    AccessDenied,
    RequiresAdmin,
    PrerequisiteVerify {
        name: String,
    },
    PrerequisiteDownload {
        name: String,
    },
    PrerequisiteInstall {
        name: String,
    },
    TaskFailed {
        name: String,
    },
    Unsupported,
    /// A newer (or blocked same) version is installed.
    AlreadyInstalled {
        name: String,
        version: String,
    },
    Cancelled,
    Unexpected,
}

#[derive(Clone, Debug)]
pub struct InstallError {
    pub kind: ErrorKind,
    /// Technical details (already redacted before display).
    pub details: String,
}

impl InstallError {
    pub fn new(kind: ErrorKind, details: impl Into<String>) -> Self {
        InstallError {
            kind,
            details: details.into(),
        }
    }

    pub fn unexpected(details: impl fmt::Display) -> Self {
        InstallError::new(ErrorKind::Unexpected, details.to_string())
    }

    pub fn cancelled() -> Self {
        InstallError::new(ErrorKind::Cancelled, "cancelled by the user")
    }

    /// Maps an I/O error to the most helpful kind.
    pub fn io(context: &str, e: &std::io::Error) -> Self {
        let kind = match e.kind() {
            std::io::ErrorKind::PermissionDenied => ErrorKind::AccessDenied,
            std::io::ErrorKind::StorageFull => ErrorKind::DiskSpace {
                required: 0,
                available: 0,
            },
            _ => ErrorKind::Unexpected,
        };
        InstallError::new(kind, format!("{context}: {e}"))
    }

    /// The user-facing sentence in `lang`.
    pub fn message(&self, lang: Language) -> String {
        let mut out = String::new();
        let t = |m: Msg| m.text(lang);
        let r = match &self.kind {
            ErrorKind::Damaged => inst_i18n::format_into(&mut out, t(Msg::ErrDamaged), &[]),
            ErrorKind::DiskSpace {
                required,
                available,
            } => {
                let req = inst_fsx::format_bytes(*required);
                let avail = inst_fsx::format_bytes(*available);
                inst_i18n::format_into(&mut out, t(Msg::ErrDiskSpace), &[&req, &avail])
            }
            ErrorKind::AccessDenied => {
                inst_i18n::format_into(&mut out, t(Msg::ErrAccessDenied), &[])
            }
            ErrorKind::RequiresAdmin => {
                inst_i18n::format_into(&mut out, t(Msg::RequiresAdmin), &[])
            }
            ErrorKind::PrerequisiteVerify { name } => {
                inst_i18n::format_into(&mut out, t(Msg::ErrPrerequisiteVerify), &[name])
            }
            ErrorKind::PrerequisiteDownload { name } => {
                inst_i18n::format_into(&mut out, t(Msg::ErrPrerequisiteDownload), &[name])
            }
            ErrorKind::PrerequisiteInstall { name } => {
                inst_i18n::format_into(&mut out, t(Msg::ErrPrerequisiteInstall), &[name])
            }
            ErrorKind::TaskFailed { name } => {
                inst_i18n::format_into(&mut out, t(Msg::ErrTaskFailed), &[name])
            }
            ErrorKind::Unsupported => {
                inst_i18n::format_into(&mut out, t(Msg::ErrUnsupportedSystem), &[])
            }
            ErrorKind::AlreadyInstalled { name, version } => {
                inst_i18n::format_into(&mut out, t(Msg::AlreadyInstalled), &[name, version])
            }
            ErrorKind::Cancelled => inst_i18n::format_into(&mut out, t(Msg::Cancel), &[]),
            ErrorKind::Unexpected => inst_i18n::format_into(&mut out, t(Msg::ErrUnexpected), &[]),
        };
        debug_assert!(r.is_ok());
        out
    }

    pub fn exit_code(&self) -> i32 {
        match self.kind {
            ErrorKind::Damaged => exit::DAMAGED_PACKAGE,
            ErrorKind::DiskSpace { .. } => exit::DISK_FULL,
            ErrorKind::AccessDenied | ErrorKind::RequiresAdmin => exit::ACCESS_DENIED,
            ErrorKind::Unsupported => exit::UNSUPPORTED_PLATFORM,
            ErrorKind::AlreadyInstalled { .. } => exit::ANOTHER_VERSION,
            ErrorKind::Cancelled => exit::USER_CANCELLED,
            _ => exit::FATAL,
        }
    }

    /// Whether offering "Retry" makes sense.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::PrerequisiteDownload { .. }
                | ErrorKind::PrerequisiteVerify { .. }
                | ErrorKind::DiskSpace { .. }
                | ErrorKind::AccessDenied
                | ErrorKind::TaskFailed { .. }
        )
    }
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.details)
    }
}

impl std::error::Error for InstallError {}

impl From<inst_payload::PayloadError> for InstallError {
    fn from(e: inst_payload::PayloadError) -> Self {
        if e.is_corruption() {
            return InstallError::new(ErrorKind::Damaged, e.to_string());
        }
        match &e {
            inst_payload::PayloadError::Cancelled => InstallError::cancelled(),
            inst_payload::PayloadError::Io(io) => InstallError::io("extracting files", io),
            _ => InstallError::unexpected(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_human() {
        let e = InstallError::new(
            ErrorKind::PrerequisiteVerify {
                name: "PostgreSQL".into(),
            },
            "sha256 mismatch",
        );
        let msg = e.message(Language::En);
        assert!(
            msg.starts_with("PostgreSQL could not be installed."),
            "{msg}"
        );
        assert!(!msg.contains("0x"));
        let e = InstallError::new(
            ErrorKind::DiskSpace {
                required: 300 << 20,
                available: 100 << 20,
            },
            "",
        );
        assert_eq!(
            e.message(Language::En),
            "Not enough disk space. 300.00 MiB required, 100.00 MiB available."
        );
        assert_eq!(e.exit_code(), exit::DISK_FULL);
    }
}
