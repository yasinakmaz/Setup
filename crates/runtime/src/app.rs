//! Static descriptors emitted by code generation.
//!
//! Everything here is `'static` data: a generated installer describes its
//! product, settings and steps as `static` items, and its installation graph
//! as a plain Rust function. Nothing is parsed or interpreted at runtime.

use inst_i18n::Language;
use inst_i18n::installer::Msg;

use crate::context::{Install, Uninstall};
use crate::error::InstallError;

/// Product identity and presentation.
#[derive(Debug)]
pub struct Product {
    pub id: &'static str,
    pub name: &'static str,
    pub publisher: &'static str,
    pub version: &'static str,
    /// Per-language description (English first).
    pub description: &'static [(Language, &'static str)],
    pub homepage: Option<&'static str>,
    pub support_url: Option<&'static str>,
    /// Main executable, relative to the installation directory.
    pub main_executable: Option<&'static str>,
    pub arguments: &'static [&'static str],
    /// Icon file inside the installation directory (for shortcuts and the
    /// Installed Apps entry).
    pub icon: Option<&'static str>,
    /// PNG bytes of the product logo for the installer UI.
    pub logo_png: Option<&'static [u8]>,
}

impl Product {
    pub fn description(&self, lang: Language) -> &'static str {
        self.description
            .iter()
            .find(|(l, _)| *l == lang)
            .or_else(|| self.description.first())
            .map_or("", |(_, d)| d)
    }
}

/// Installation scope, resolved at build time from the privilege analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Per-user location, no elevation.
    User,
    /// Machine-wide location; requires administrator/root.
    Machine,
}

#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub integrity_verification: bool,
    pub rollback: bool,
    pub logging: bool,
    pub silent_install: bool,
    pub silent_uninstall: bool,
    pub uninstaller: bool,
    pub detect_existing_version: bool,
    pub disk_space_check: bool,
    pub failure_recovery: bool,
    pub allow_downgrade: bool,
    pub same_version: SameVersion,
    pub reboot: Reboot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SameVersion {
    Repair,
    Reinstall,
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reboot {
    Never,
    Prompt,
    Automatic,
}

#[derive(Clone, Copy, Debug)]
pub struct Integration {
    pub start_menu: bool,
    pub desktop_shortcut: bool,
    pub launch_at_startup: bool,
    pub add_to_path: bool,
    pub offer_launch: bool,
    pub file_associations: &'static [FileAssociation],
    pub protocols: &'static [Protocol],
}

#[derive(Clone, Copy, Debug)]
pub struct FileAssociation {
    pub extension: &'static str,
    pub description: &'static str,
    pub mime_type: Option<&'static str>,
}

#[derive(Clone, Copy, Debug)]
pub struct Protocol {
    pub scheme: &'static str,
    pub description: &'static str,
}

/// Installer input field (rendered by the UI, `--set id=value` when silent).
#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub id: &'static str,
    pub label: &'static [(Language, &'static str)],
    pub kind: FieldKind,
    pub required: bool,
    pub prominent: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum FieldKind {
    Text {
        default: &'static str,
    },
    Password,
    Checkbox {
        default: bool,
    },
    Select {
        options: &'static [(&'static str, &'static str)],
        default: &'static str,
    },
    Number {
        default: i64,
        min: i64,
        max: i64,
    },
}

impl Field {
    pub fn label(&self, lang: Language) -> &'static str {
        self.label
            .iter()
            .find(|(l, _)| *l == lang)
            .or_else(|| self.label.first())
            .map_or(self.id, |(_, t)| t)
    }

    pub fn is_secret(&self) -> bool {
        matches!(self.kind, FieldKind::Password)
    }
}

/// Everything the UI and engine need to know about the installer.
pub struct Settings {
    pub scope: Scope,
    /// The installation graph contains machine-wide operations; the
    /// installer must run as administrator/root.
    pub requires_elevation: bool,
    /// Folder name below the scope's base directory.
    pub folder: &'static str,
    pub allow_change_location: bool,
    pub languages: &'static [Language],
    pub fallback_language: Language,
    pub language_selector: bool,
    pub policy: Policy,
    pub integration: Integration,
    pub fields: &'static [Field],
    /// Uncompressed size of the application, for disk-space checks and the
    /// Installed Apps entry.
    pub installed_size: u64,
    /// UI template id and accent color (`0xRRGGBB`).
    pub template: &'static str,
    pub accent: u32,
}

/// A step shown in the progress list.
#[derive(Clone, Copy, Debug)]
pub struct StepInfo {
    pub id: &'static str,
    pub label: Msg,
    /// Relative share of the overall progress bar.
    pub weight: u32,
}

pub type InstallFn = fn(&mut Install<'_>) -> Result<(), InstallError>;
pub type UninstallFn = fn(&mut Uninstall<'_>) -> Result<(), InstallError>;

/// The complete generated installer.
pub struct App {
    pub product: &'static Product,
    pub settings: &'static Settings,
    pub steps: &'static [StepInfo],
    /// The installation graph, generated as straight-line Rust.
    pub install: InstallFn,
    /// `before-uninstall` actions.
    pub before_uninstall: UninstallFn,
    /// `after-uninstall` actions.
    pub after_uninstall: UninstallFn,
    /// Prerequisites resolved at build time.
    pub prerequisites: &'static [crate::prereq::Prerequisite],
}

/// Uninstall hook that does nothing (no uninstall actions configured).
pub fn no_uninstall_hook(_: &mut Uninstall<'_>) -> Result<(), InstallError> {
    Ok(())
}
