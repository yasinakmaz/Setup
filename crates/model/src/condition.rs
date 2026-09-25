//! Visual conditions.
//!
//! Conditions are a small typed tree edited visually in the Studio. The
//! generated installer evaluates them as plain Rust boolean expressions; no
//! expression language exists at runtime.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::platform::{Arch, Os};
use crate::value::PathExpr;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(
    tag = "if",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
pub enum Condition {
    All {
        all: Vec<Condition>,
    },
    Any {
        any: Vec<Condition>,
    },
    Not {
        not: Box<Condition>,
    },
    Os {
        os: Os,
    },
    Arch {
        arch: Arch,
    },
    /// Windows build number at least `build` (e.g. 19041 for 20H1).
    WindowsBuildAtLeast {
        build: u32,
    },
    /// The prerequisite with this catalog id is not installed (or too old).
    PrerequisiteMissing {
        prerequisite: String,
    },
    PrerequisiteInstalled {
        prerequisite: String,
    },
    FileExists {
        path: PathExpr,
    },
    EnvVarSet {
        name: String,
    },
    EnvVarEquals {
        name: String,
        value: String,
    },
    /// An executable with this name is on `PATH`.
    CommandAvailable {
        name: String,
    },
    /// The installer runs with administrator/root rights.
    Elevated,
    /// No previous version is installed.
    FreshInstall,
    /// An older version is installed.
    Upgrade,
    /// The same version is installed and is being repaired.
    Repair,
    /// A checkbox/radio option of the installer UI is selected.
    OptionSelected {
        option: String,
    },
    /// A value captured by an HTTP action equals `value`.
    CapturedEquals {
        name: String,
        value: String,
    },
}

impl Condition {
    pub fn all(items: impl IntoIterator<Item = Condition>) -> Condition {
        Condition::All {
            all: items.into_iter().collect(),
        }
    }

    pub fn any(items: impl IntoIterator<Item = Condition>) -> Condition {
        Condition::Any {
            any: items.into_iter().collect(),
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn not(c: Condition) -> Condition {
        Condition::Not { not: Box::new(c) }
    }

    /// Visits every node of the tree, parents first.
    pub fn walk<'a>(&'a self, f: &mut dyn FnMut(&'a Condition)) {
        f(self);
        match self {
            Condition::All { all: items } | Condition::Any { any: items } => {
                for c in items {
                    c.walk(f);
                }
            }
            Condition::Not { not } => not.walk(f),
            _ => {}
        }
    }

    /// Statically simplifies the condition for a known OS/architecture, so
    /// platform branches that can never run are not compiled into an
    /// installer. Returns `Some(bool)` when the result is fully determined.
    pub fn resolve_static(&self, os: Os, arch: Arch) -> Option<bool> {
        match self {
            Condition::Os { os: o } => Some(*o == os),
            Condition::Arch { arch: a } => Some(*a == arch),
            Condition::WindowsBuildAtLeast { .. } if os != Os::Windows => Some(false),
            Condition::All { all } => {
                let mut unknown = false;
                for c in all {
                    match c.resolve_static(os, arch) {
                        Some(false) => return Some(false),
                        Some(true) => {}
                        None => unknown = true,
                    }
                }
                if unknown { None } else { Some(true) }
            }
            Condition::Any { any } => {
                let mut unknown = false;
                for c in any {
                    match c.resolve_static(os, arch) {
                        Some(true) => return Some(true),
                        Some(false) => {}
                        None => unknown = true,
                    }
                }
                if unknown { None } else { Some(false) }
            }
            Condition::Not { not } => not.resolve_static(os, arch).map(|b| !b),
            _ => None,
        }
    }
}

/// Human-readable rendering used by the Studio (`IF Windows AND x64 AND …`).
impl fmt::Display for Condition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn join(f: &mut fmt::Formatter<'_>, items: &[Condition], sep: &str) -> fmt::Result {
            if items.is_empty() {
                return f.write_str(if sep == " AND " { "always" } else { "never" });
            }
            for (i, c) in items.iter().enumerate() {
                if i > 0 {
                    f.write_str(sep)?;
                }
                match c {
                    Condition::All { .. } | Condition::Any { .. } => write!(f, "({c})")?,
                    _ => write!(f, "{c}")?,
                }
            }
            Ok(())
        }
        match self {
            Condition::All { all } => join(f, all, " AND "),
            Condition::Any { any } => join(f, any, " OR "),
            Condition::Not { not } => write!(f, "NOT ({not})"),
            Condition::Os { os } => f.write_str(os.name()),
            Condition::Arch { arch } => f.write_str(match arch {
                Arch::X64 => "x64",
                Arch::Arm64 => "ARM64",
            }),
            Condition::WindowsBuildAtLeast { build } => write!(f, "Windows build ≥ {build}"),
            Condition::PrerequisiteMissing { prerequisite } => {
                write!(f, "{prerequisite} not installed")
            }
            Condition::PrerequisiteInstalled { prerequisite } => {
                write!(f, "{prerequisite} installed")
            }
            Condition::FileExists { path } => write!(f, "file {:?}/{} exists", path.base, path.rel),
            Condition::EnvVarSet { name } => write!(f, "${name} is set"),
            Condition::EnvVarEquals { name, value } => write!(f, "${name} = {value:?}"),
            Condition::CommandAvailable { name } => write!(f, "`{name}` available"),
            Condition::Elevated => f.write_str("running elevated"),
            Condition::FreshInstall => f.write_str("fresh install"),
            Condition::Upgrade => f.write_str("upgrade"),
            Condition::Repair => f.write_str("repair"),
            Condition::OptionSelected { option } => write!(f, "option '{option}' selected"),
            Condition::CapturedEquals { name, value } => write!(f, "{name} = {value:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Condition {
        Condition::all([
            Condition::Os { os: Os::Windows },
            Condition::Arch { arch: Arch::X64 },
            Condition::PrerequisiteMissing {
                prerequisite: "postgresql".into(),
            },
        ])
    }

    #[test]
    fn renders_like_the_designer() {
        assert_eq!(
            example().to_string(),
            "Windows AND x64 AND postgresql not installed"
        );
    }

    #[test]
    fn static_resolution_prunes_platform_branches() {
        let c = example();
        assert_eq!(c.resolve_static(Os::Linux, Arch::X64), Some(false));
        assert_eq!(c.resolve_static(Os::Windows, Arch::X64), None);
        let only_os = Condition::Os { os: Os::Linux };
        assert_eq!(only_os.resolve_static(Os::Linux, Arch::Arm64), Some(true));
        assert_eq!(
            Condition::not(only_os).resolve_static(Os::Linux, Arch::Arm64),
            Some(false)
        );
    }

    #[test]
    fn toml_roundtrip() {
        #[derive(Serialize, Deserialize)]
        struct H {
            when: Condition,
        }
        let text = toml::to_string(&H { when: example() }).expect("ser");
        let back: H = toml::from_str(&text).expect("de");
        assert_eq!(back.when, example(), "{text}");
    }
}
