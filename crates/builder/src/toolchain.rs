//! Rust toolchain and runtime-source discovery.

use std::path::{Path, PathBuf};
use std::process::Command;

use inst_model::platform::{Os, Target};

/// Canonicalizes a path without producing a Windows `\\?\` verbatim path:
/// `std::path::Path::canonicalize` always adds that prefix on Windows, and
/// `cargo` cannot parse a manifest `path = "..."` dependency that contains
/// one ("invalid path url"). `dunce::canonicalize` resolves the same `..`
/// and symlinks but keeps the legacy path form whenever that form can
/// still address the target.
#[cfg(windows)]
fn canonicalize(p: PathBuf) -> PathBuf {
    dunce::canonicalize(&p).unwrap_or(p)
}

#[cfg(not(windows))]
fn canonicalize(p: PathBuf) -> PathBuf {
    p.canonicalize().unwrap_or(p)
}

/// Where the runtime crates (and the lockfile pinning their dependencies)
/// live. A Studio distribution ships them in `runtime-src/`; development
/// builds use the workspace.
#[derive(Clone, Debug)]
pub struct RuntimeLocation {
    pub root: PathBuf,
}

impl RuntimeLocation {
    pub fn discover() -> Option<RuntimeLocation> {
        let candidates = [
            std::env::var_os("INST_RUNTIME_SRC").map(PathBuf::from),
            std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(|p| p.join("runtime-src"))),
            Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")),
        ];
        candidates
            .into_iter()
            .flatten()
            .find(|p| p.join("crates/runtime/Cargo.toml").is_file())
            .map(|root| RuntimeLocation {
                root: canonicalize(root),
            })
    }

    pub fn runtime(&self) -> PathBuf {
        self.root.join("crates/runtime")
    }

    pub fn runtime_ui(&self) -> Option<PathBuf> {
        let p = self.root.join("crates/runtime-ui");
        p.join("Cargo.toml").is_file().then_some(p)
    }

    pub fn lockfile(&self) -> Option<PathBuf> {
        let p = self.root.join("Cargo.lock");
        p.is_file().then_some(p)
    }

    pub fn toolchain_file(&self) -> Option<PathBuf> {
        let p = self.root.join("rust-toolchain.toml");
        p.is_file().then_some(p)
    }
}

#[derive(Clone, Debug)]
pub struct Toolchain {
    pub cargo: PathBuf,
    pub has_rustup: bool,
    pub installed_targets: Vec<String>,
    pub host: String,
}

impl Toolchain {
    /// Probes `cargo`/`rustup`. `dir` is where the generated crates live so
    /// `rust-toolchain.toml` pinning applies.
    pub fn detect(dir: &Path) -> Result<Toolchain, String> {
        let cargo = std::env::var_os("CARGO").map_or_else(|| PathBuf::from("cargo"), PathBuf::from);
        let version = Command::new(&cargo)
            .arg("--version")
            .current_dir(dir)
            .output()
            .map_err(|e| format!("cargo not found ({e}); install Rust from https://rustup.rs"))?;
        if !version.status.success() {
            return Err(format!(
                "cargo failed: {}",
                String::from_utf8_lossy(&version.stderr)
            ));
        }
        let rustc = Command::new("rustc")
            .arg("-vV")
            .current_dir(dir)
            .output()
            .map_err(|e| e.to_string())?;
        let host = String::from_utf8_lossy(&rustc.stdout)
            .lines()
            .find_map(|l| l.strip_prefix("host: ").map(str::to_owned))
            .unwrap_or_default();
        let targets = Command::new("rustup")
            .args(["target", "list", "--installed"])
            .current_dir(dir)
            .output();
        let (has_rustup, installed_targets) = match targets {
            Ok(o) if o.status.success() => (
                true,
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(str::to_owned)
                    .collect(),
            ),
            _ => (false, vec![host.clone()]),
        };
        Ok(Toolchain {
            cargo,
            has_rustup,
            installed_targets,
            host,
        })
    }

    /// Picks the Rust target triple for an output target: MSVC on Windows
    /// hosts, GNU when cross-compiling from Linux.
    pub fn triple_for(&self, target: Target) -> Result<&'static str, String> {
        let triples = target.rust_triples();
        let host_is_windows = self.host.contains("windows");
        let preferred: Vec<&'static str> = if target.os == Os::Windows && !host_is_windows {
            triples.iter().rev().copied().collect()
        } else {
            triples.to_vec()
        };
        preferred
            .iter()
            .copied()
            .find(|t| self.installed_targets.iter().any(|i| i == t))
            .ok_or_else(|| {
                format!(
                    "Rust target for {target} is not installed; run `rustup target add {}`",
                    preferred[0]
                )
            })
    }
}

/// Whether a program can be executed.
pub fn available(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
