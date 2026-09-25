//! Catalog schema. The built-in catalog and user extensions use the same
//! TOML format (`[[item]]` tables).

use serde::{Deserialize, Serialize};

use inst_model::platform::{Arch, Os};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CatalogFile {
    #[serde(rename = "item", default)]
    pub items: Vec<CatalogItem>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    Runtime,
    Database,
    Tool,
    Service,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerOs {
    #[serde(default)]
    pub windows: bool,
    #[serde(default)]
    pub linux: bool,
}

impl PerOs {
    pub fn get(&self, os: Os) -> bool {
        match os {
            Os::Windows => self.windows,
            Os::Linux => self.linux,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CatalogItem {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub homepage: String,
    pub license: String,
    pub category: Category,
    #[serde(default)]
    pub description: String,
    /// Supported release channels / major versions, newest first.
    pub channels: Vec<String>,
    /// Whether installing it needs administrator/root rights.
    #[serde(default)]
    pub requires_admin: PerOs,
    #[serde(rename = "package", default)]
    pub packages: Vec<Package>,
    /// Shown in the Studio (limitations, licensing notes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl CatalogItem {
    pub fn package(&self, os: Os, arch: Arch) -> Option<&Package> {
        self.packages.iter().find(|p| p.os == os && p.arch == arch)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileKind {
    Exe,
    Msi,
    Zip,
    TarGz,
    TarXz,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Package {
    pub os: Os,
    pub arch: Arch,
    pub file: FileKind,
    pub detection: Detection,
    pub install: InstallSpec,
    #[serde(rename = "source", default)]
    pub sources: Vec<SourceDef>,
    pub integrity: IntegrityStrategy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<SignatureSpec>,
}

/// How an acquisition route relates to the vendor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Route {
    /// Vendor's own download server.
    OfficialDirect,
    /// Vendor-operated CDN or storage account.
    OfficialCdn,
    /// Location obtained from the vendor's release API/metadata.
    OfficialReleaseApi,
    /// Vendor's package repository or release page (e.g. GitHub releases of
    /// the vendor's own organization).
    OfficialRepository,
    /// A mirror operated or officially listed by the vendor.
    OfficialMirror,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SourceDef {
    pub route: Route,
    /// URL template: `{version}`, `{channel}`, `{version_major}` are
    /// substituted at resolution time.
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "kebab-case")]
pub enum Detection {
    /// A registry value exists (optionally with a version in another value).
    Registry {
        key: String,
        #[serde(default)]
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version_value: Option<String>,
        /// Check both HKLM and HKCU.
        #[serde(default)]
        per_user_too: bool,
    },
    /// .NET shared framework directory `dotnet/shared/<framework>/<version>`.
    DotnetSharedFramework { framework: String },
    /// Run a program and read a version from its output.
    Command {
        program: String,
        #[serde(default)]
        args: Vec<String>,
        /// Version is printed on stderr (e.g. `java -version`).
        #[serde(default)]
        stderr: bool,
    },
    /// A Windows service or systemd unit exists.
    Service { name: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallKind {
    /// Run the downloaded executable with `args`.
    Exe,
    /// `msiexec /i <file> /qn /norestart` + `args`.
    Msi,
    /// Extract into a per-user directory (no elevation).
    ExtractUserLocal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct InstallSpec {
    pub kind: InstallKind,
    /// Silent-mode arguments. Tokens: `{secret:<field>}` passes an installer
    /// secret input, `{input:<field>}` a plain input.
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "zero")]
    pub success_codes: Vec<i32>,
    #[serde(default)]
    pub reboot_codes: Vec<i32>,
    /// Exit code meaning "already installed / newer version present".
    #[serde(default)]
    pub already_installed_codes: Vec<i32>,
}

fn zero() -> Vec<i32> {
    vec![0]
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "kebab-case")]
pub enum IntegrityStrategy {
    /// .NET release metadata JSON publishes a SHA-512 per file.
    DotnetReleaseMetadata { metadata_url: String, component: String, file_name: String },
    /// A vendor checksum file (`SHASUMS256.txt` style: `<hex>  <file>`).
    ChecksumFile { url: String, file_name: String },
    /// No vendor checksum: the Studio downloads the file at build time,
    /// verifies the publisher signature where possible, and pins its hash.
    PinAtBuild,
    /// No verifiable integrity metadata. Automatic download is refused;
    /// the package must be embedded or supplied with a custom verified hash.
    Unverifiable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SignatureSpec {
    /// Expected Authenticode signer (subject CN) on Windows.
    pub authenticode_publisher: String,
}
