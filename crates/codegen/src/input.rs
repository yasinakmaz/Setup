//! Everything code generation needs, resolved by the build system.

use std::path::PathBuf;

use inst_model::platform::Target;
use inst_model::privileges::PrivilegeAnalysis;
use inst_model::project::Project;

/// Optimization profile for the generated installer. Which one is best is
/// measured by the build system, not assumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OptProfile {
    /// `opt-level = "z"`.
    Smallest,
    /// `opt-level = "s"`.
    Small,
    /// `opt-level = 3`.
    Fast,
}

impl OptProfile {
    pub const fn opt_level(self) -> &'static str {
        match self {
            OptProfile::Smallest => "\"z\"",
            OptProfile::Small => "\"s\"",
            OptProfile::Fast => "3",
        }
    }
}

/// Detection strategy of a resolved prerequisite (mirror of the catalog's).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrereqDetection {
    Registry {
        key: String,
        value: String,
        version_value: Option<String>,
        per_user_too: bool,
    },
    DotnetSharedFramework { framework: String },
    Command { program: String, args: Vec<String>, stderr: bool },
    Service { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrereqSource {
    Download { urls: Vec<String>, hash: String, size: u64 },
    /// Path of the package inside the payload.
    Embedded { payload_path: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrereqKind {
    Exe,
    Msi,
}

/// A prerequisite resolved for one target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrereqInput {
    pub id: String,
    pub name: String,
    pub version: String,
    pub min_version: Option<String>,
    pub same_major: bool,
    pub detection: PrereqDetection,
    pub source: PrereqSource,
    pub file_name: String,
    pub kind: PrereqKind,
    /// Catalog argument templates (`{secret:x}`, `{input:x}` allowed).
    pub args: Vec<String>,
    pub success_codes: Vec<i32>,
    pub reboot_codes: Vec<i32>,
    pub already_installed_codes: Vec<i32>,
    pub publisher: Option<String>,
}

/// Paths of the runtime crates the generated installer depends on.
#[derive(Clone, Debug)]
pub struct RuntimeSources {
    /// `crates/runtime`.
    pub runtime: PathBuf,
    /// `crates/runtime-ui` (graphical frontend).
    pub runtime_ui: Option<PathBuf>,
}

pub struct CodegenInput<'a> {
    pub project: &'a Project,
    pub target: Target,
    pub privileges: &'a PrivilegeAnalysis,
    /// Uncompressed application size (disk-space checks, Installed Apps).
    pub installed_size: u64,
    pub prerequisites: &'a [PrereqInput],
    pub runtime: &'a RuntimeSources,
    /// Extra runtime features (codecs chosen by compression analysis).
    pub codec_features: &'a [&'static str],
    pub gui: bool,
    pub profile: OptProfile,
    /// Contents of project script files, by action id.
    pub project_scripts: &'a [(String, String)],
    /// Product logo for the installer UI (PNG).
    pub logo_png: Option<Vec<u8>>,
    /// Windows icon (`.ico`) for the executable resource.
    pub windows_icon: Option<Vec<u8>>,
    /// Main executable's icon inside the install dir, if any.
    pub installed_icon: Option<String>,
}
