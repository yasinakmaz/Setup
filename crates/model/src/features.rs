//! Derivation of the runtime cargo features a generated installer needs.
//!
//! Only what the project uses is compiled: no PostgreSQL support without a
//! PostgreSQL action, no HTTP stack without HTTP actions or downloads, no
//! string tables for disabled languages.

use std::collections::BTreeSet;

use crate::action::{ActionKind, DbProvider};
use crate::platform::{Os, Target};
use crate::project::{Acquisition, Project};

/// A runtime feature with the reason it is included (for the build report
/// and size attribution).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Feature {
    pub name: &'static str,
    pub reason: String,
}

/// Every optional runtime feature, for "✓ / ✕" listings.
pub const OPTIONAL_FEATURES: &[(&str, &str)] = &[
    ("gui", "Graphical installer UI"),
    ("download", "Prerequisite downloads (HTTPS)"),
    ("http", "HTTP actions"),
    ("db-postgres", "PostgreSQL"),
    ("db-mysql", "MySQL / MariaDB"),
    ("db-sqlite", "SQLite"),
    ("db-mssql", "SQL Server"),
    ("scripts", "PowerShell / shell scripts"),
    ("services", "Service installation"),
    ("registry", "Windows registry"),
    ("environment", "Environment variables"),
];

pub fn runtime_features(project: &Project, target: Target, gui: bool) -> Vec<Feature> {
    let mut set: BTreeSet<Feature> = BTreeSet::new();
    let mut add = |name: &'static str, reason: String| {
        if !set.iter().any(|f| f.name == name) {
            set.insert(Feature { name, reason });
        }
    };
    if gui {
        add("gui", "graphical installer".into());
    }
    for lang in &project.ui.languages {
        let name = match lang {
            inst_i18n::Language::En => continue,
            inst_i18n::Language::Tr => "lang-tr",
            inst_i18n::Language::Ar => "lang-ar",
            inst_i18n::Language::Es => "lang-es",
            inst_i18n::Language::Fr => "lang-fr",
            inst_i18n::Language::De => "lang-de",
            inst_i18n::Language::Ru => "lang-ru",
        };
        add(name, format!("language {} enabled", lang.native_name()));
    }
    let applies = |c: &Option<crate::condition::Condition>| {
        c.as_ref()
            .and_then(|c| c.resolve_static(target.os, target.arch))
            .unwrap_or(true)
    };
    for p in project.prerequisites.iter().filter(|p| applies(&p.condition)) {
        if p.acquisition != Acquisition::Embedded {
            add("download", format!("prerequisite '{}' is downloaded when missing", p.id));
        }
    }
    for a in project.actions.iter().filter(|a| a.enabled && applies(&a.condition)) {
        let why = || format!("action '{}'", a.id);
        match &a.kind {
            ActionKind::Database(d) => {
                let name = match d.provider {
                    DbProvider::Postgres => "db-postgres",
                    DbProvider::MySql => "db-mysql",
                    DbProvider::Sqlite => "db-sqlite",
                    DbProvider::MsSql => "db-mssql",
                };
                add(name, why());
            }
            ActionKind::Http(_) => add("http", why()),
            ActionKind::Script(_) => add("scripts", why()),
            ActionKind::Service(_) => add("services", why()),
            ActionKind::Registry(_) if target.os == Os::Windows => add("registry", why()),
            ActionKind::Environment(_) => add("environment", why()),
            ActionKind::Registry(_) | ActionKind::Run(_) => {}
        }
    }
    let integ = &project.integration;
    if integ.add_to_path {
        add("environment", "PATH modification enabled".into());
    }
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::Version;

    #[test]
    fn minimal_project_needs_only_gui_and_languages() {
        let mut p = Project::new("App", "Acme", Version::parse("1.0").expect("v"), "d".into());
        p.ui.languages = vec![inst_i18n::Language::En, inst_i18n::Language::Ar];
        let names: Vec<_> = runtime_features(&p, Target::LINUX_X64, true)
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(names, ["gui", "lang-ar"]);
    }
}
