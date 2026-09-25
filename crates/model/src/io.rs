//! Project file (de)serialization and schema migrations.
//!
//! Project files are TOML with a mandatory `schema` field. Loading parses to
//! a generic table first, migrates it step by step to [`SCHEMA_VERSION`],
//! then deserializes the typed model. Newer schemas are rejected instead of
//! being silently truncated.

use std::fmt;

use crate::project::{Project, SCHEMA_VERSION};

#[derive(Debug)]
pub enum ProjectFileError {
    Parse(String),
    MissingSchema,
    TooNew(u32),
    Migration { from: u32, message: String },
    Serialize(String),
}

impl fmt::Display for ProjectFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectFileError::Parse(e) => write!(f, "invalid project file: {e}"),
            ProjectFileError::MissingSchema => f.write_str("project file has no `schema` version"),
            ProjectFileError::TooNew(v) => write!(
                f,
                "project schema {v} is newer than this version supports ({SCHEMA_VERSION}); update the Studio"
            ),
            ProjectFileError::Migration { from, message } => {
                write!(f, "cannot migrate project from schema {from}: {message}")
            }
            ProjectFileError::Serialize(e) => write!(f, "cannot serialize project: {e}"),
        }
    }
}

impl std::error::Error for ProjectFileError {}

/// One migration step from schema `N` to `N + 1`.
type Migration = fn(&mut toml::Table) -> Result<(), String>;

/// `MIGRATIONS[i]` migrates schema `i` to `i + 1`. Schema 0 was the
/// pre-release format that stored `product.version` as an integer array.
const MIGRATIONS: &[Migration] = &[migrate_0_to_1];

fn migrate_0_to_1(t: &mut toml::Table) -> Result<(), String> {
    if let Some(toml::Value::Table(product)) = t.get_mut("product")
        && let Some(toml::Value::Array(parts)) = product.get("version")
    {
        let mut s = String::new();
        for (i, p) in parts.iter().enumerate() {
            let n = p.as_integer().ok_or("version parts must be integers")?;
            if i > 0 {
                s.push('.');
            }
            s.push_str(&n.to_string());
        }
        product.insert("version".into(), toml::Value::String(s));
    }
    Ok(())
}

pub fn from_toml(text: &str) -> Result<Project, ProjectFileError> {
    let mut table: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| ProjectFileError::Parse(e.to_string()))?;
    let schema = table
        .get("schema")
        .and_then(toml::Value::as_integer)
        .ok_or(ProjectFileError::MissingSchema)?;
    let mut schema = u32::try_from(schema).map_err(|_| ProjectFileError::MissingSchema)?;
    if schema > SCHEMA_VERSION {
        return Err(ProjectFileError::TooNew(schema));
    }
    while schema < SCHEMA_VERSION {
        let step = MIGRATIONS[schema as usize];
        step(&mut table).map_err(|message| ProjectFileError::Migration {
            from: schema,
            message,
        })?;
        schema += 1;
        table.insert("schema".into(), toml::Value::Integer(i64::from(schema)));
    }
    Project::deserialize_table(table)
}

impl Project {
    fn deserialize_table(table: toml::Table) -> Result<Project, ProjectFileError> {
        use serde::Deserialize;
        Project::deserialize(table).map_err(|e| ProjectFileError::Parse(e.to_string()))
    }
}

pub fn to_toml(project: &Project) -> Result<String, ProjectFileError> {
    toml::to_string_pretty(project).map_err(|e| ProjectFileError::Serialize(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::Version;

    #[test]
    fn roundtrip() {
        let mut p = Project::new(
            "Acme Orders",
            "Acme",
            Version::parse("1.4.0").expect("v"),
            "../dist".into(),
        );
        p.application.main_executable = Some("AcmeOrders.exe".into());
        p.product
            .description
            .set(inst_i18n::Language::Tr, "Sipariş yönetimi".into());
        let text = to_toml(&p).expect("ser");
        assert_eq!(from_toml(&text).expect("de"), p, "{text}");
    }

    #[test]
    fn migrates_schema_zero() {
        let text = r#"
schema = 0
[product]
id = "com.acme.app"
name = "App"
publisher = "Acme"
version = [1, 2, 3]
[application]
source = "dist"
"#;
        let p = from_toml(text).expect("migrated");
        assert_eq!(p.schema, SCHEMA_VERSION);
        assert_eq!(p.product.version.to_string(), "1.2.3");
        // Omitted sections take their policy defaults.
        assert!(p.policy.rollback);
        assert!(!p.integration.desktop_shortcut);
    }

    #[test]
    fn rejects_newer_schema() {
        assert!(matches!(
            from_toml("schema = 999"),
            Err(ProjectFileError::TooNew(999))
        ));
        assert!(matches!(
            from_toml("x = 1"),
            Err(ProjectFileError::MissingSchema)
        ));
    }
}
