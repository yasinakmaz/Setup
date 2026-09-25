//! Structural validation: problems that make a build impossible or unsafe.
//!
//! The Doctor builds on these diagnostics and adds recommendations; this
//! module only reports facts about the model itself.

use std::collections::HashSet;
use std::fmt;

use crate::action::{ActionKind, ServiceScope};
use crate::graph::GraphIssue;
use crate::ids::validate_local_id;
use crate::platform::Os;
use crate::privileges::{self, PrerequisiteFacts};
use crate::project::{Acquisition, InputKind, Project};
use crate::value::{SecretRef, Value, ValueRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable code, e.g. `M0101`.
    pub code: &'static str,
    /// Dotted path of the offending field, e.g. `actions[2].url`.
    pub location: String,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "{sev}[{}] {}: {}", self.code, self.location, self.message)
    }
}

struct Sink(Vec<Diagnostic>);

impl Sink {
    fn error(&mut self, code: &'static str, location: impl Into<String>, message: impl Into<String>) {
        self.0.push(Diagnostic {
            severity: Severity::Error,
            code,
            location: location.into(),
            message: message.into(),
        });
    }
    fn warn(&mut self, code: &'static str, location: impl Into<String>, message: impl Into<String>) {
        self.0.push(Diagnostic {
            severity: Severity::Warning,
            code,
            location: location.into(),
            message: message.into(),
        });
    }
}

pub fn validate(project: &Project, facts: &dyn PrerequisiteFacts) -> Vec<Diagnostic> {
    let mut d = Sink(Vec::new());
    let p = project;

    if p.schema != crate::project::SCHEMA_VERSION {
        d.error("M0001", "schema", format!("unsupported schema version {}", p.schema));
    }
    if p.product.name.trim().is_empty() {
        d.error("M0101", "product.name", "product name is empty");
    }
    if p.product.publisher.trim().is_empty() {
        d.warn("M0102", "product.publisher", "publisher is empty; Installed Apps will show no publisher");
    }
    let targets = p.enabled_targets();
    if targets.is_empty() {
        d.error("M0103", "targets", "no output target is enabled");
    }
    if targets.iter().any(|t| t.os == Os::Windows) && !p.product.version.fits_windows_file_version() {
        d.error(
            "M0104",
            "product.version",
            "Windows versions must have parts ≤ 65535",
        );
    }
    match &p.application.main_executable {
        None => d.warn(
            "M0201",
            "application.main-executable",
            "no main executable; shortcuts and \"Open application\" are disabled",
        ),
        Some(exe) => {
            if let Err(e) = inst_fsx::relpath::validate(exe) {
                d.error("M0202", "application.main-executable", format!("invalid path: {e}"));
            }
        }
    }

    // Languages.
    if p.ui.languages.is_empty() {
        d.error("M0301", "ui.languages", "at least one language must be enabled");
    } else if !p.ui.languages.contains(&p.ui.fallback_language) {
        d.error("M0302", "ui.fallback-language", "fallback language is not enabled");
    }

    // Input fields.
    let mut field_ids = HashSet::new();
    for (i, f) in p.ui.fields.iter().enumerate() {
        if validate_local_id(&f.id).is_err() {
            d.error("M0401", format!("ui.fields[{i}].id"), format!("invalid field id {:?}", f.id));
        }
        if !field_ids.insert(f.id.as_str()) {
            d.error("M0402", format!("ui.fields[{i}].id"), format!("duplicate field id {:?}", f.id));
        }
    }
    let field_kind = |id: &str| p.ui.fields.iter().find(|f| f.id == id).map(|f| &f.kind);
    let check_value = |d: &mut Sink, loc: &str, v: &Value| match v {
        Value::Ref(ValueRef::Input { field }) => match field_kind(field) {
            None => d.error("M0403", loc, format!("unknown input field {field:?}")),
            Some(InputKind::Password) => d.error(
                "M0404",
                loc,
                format!("password field {field:?} must be referenced as a secret"),
            ),
            Some(_) => {}
        },
        Value::Ref(ValueRef::Secret {
            secret: SecretRef::Input { field },
        }) => match field_kind(field) {
            None => d.error("M0403", loc, format!("unknown input field {field:?}")),
            Some(InputKind::Password) => {}
            Some(_) => d.warn(
                "M0405",
                loc,
                format!("secret {field:?} comes from a non-password field and will be visible while typing"),
            ),
        },
        _ => {}
    };

    // Actions.
    let mut action_ids = HashSet::new();
    for (i, a) in p.actions.iter().enumerate() {
        let loc = format!("actions[{i}]");
        if validate_local_id(&a.id).is_err() {
            d.error("M0501", format!("{loc}.id"), format!("invalid action id {:?}", a.id));
        }
        if !action_ids.insert(a.id.as_str()) {
            d.error("M0502", format!("{loc}.id"), format!("duplicate action id {:?}", a.id));
        }
        match &a.kind {
            ActionKind::Script(s) => {
                for v in s.args.iter().chain(s.env.values()) {
                    check_value(&mut d, &loc, v);
                }
                for name in s.env.keys() {
                    if name.is_empty() || name.contains(['=', '\0']) {
                        d.error("M0503", format!("{loc}.env"), format!("invalid variable name {name:?}"));
                    }
                }
                if s.allowed_exit_codes.is_empty() {
                    d.error("M0504", format!("{loc}.allowed-exit-codes"), "no exit code is accepted");
                }
            }
            ActionKind::Http(h) => {
                if h.url.starts_with("http://") {
                    d.warn("M0510", format!("{loc}.url"), "plain HTTP is not encrypted; use https://");
                } else if !h.url.starts_with("https://") {
                    d.error("M0511", format!("{loc}.url"), "URL must start with https:// or http://");
                }
                for (_, v) in h.headers.iter().chain(&h.query) {
                    check_value(&mut d, &loc, v);
                }
            }
            ActionKind::Database(db) => {
                for v in [&db.connection.host, &db.connection.user, &db.connection.database]
                    .into_iter()
                    .flatten()
                {
                    check_value(&mut d, &loc, v);
                }
                if db.provider == crate::action::DbProvider::Sqlite && db.connection.file.is_none() {
                    d.error("M0520", format!("{loc}.connection.file"), "SQLite needs a database file");
                }
            }
            ActionKind::Service(s) => {
                if s.name.is_empty() || s.name.contains(char::is_whitespace) {
                    d.error("M0530", format!("{loc}.name"), "service names must not be empty or contain spaces");
                }
                if let Err(e) = inst_fsx::relpath::validate(&s.executable) {
                    d.error("M0531", format!("{loc}.executable"), format!("invalid path: {e}"));
                }
                if s.scope == ServiceScope::User && targets.iter().any(|t| t.os == Os::Windows) {
                    let windows_applies = a
                        .condition
                        .as_ref()
                        .and_then(|c| c.resolve_static(Os::Windows, crate::platform::Arch::X64))
                        .unwrap_or(true);
                    if windows_applies {
                        d.error(
                            "M0532",
                            format!("{loc}.scope"),
                            "user services are not available on Windows; add an OS condition or use system scope",
                        );
                    }
                }
            }
            ActionKind::Run(r) => {
                if let Err(e) = inst_fsx::relpath::validate(&r.program) {
                    d.error("M0540", format!("{loc}.program"), format!("invalid path: {e}"));
                }
            }
            ActionKind::Registry(_) | ActionKind::Environment(_) => {}
        }
    }

    // Prerequisites.
    let mut prereq_ids = HashSet::new();
    for (i, pr) in p.prerequisites.iter().enumerate() {
        let loc = format!("prerequisites[{i}]");
        if !prereq_ids.insert(pr.id.as_str()) {
            d.error("M0601", loc.clone(), format!("prerequisite {:?} listed twice", pr.id));
        }
        if pr.acquisition == Acquisition::Custom {
            if pr.custom_sources.is_empty() {
                d.error("M0602", loc.clone(), "custom acquisition needs at least one source");
            }
            for (j, s) in pr.custom_sources.iter().enumerate() {
                if !s.url.starts_with("https://") {
                    d.error("M0603", format!("{loc}.custom-sources[{j}]"), "custom sources must use https://");
                }
                if !(s.hash.starts_with("sha256:") || s.hash.starts_with("blake3:")) {
                    d.error(
                        "M0604",
                        format!("{loc}.custom-sources[{j}].hash"),
                        "hash must be `sha256:<hex>` or `blake3:<hex>`; unverified binaries are never trusted",
                    );
                }
            }
        }
    }

    // Graph.
    let graph = p.effective_graph();
    for issue in graph.validate(p) {
        let (severity, code) = if issue.is_warning() {
            (Severity::Warning, "M0701")
        } else {
            (Severity::Error, "M0700")
        };
        let message = issue.to_string();
        d.0.push(Diagnostic {
            severity,
            code,
            location: "graph".into(),
            message,
        });
        if matches!(issue, GraphIssue::Cycle(_)) {
            break;
        }
    }

    // Privileges.
    for t in &targets {
        let analysis = privileges::analyze(p, *t, facts);
        if analysis.conflict {
            let reasons: Vec<String> = analysis.reasons.iter().map(ToString::to_string).collect();
            d.error(
                "M0801",
                "install.privileges",
                format!(
                    "'Current user' was forced but {t} needs administrator rights: {}",
                    reasons.join("; ")
                ),
            );
        }
    }
    d.0
}

pub fn has_errors(diags: &[Diagnostic]) -> bool {
    diags.iter().any(|d| d.severity == Severity::Error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::Version;
    use crate::privileges::NoFacts;

    #[test]
    fn new_project_only_warns_about_missing_executable() {
        let p = Project::new("App", "Acme", Version::parse("1.0").expect("v"), "d".into());
        let diags = validate(&p, &NoFacts);
        assert!(!has_errors(&diags), "{diags:?}");
        assert!(diags.iter().any(|d| d.code == "M0201"));
    }

    #[test]
    fn reports_bad_fields() {
        let mut p = Project::new("", "", Version::parse("70000.0").expect("v"), "d".into());
        p.ui.languages.clear();
        p.application.main_executable = Some("../x.exe".into());
        let codes: Vec<_> = validate(&p, &NoFacts).into_iter().map(|d| d.code).collect();
        for code in ["M0101", "M0102", "M0104", "M0202", "M0301"] {
            assert!(codes.contains(&code), "{code} missing from {codes:?}");
        }
    }
}
