//! The project: a single typed document shared by Simple and Advanced mode.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use inst_i18n::Language;

use crate::action::{Action, LifecycleEvent};
use crate::condition::Condition;
use crate::graph::InstallGraph;
use crate::ids::{ProductId, Version, VersionReq};
use crate::platform::Target;
use crate::policy::{self, Policy};
use crate::text::LocalizedText;

/// Current project schema version. Increment on incompatible changes and add
/// a migration in [`crate::migrate`].
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Project {
    pub schema: u32,
    pub product: Product,
    pub application: Application,
    #[serde(default)]
    pub install: InstallSettings,
    #[serde(default)]
    pub integration: Integration,
    #[serde(default)]
    pub policy: Policy,
    #[serde(default)]
    pub targets: Targets,
    #[serde(default)]
    pub compression: CompressionSettings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prerequisites: Vec<PrerequisiteRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<Component>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
    /// Custom installation graph (Advanced mode). `None` uses the graph
    /// derived from the rest of the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<InstallGraph>,
    #[serde(default)]
    pub ui: UiSettings,
    #[serde(default)]
    pub signing: SigningSettings,
    #[serde(default)]
    pub update: UpdateSettings,
    #[serde(default)]
    pub studio: StudioSettings,
}

impl Project {
    /// A new project with safe defaults for an application in `source`.
    pub fn new(name: &str, publisher: &str, version: Version, source: PathBuf) -> Project {
        Project {
            schema: SCHEMA_VERSION,
            product: Product {
                id: ProductId::suggest(publisher, name),
                name: name.to_owned(),
                publisher: publisher.to_owned(),
                version,
                description: LocalizedText::Empty,
                homepage: None,
                support_url: None,
                icon: None,
                license_file: None,
            },
            application: Application {
                source,
                main_executable: None,
                exclude: default_excludes(),
                arguments: Vec::new(),
            },
            install: InstallSettings::default(),
            integration: Integration::default(),
            policy: Policy::default(),
            targets: Targets::default(),
            compression: CompressionSettings::default(),
            prerequisites: Vec::new(),
            components: Vec::new(),
            actions: Vec::new(),
            graph: None,
            ui: UiSettings::default(),
            signing: SigningSettings::default(),
            update: UpdateSettings::default(),
            studio: StudioSettings::default(),
        }
    }

    /// The effective installation graph: the custom one, or the default one
    /// derived from the project.
    pub fn effective_graph(&self) -> InstallGraph {
        self.graph
            .clone()
            .unwrap_or_else(|| InstallGraph::default_for(self))
    }

    /// Enabled actions attached to `event`, in declaration order.
    pub fn actions_for(&self, event: LifecycleEvent) -> impl Iterator<Item = &Action> {
        self.actions
            .iter()
            .filter(move |a| a.enabled && a.event == Some(event))
    }

    pub fn action(&self, id: &str) -> Option<&Action> {
        self.actions.iter().find(|a| a.id == id)
    }

    /// Resolves `application.source` relative to the project file location.
    pub fn source_dir(&self, project_file: &Path) -> PathBuf {
        if self.application.source.is_absolute() {
            self.application.source.clone()
        } else {
            project_file
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(&self.application.source)
        }
    }

    pub fn enabled_targets(&self) -> Vec<Target> {
        self.targets.enabled()
    }
}

fn default_excludes() -> Vec<String> {
    ["*.pdb", "*.ilk", "*.tmp", "*.log", "Thumbs.db", ".DS_Store", ".git/"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Product {
    pub id: ProductId,
    pub name: String,
    pub publisher: String,
    pub version: Version,
    #[serde(default, skip_serializing_if = "LocalizedText::is_empty")]
    pub description: LocalizedText,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support_url: Option<String>,
    /// Icon file relative to the project (`.ico`, `.png` or `.svg`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_file: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Application {
    /// Application directory, relative to the project file.
    pub source: PathBuf,
    /// Main executable relative to `source` (`/`-separated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_executable: Option<String>,
    /// Glob patterns excluded from the payload (`*.pdb`, `logs/`).
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Arguments used by shortcuts and "Open application".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<String>,
}

/// Privilege requirement of the installer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Privileges {
    /// Derived from the installation graph (see [`crate::privileges`]).
    #[default]
    Auto,
    CurrentUser,
    Administrator,
}

/// Base of the installation directory.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallBase {
    /// Per-user unless elevation is required for another reason.
    #[default]
    Auto,
    /// `%LOCALAPPDATA%\Programs` / `$XDG_DATA_HOME`.
    PerUser,
    /// `%ProgramFiles%` / `/opt`.
    PerMachine,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct InstallLocation {
    #[serde(default)]
    pub base: InstallBase,
    /// Folder below the base; defaults to the product name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

impl Default for InstallLocation {
    fn default() -> Self {
        InstallLocation {
            base: InstallBase::Auto,
            folder: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct InstallSettings {
    pub privileges: Privileges,
    pub location: InstallLocation,
    pub allow_change_location: bool,
}

impl Default for InstallSettings {
    fn default() -> Self {
        InstallSettings {
            privileges: Privileges::Auto,
            location: InstallLocation::default(),
            allow_change_location: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct FileAssociation {
    /// Extension without the dot.
    pub extension: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ProtocolAssociation {
    pub scheme: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct FirewallRule {
    pub name: String,
    pub port: u16,
    pub protocol: String,
}

/// Desktop integration. Defaults come from [`policy`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Integration {
    pub start_menu: bool,
    pub desktop_shortcut: bool,
    pub launch_at_startup: bool,
    pub add_to_path: bool,
    pub file_associations: Vec<FileAssociation>,
    pub protocols: Vec<ProtocolAssociation>,
    pub firewall_rules: Vec<FirewallRule>,
    /// Offer "Open application" on the finish page.
    pub offer_launch: bool,
}

impl Default for Integration {
    fn default() -> Self {
        Integration {
            start_menu: policy::on::START_MENU_ENTRY,
            desktop_shortcut: policy::off::DESKTOP_SHORTCUT,
            launch_at_startup: policy::off::LAUNCH_AT_STARTUP,
            add_to_path: policy::off::PATH_MODIFICATION,
            file_associations: Vec::new(),
            protocols: Vec::new(),
            firewall_rules: Vec::new(),
            offer_launch: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Targets {
    pub windows_x64: bool,
    pub linux_x64: bool,
    pub windows_arm64: bool,
    pub linux_arm64: bool,
}

impl Default for Targets {
    fn default() -> Self {
        Targets {
            windows_x64: true,
            linux_x64: true,
            windows_arm64: false,
            linux_arm64: false,
        }
    }
}

impl Targets {
    pub fn enabled(&self) -> Vec<Target> {
        let mut out = Vec::with_capacity(4);
        if self.windows_x64 {
            out.push(Target::WINDOWS_X64);
        }
        if self.linux_x64 {
            out.push(Target::LINUX_X64);
        }
        if self.windows_arm64 {
            out.push(Target::WINDOWS_ARM64);
        }
        if self.linux_arm64 {
            out.push(Target::LINUX_ARM64);
        }
        out
    }

    pub fn set(&mut self, target: Target, enabled: bool) {
        match target {
            Target::WINDOWS_X64 => self.windows_x64 = enabled,
            Target::LINUX_X64 => self.linux_x64 = enabled,
            Target::WINDOWS_ARM64 => self.windows_arm64 = enabled,
            Target::LINUX_ARM64 => self.linux_arm64 = enabled,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompressionProfile {
    /// Benchmark candidates on the payload and pick the best trade-off.
    #[default]
    Auto,
    SmallestSize,
    Balanced,
    FastInstall,
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompressionAccelerator {
    #[default]
    Auto,
    Cpu,
    NvidiaCuda,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct CompressionSettings {
    pub profile: CompressionProfile,
    pub accelerator: CompressionAccelerator,
    /// Target uncompressed size of a block in MiB for grouped-solid
    /// compression; `0` = decided by the profile.
    pub block_size_mib: u32,
}

impl Default for CompressionSettings {
    fn default() -> Self {
        CompressionSettings {
            profile: CompressionProfile::Auto,
            accelerator: CompressionAccelerator::Auto,
            block_size_mib: 0,
        }
    }
}

/// How a prerequisite reaches the target machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Acquisition {
    /// Detect; download from verified sources only when missing.
    #[default]
    Automatic,
    /// Embed the package into the setup.
    Embedded,
    /// Use the developer's own verified sources.
    Custom,
}

/// A developer-supplied download source. Must carry integrity metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CustomSource {
    pub url: String,
    /// Hex BLAKE3 or SHA-256 (`sha256:<hex>`, `blake3:<hex>`).
    pub hash: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PrerequisiteRef {
    /// Catalog id, e.g. `dotnet-desktop-runtime`.
    pub id: String,
    #[serde(default)]
    pub version: VersionReq,
    #[serde(default)]
    pub acquisition: Acquisition,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_sources: Vec<CustomSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Condition>,
    /// Set when the Project Analyzer suggested it (shown with its reason).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_reason: Option<String>,
}

/// An optional part of the application the user can select.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Component {
    pub id: String,
    pub name: LocalizedText,
    #[serde(default, skip_serializing_if = "LocalizedText::is_empty")]
    pub description: LocalizedText,
    /// Glob patterns (relative to the application directory) of the files
    /// that belong to this component.
    pub files: Vec<String>,
    #[serde(default = "yes")]
    pub selected_by_default: bool,
}

fn yes() -> bool {
    true
}

/// Input field on an installer page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct InputField {
    pub id: String,
    pub label: LocalizedText,
    #[serde(flatten)]
    pub kind: InputKind,
    #[serde(default)]
    pub required: bool,
    /// Show on the main page instead of "Advanced options".
    #[serde(default)]
    pub prominent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "kebab-case")]
pub enum InputKind {
    Text {
        #[serde(default)]
        default: String,
    },
    /// Secret: never logged, never written to the manifest.
    Password,
    Checkbox {
        #[serde(default)]
        default: bool,
    },
    Select {
        options: Vec<(String, LocalizedText)>,
        #[serde(default)]
        default: String,
    },
    Number {
        default: i64,
        min: i64,
        max: i64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct UiSettings {
    /// Template id (built-in `modern` by default).
    pub template: String,
    /// Enabled installer languages. Unused ones are not compiled in.
    pub languages: Vec<Language>,
    /// Language used when the system language is not enabled.
    pub fallback_language: Language,
    /// Let the user switch language in the installer.
    pub language_selector: bool,
    pub fields: Vec<InputField>,
    /// Accent color override (`#RRGGBB`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent_color: Option<String>,
    /// Show a license agreement before installing.
    pub show_license: bool,
    /// Per-template properties edited in the Visual Designer.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub template_properties: BTreeMap<String, String>,
}

impl Default for UiSettings {
    fn default() -> Self {
        UiSettings {
            template: "modern".to_owned(),
            languages: Language::ALL.to_vec(),
            fallback_language: Language::En,
            language_selector: true,
            fields: Vec::new(),
            accent_color: None,
            show_license: false,
            template_properties: BTreeMap::new(),
        }
    }
}

/// Where a signing credential comes from. Never plaintext in the project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "kebab-case", rename_all_fields = "kebab-case")]
pub enum CredentialSource {
    /// Environment variable of the build process.
    Env { var: String },
    /// Entry in the OS credential store (Windows Credential Manager, Secret
    /// Service).
    Keychain { entry: String },
    /// Prompt when building interactively.
    Prompt,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "kebab-case", rename_all_fields = "kebab-case")]
pub enum WindowsSigning {
    /// `signtool` with a certificate from the Windows certificate store.
    CertificateStore { thumbprint: String, timestamp_url: String },
    /// `signtool` with a PFX file; the password comes from a credential source.
    PfxFile {
        path: PathBuf,
        password: CredentialSource,
        timestamp_url: String,
    },
    /// An external signing command (cloud HSM, Azure Trusted Signing, …).
    /// `{file}` is passed as a separate argument, never through a shell.
    Command { program: String, args: Vec<String> },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct SigningSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub windows: Option<WindowsSigning>,
    /// Detached signature for Linux artifacts (e.g. GPG key id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linux_key: Option<String>,
    /// Treat unsigned output as an error instead of a warning.
    pub require_signed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct UpdateSettings {
    /// Update feed URL. Without one, "Check for updates" is not offered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feed_url: Option<String>,
    pub check_for_updates_default: bool,
}

/// Studio-only presentation preferences stored with the project.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct StudioSettings {
    pub mode: EditorMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EditorMode {
    #[default]
    Simple,
    Advanced,
}
