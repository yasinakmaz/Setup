//! Target platforms. macOS is intentionally absent from this version.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Os {
    Windows,
    Linux,
}

impl Os {
    pub const ALL: [Os; 2] = [Os::Windows, Os::Linux];

    pub const fn name(self) -> &'static str {
        match self {
            Os::Windows => "Windows",
            Os::Linux => "Linux",
        }
    }

    /// The OS this binary was compiled for, if it is a supported target.
    pub const fn host() -> Option<Os> {
        if cfg!(windows) {
            Some(Os::Windows)
        } else if cfg!(target_os = "linux") {
            Some(Os::Linux)
        } else {
            None
        }
    }
}

/// CPU architecture. Binaries are always built for the generic baseline of
/// the architecture — never `target-cpu=native`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Arch {
    X64,
    Arm64,
}

impl Arch {
    pub const fn host() -> Option<Arch> {
        if cfg!(target_arch = "x86_64") {
            Some(Arch::X64)
        } else if cfg!(target_arch = "aarch64") {
            Some(Arch::Arm64)
        } else {
            None
        }
    }
}

/// An output target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Target {
    pub os: Os,
    pub arch: Arch,
}

impl Target {
    pub const WINDOWS_X64: Target = Target {
        os: Os::Windows,
        arch: Arch::X64,
    };
    pub const LINUX_X64: Target = Target {
        os: Os::Linux,
        arch: Arch::X64,
    };
    pub const WINDOWS_ARM64: Target = Target {
        os: Os::Windows,
        arch: Arch::Arm64,
    };
    pub const LINUX_ARM64: Target = Target {
        os: Os::Linux,
        arch: Arch::Arm64,
    };

    /// Targets supported by this version.
    pub const SUPPORTED: [Target; 4] = [
        Target::WINDOWS_X64,
        Target::LINUX_X64,
        Target::WINDOWS_ARM64,
        Target::LINUX_ARM64,
    ];

    pub fn host() -> Option<Target> {
        Some(Target {
            os: Os::host()?,
            arch: Arch::host()?,
        })
    }

    /// Architecture suffix used in artifact names, following each
    /// platform's convention (`x64` on Windows, `x86_64` for AppImage).
    pub const fn arch_suffix(self) -> &'static str {
        match (self.os, self.arch) {
            (Os::Windows, Arch::X64) => "x64",
            (Os::Windows, Arch::Arm64) => "arm64",
            (Os::Linux, Arch::X64) => "x86_64",
            (Os::Linux, Arch::Arm64) => "aarch64",
        }
    }

    /// `Product-Setup-x64.exe` / `Product-Setup-x86_64.AppImage`.
    pub fn artifact_name(self, product_file_stem: &str) -> String {
        let ext = match self.os {
            Os::Windows => "exe",
            Os::Linux => "AppImage",
        };
        format!("{product_file_stem}-Setup-{}.{ext}", self.arch_suffix())
    }

    /// Rust target triples to try, preferred first. Windows prefers MSVC but
    /// accepts the GNU ABI when cross-compiling from Linux.
    pub const fn rust_triples(self) -> &'static [&'static str] {
        match (self.os, self.arch) {
            (Os::Windows, Arch::X64) => &["x86_64-pc-windows-msvc", "x86_64-pc-windows-gnu"],
            (Os::Windows, Arch::Arm64) => &["aarch64-pc-windows-msvc", "aarch64-pc-windows-gnullvm"],
            (Os::Linux, Arch::X64) => &["x86_64-unknown-linux-gnu"],
            (Os::Linux, Arch::Arm64) => &["aarch64-unknown-linux-gnu"],
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.os.name(), self.arch_suffix())
    }
}

/// Produces a file-name-safe stem from a product name, keeping Unicode
/// letters (file systems handle them) but removing separators and
/// characters invalid on Windows.
pub fn file_stem(product_name: &str) -> String {
    let mut out = String::with_capacity(product_name.len());
    for c in product_name.chars() {
        if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
            out.push(c);
        } else if (c.is_whitespace() || c == '+') && !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches(['-', '.']);
    if trimmed.is_empty() {
        "Product".to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_names() {
        let stem = file_stem("Acme Orders");
        assert_eq!(Target::WINDOWS_X64.artifact_name(&stem), "Acme-Orders-Setup-x64.exe");
        assert_eq!(
            Target::LINUX_X64.artifact_name(&stem),
            "Acme-Orders-Setup-x86_64.AppImage"
        );
        assert_eq!(file_stem("  :/\\ "), "Product");
        assert_eq!(file_stem("Sipariş Yönetimi"), "Sipariş-Yönetimi");
    }
}
