//! Typed values resolved at install time.
//!
//! There is no string templating language: a value is either a literal or a
//! typed reference (an installer input field, a secret, a known folder, a
//! captured HTTP response). Code generation turns each variant into a direct
//! Rust expression.

use serde::{Deserialize, Serialize};

/// Well-known folders, resolved per platform and install scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KnownFolder {
    /// The chosen installation directory.
    Install,
    /// User home directory.
    Home,
    /// Per-user application data (`%APPDATA%`, `$XDG_CONFIG_HOME`).
    UserConfig,
    /// Per-user local data (`%LOCALAPPDATA%`, `$XDG_DATA_HOME`).
    UserData,
    /// Machine-wide application data (`%ProgramData%`, `/var/lib`).
    MachineData,
    /// Temporary directory of the installer process.
    Temp,
    /// Desktop of the current user.
    Desktop,
}

/// A path relative to a known folder, e.g. `{ base = "install", rel = "config/app.json" }`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PathExpr {
    pub base: KnownFolder,
    /// `/`-separated relative path (validated like payload paths). Empty
    /// means the folder itself.
    #[serde(default)]
    pub rel: String,
}

impl PathExpr {
    pub fn install(rel: impl Into<String>) -> PathExpr {
        PathExpr {
            base: KnownFolder::Install,
            rel: rel.into(),
        }
    }
}

/// Where a secret comes from at install time. Secrets are never stored in
/// plaintext in the project file.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(
    tag = "from",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
pub enum SecretRef {
    /// Entered by the user in an installer input field (password field), or
    /// passed as `--set <field>=…` / environment variable in silent mode.
    Input { field: String },
    /// Read from an environment variable of the installer process.
    Env { var: String },
}

/// A value used as an argument, header, connection parameter, etc.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Literal(String),
    Ref(ValueRef),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(
    tag = "ref",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
pub enum ValueRef {
    /// Value of a non-secret installer input field.
    Input {
        field: String,
    },
    Secret {
        #[serde(flatten)]
        secret: SecretRef,
    },
    Path {
        #[serde(flatten)]
        path: PathExpr,
    },
    ProductVersion,
    ProductName,
    /// A value captured by an earlier HTTP action.
    Captured {
        name: String,
    },
}

impl Value {
    pub fn literal(s: impl Into<String>) -> Value {
        Value::Literal(s.into())
    }

    /// Whether the value is (or may contain) secret material.
    pub fn is_secret(&self) -> bool {
        matches!(self, Value::Ref(ValueRef::Secret { .. }))
    }

    /// Input fields this value depends on.
    pub fn input_field(&self) -> Option<&str> {
        match self {
            Value::Ref(ValueRef::Input { field })
            | Value::Ref(ValueRef::Secret {
                secret: SecretRef::Input { field },
            }) => Some(field),
            _ => None,
        }
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::literal(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Holder {
        v: Vec<Value>,
    }

    #[test]
    fn values_roundtrip_through_toml() {
        let h = Holder {
            v: vec![
                Value::literal("--port"),
                Value::Ref(ValueRef::Input {
                    field: "port".into(),
                }),
                Value::Ref(ValueRef::Secret {
                    secret: SecretRef::Input {
                        field: "db_password".into(),
                    },
                }),
                Value::Ref(ValueRef::Path {
                    path: PathExpr::install("bin/app"),
                }),
                Value::Ref(ValueRef::ProductVersion),
            ],
        };
        let text = toml::to_string(&h).expect("serialize");
        let back: Holder = toml::from_str(&text).expect("deserialize");
        assert_eq!(back, h, "{text}");
        assert!(back.v[2].is_secret());
        assert_eq!(back.v[1].input_field(), Some("port"));
    }
}
