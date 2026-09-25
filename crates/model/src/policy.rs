//! Central default ON/OFF policy.
//!
//! Every safety-relevant default lives here, with its rationale, so the
//! Studio can show *why* something is on or off and so a change of policy is
//! a one-line, reviewable diff. Model types use these constants for their
//! `Default` implementations and serde defaults.

use serde::{Deserialize, Serialize};

/// Defaults that must be ON: they protect the end user and cost little.
pub mod on {
    pub const INTEGRITY_VERIFICATION: bool = true;
    pub const ROLLBACK: bool = true;
    pub const LOGGING: bool = true;
    pub const SILENT_INSTALL: bool = true;
    pub const SILENT_UNINSTALL: bool = true;
    pub const UNINSTALLER: bool = true;
    pub const DETECT_EXISTING_VERSION: bool = true;
    pub const ARCHITECTURE_CHECK: bool = true;
    pub const DISK_SPACE_CHECK: bool = true;
    pub const PREREQUISITE_DETECTION: bool = true;
    pub const ATOMIC_FILE_OPERATIONS: bool = true;
    pub const FAILURE_RECOVERY: bool = true;
    /// Start menu / application menu entry: expected by users, reversible.
    pub const START_MENU_ENTRY: bool = true;
}

/// Defaults that must be OFF: they change the system beyond the application
/// folder, affect other software, or have privacy implications. The
/// developer must opt in explicitly.
pub mod off {
    pub const DESKTOP_SHORTCUT: bool = false;
    pub const LAUNCH_AT_STARTUP: bool = false;
    pub const PATH_MODIFICATION: bool = false;
    pub const FILE_ASSOCIATIONS: bool = false;
    pub const PROTOCOL_ASSOCIATIONS: bool = false;
    pub const FIREWALL_RULES: bool = false;
    pub const SERVICE_INSTALLATION: bool = false;
    pub const TELEMETRY: bool = false;
    pub const AUTOMATIC_REBOOT: bool = false;
}

/// A documented default, for display in the Studio.
#[derive(Clone, Copy, Debug)]
pub struct DocumentedDefault {
    pub key: &'static str,
    pub value: bool,
    pub rationale: &'static str,
}

/// Every default, in display order.
pub const DEFAULTS: &[DocumentedDefault] = &[
    DocumentedDefault { key: "integrity-verification", value: on::INTEGRITY_VERIFICATION, rationale: "A corrupted or tampered setup must never install anything." },
    DocumentedDefault { key: "rollback", value: on::ROLLBACK, rationale: "A failed installation must leave the system as it was." },
    DocumentedDefault { key: "logging", value: on::LOGGING, rationale: "Failures must be diagnosable; secrets are always redacted." },
    DocumentedDefault { key: "silent-install", value: on::SILENT_INSTALL, rationale: "Administrators deploy software unattended." },
    DocumentedDefault { key: "silent-uninstall", value: on::SILENT_UNINSTALL, rationale: "Unattended removal is as important as unattended installation." },
    DocumentedDefault { key: "uninstaller", value: on::UNINSTALLER, rationale: "Every installed product must be removable." },
    DocumentedDefault { key: "detect-existing-version", value: on::DETECT_EXISTING_VERSION, rationale: "Upgrades and repairs must not create duplicate installations." },
    DocumentedDefault { key: "architecture-check", value: on::ARCHITECTURE_CHECK, rationale: "Installing binaries for the wrong CPU produces a broken application." },
    DocumentedDefault { key: "disk-space-check", value: on::DISK_SPACE_CHECK, rationale: "Fail before copying anything instead of half-way through." },
    DocumentedDefault { key: "prerequisite-detection", value: on::PREREQUISITE_DETECTION, rationale: "Never download or reinstall what is already present." },
    DocumentedDefault { key: "atomic-file-operations", value: on::ATOMIC_FILE_OPERATIONS, rationale: "Files are either the old or the new version, never half-written." },
    DocumentedDefault { key: "failure-recovery", value: on::FAILURE_RECOVERY, rationale: "Interrupted installations can be repaired or cleanly retried." },
    DocumentedDefault { key: "start-menu-entry", value: on::START_MENU_ENTRY, rationale: "Users expect to find installed applications in the system menu." },
    DocumentedDefault { key: "desktop-shortcut", value: off::DESKTOP_SHORTCUT, rationale: "The desktop belongs to the user; add icons only when asked." },
    DocumentedDefault { key: "launch-at-startup", value: off::LAUNCH_AT_STARTUP, rationale: "Slows down every login; must be an explicit product decision." },
    DocumentedDefault { key: "path-modification", value: off::PATH_MODIFICATION, rationale: "Changes how every other program resolves commands." },
    DocumentedDefault { key: "file-associations", value: off::FILE_ASSOCIATIONS, rationale: "Takes over file types from applications the user already chose." },
    DocumentedDefault { key: "protocol-associations", value: off::PROTOCOL_ASSOCIATIONS, rationale: "Takes over URL schemes system-wide." },
    DocumentedDefault { key: "firewall-rules", value: off::FIREWALL_RULES, rationale: "Opens the machine to the network; needs administrator consent." },
    DocumentedDefault { key: "service-installation", value: off::SERVICE_INSTALLATION, rationale: "Background services run without the user; needs elevation." },
    DocumentedDefault { key: "telemetry", value: off::TELEMETRY, rationale: "Privacy: no data leaves the machine unless the product opts in." },
    DocumentedDefault { key: "automatic-reboot", value: off::AUTOMATIC_REBOOT, rationale: "Rebooting without consent loses the user's work." },
];

/// Installer behaviour switches. Serialized in the project so that a policy
/// change in a new Studio version never silently changes an existing
/// project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Policy {
    pub integrity_verification: bool,
    pub rollback: bool,
    pub logging: bool,
    pub silent_install: bool,
    pub silent_uninstall: bool,
    pub uninstaller: bool,
    pub detect_existing_version: bool,
    pub architecture_check: bool,
    pub disk_space_check: bool,
    pub prerequisite_detection: bool,
    pub atomic_file_operations: bool,
    pub failure_recovery: bool,
    pub telemetry: bool,
    pub reboot: RebootPolicy,
    pub same_version: SameVersionPolicy,
    pub allow_downgrade: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            integrity_verification: on::INTEGRITY_VERIFICATION,
            rollback: on::ROLLBACK,
            logging: on::LOGGING,
            silent_install: on::SILENT_INSTALL,
            silent_uninstall: on::SILENT_UNINSTALL,
            uninstaller: on::UNINSTALLER,
            detect_existing_version: on::DETECT_EXISTING_VERSION,
            architecture_check: on::ARCHITECTURE_CHECK,
            disk_space_check: on::DISK_SPACE_CHECK,
            prerequisite_detection: on::PREREQUISITE_DETECTION,
            atomic_file_operations: on::ATOMIC_FILE_OPERATIONS,
            failure_recovery: on::FAILURE_RECOVERY,
            telemetry: off::TELEMETRY,
            reboot: RebootPolicy::default(),
            same_version: SameVersionPolicy::default(),
            allow_downgrade: false,
        }
    }
}

/// What to do when a prerequisite or action requests a reboot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RebootPolicy {
    /// Never reboot; tell the user a restart is pending.
    #[default]
    Never,
    /// Ask the user (never in silent mode).
    Prompt,
    /// Reboot automatically. Explicit opt-in only.
    Automatic,
}

/// Behaviour when the same version is already installed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SameVersionPolicy {
    /// Offer repair.
    #[default]
    Repair,
    /// Reinstall over the existing files.
    Reinstall,
    /// Refuse.
    Block,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_default_matches_documented_table() {
        let p = Policy::default();
        let lookup = |k: &str| DEFAULTS.iter().find(|d| d.key == k).map(|d| d.value);
        assert_eq!(lookup("rollback"), Some(p.rollback));
        assert_eq!(lookup("telemetry"), Some(p.telemetry));
        assert_eq!(lookup("integrity-verification"), Some(p.integrity_verification));
        assert!(DEFAULTS.iter().filter(|d| !d.value).all(|d| !d.rationale.is_empty()));
        assert_eq!(p.reboot, RebootPolicy::Never);
    }
}
