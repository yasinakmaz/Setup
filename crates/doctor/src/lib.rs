//! Doctor: project diagnostics with safe fixes.
//!
//! The Doctor builds on the model's structural validation and adds
//! security, compatibility, optimization and recommendation findings.
//! Fixes are offered where possible; only fixes that cannot override an
//! explicit developer decision are "safe" and applied by *Fix All Safe
//! Issues*.

use std::fmt;
use std::path::Path;

use inst_catalog::{Catalog, IntegrityStrategy, Redundancy};
use inst_model::action::{ActionKind, Elevation};
use inst_model::platform::Os;
use inst_model::project::{Acquisition, CompressionProfile, Project};
use inst_model::validate::{self, Severity};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Error,
    Warning,
    Security,
    Compatibility,
    Optimization,
    Recommendation,
}

impl Category {
    pub const fn label(self) -> &'static str {
        match self {
            Category::Error => "Error",
            Category::Warning => "Warning",
            Category::Security => "Security",
            Category::Compatibility => "Compatibility",
            Category::Optimization => "Optimization",
            Category::Recommendation => "Recommendation",
        }
    }

    /// Blocks a build.
    pub const fn is_blocking(self) -> bool {
        matches!(self, Category::Error)
    }
}

/// An automatic change the Doctor can make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fix {
    SetPublisher(String),
    SetMainExecutable(String),
    UseAutoCompression,
    EmbedPrerequisite(String),
    RemovePrerequisite(String),
    EnableFallbackLanguage,
    DisableDesktopShortcut,
}

impl Fix {
    /// Safe fixes never undo an explicit developer choice.
    pub const fn is_safe(&self) -> bool {
        matches!(
            self,
            Fix::SetPublisher(_) | Fix::SetMainExecutable(_) | Fix::EnableFallbackLanguage
        )
    }

    pub fn label(&self) -> String {
        match self {
            Fix::SetPublisher(p) => format!("Set publisher to \"{p}\""),
            Fix::SetMainExecutable(m) => format!("Use {m} as main executable"),
            Fix::UseAutoCompression => "Switch compression to Auto".into(),
            Fix::EmbedPrerequisite(id) => format!("Embed {id} in the setup"),
            Fix::RemovePrerequisite(id) => format!("Remove prerequisite {id}"),
            Fix::EnableFallbackLanguage => "Enable the fallback language".into(),
            Fix::DisableDesktopShortcut => "Do not create a desktop shortcut".into(),
        }
    }

    pub fn apply(&self, p: &mut Project) {
        match self {
            Fix::SetPublisher(v) => p.product.publisher = v.clone(),
            Fix::SetMainExecutable(m) => p.application.main_executable = Some(m.clone()),
            Fix::UseAutoCompression => p.compression.profile = CompressionProfile::Auto,
            Fix::EmbedPrerequisite(id) => {
                if let Some(pr) = p.prerequisites.iter_mut().find(|x| &x.id == id) {
                    pr.acquisition = Acquisition::Embedded;
                }
            }
            Fix::RemovePrerequisite(id) => p.prerequisites.retain(|x| &x.id != id),
            Fix::EnableFallbackLanguage => {
                if !p.ui.languages.contains(&p.ui.fallback_language) {
                    p.ui.languages.insert(0, p.ui.fallback_language);
                }
            }
            Fix::DisableDesktopShortcut => p.integration.desktop_shortcut = false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub category: Category,
    pub code: &'static str,
    /// Where in the project (dotted path) the finding applies.
    pub location: String,
    pub message: String,
    pub fixes: Vec<Fix>,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:<14} [{}] {}: {}",
            self.category.label(),
            self.code,
            self.location,
            self.message
        )?;
        for fix in &self.fixes {
            write!(
                f,
                "\n{:<17}fix: {}{}",
                "",
                fix.label(),
                if fix.is_safe() { " (safe)" } else { "" }
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn errors(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.category == Category::Error)
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.category == Category::Warning)
            .count()
    }

    pub fn safe_fixes(&self) -> Vec<Fix> {
        let mut out: Vec<Fix> = Vec::new();
        for f in &self.findings {
            for fix in f.fixes.iter().filter(|x| x.is_safe()) {
                if !out.contains(fix) {
                    out.push(fix.clone());
                }
            }
        }
        out
    }

    /// Applies every safe fix. Returns how many were applied.
    pub fn fix_all_safe(&self, p: &mut Project) -> usize {
        let fixes = self.safe_fixes();
        for fix in &fixes {
            fix.apply(p);
        }
        fixes.len()
    }
}

struct Sink(Vec<Finding>);

impl Sink {
    fn add(
        &mut self,
        category: Category,
        code: &'static str,
        location: impl Into<String>,
        message: impl Into<String>,
        fixes: Vec<Fix>,
    ) {
        self.0.push(Finding {
            category,
            code,
            location: location.into(),
            message: message.into(),
            fixes,
        });
    }
}

/// Examines a project. `project_file` locates relative paths.
pub fn examine(p: &Project, project_file: &Path, catalog: &Catalog) -> Report {
    let mut s = Sink(Vec::new());

    // Structural validation.
    for d in validate::validate(p, catalog) {
        let category = match d.severity {
            Severity::Error => Category::Error,
            Severity::Warning => match d.code {
                "M0510" | "M0405" => Category::Security,
                _ => Category::Warning,
            },
        };
        let fixes = match d.code {
            "M0302" => vec![Fix::EnableFallbackLanguage],
            _ => Vec::new(),
        };
        s.add(category, d.code, d.location, d.message, fixes);
    }

    let targets = p.enabled_targets();
    let source = p.source_dir(project_file);

    // Application folder and main executable.
    if !source.is_dir() {
        s.add(
            Category::Error,
            "D0101",
            "application.source",
            format!("{} does not exist", source.display()),
            vec![],
        );
    } else if let Some(main) = &p.application.main_executable {
        if !source.join(main).is_file() {
            let fix = inst_analyzer::analyze(&source)
                .ok()
                .and_then(|a| a.main_executable)
                .map(|m| vec![Fix::SetMainExecutable(m.value)])
                .unwrap_or_default();
            s.add(
                Category::Error,
                "D0102",
                "application.main-executable",
                format!("{main} does not exist in the application folder"),
                fix,
            );
        }
    } else if let Ok(a) = inst_analyzer::analyze(&source)
        && let Some(m) = a.main_executable
    {
        s.add(
            Category::Recommendation,
            "D0103",
            "application.main-executable",
            format!(
                "no main executable set; {} looks like it ({})",
                m.value, m.reason
            ),
            vec![Fix::SetMainExecutable(m.value)],
        );
    }

    if p.product.publisher.trim().is_empty()
        && let Ok(a) = inst_analyzer::analyze(&source)
        && !a.publisher.value.is_empty()
    {
        s.add(
            Category::Recommendation,
            "D0104",
            "product.publisher",
            format!(
                "the executable names \"{}\" as publisher",
                a.publisher.value
            ),
            vec![Fix::SetPublisher(a.publisher.value)],
        );
    }
    if p.product.icon.is_none() {
        s.add(
            Category::Recommendation,
            "D0105",
            "product.icon",
            "no icon: shortcuts and the installer use generic icons",
            vec![],
        );
    }
    if p.product.description.is_empty() {
        s.add(
            Category::Recommendation,
            "D0106",
            "product.description",
            "a short description is shown on the install screen",
            vec![],
        );
    }

    // Security.
    if targets.iter().any(|t| t.os == Os::Windows) && p.signing.windows.is_none() {
        s.add(
            Category::Security,
            "D0201",
            "signing.windows",
            "Windows code signing is not configured; SmartScreen will warn users and tampering cannot be detected by Windows",
            vec![],
        );
    }
    for (i, a) in p.actions.iter().enumerate() {
        if let ActionKind::Script(sc) = &a.kind
            && sc.elevation == Elevation::Required
        {
            s.add(
                Category::Security,
                "D0202",
                format!("actions[{i}]"),
                format!(
                    "script '{}' forces the whole installer to run elevated",
                    a.id
                ),
                vec![],
            );
        }
    }

    // Prerequisites.
    for (i, pr) in p.prerequisites.iter().enumerate() {
        let loc = format!("prerequisites[{i}]");
        let Some(item) = catalog.get(&pr.id) else {
            s.add(
                Category::Error,
                "D0301",
                loc,
                format!("{} is not in the catalog", pr.id),
                vec![Fix::RemovePrerequisite(pr.id.clone())],
            );
            continue;
        };
        for t in &targets {
            let applies = pr
                .condition
                .as_ref()
                .and_then(|c| c.resolve_static(t.os, t.arch))
                .unwrap_or(true);
            if !applies {
                continue;
            }
            let Some(pkg) = item.package(t.os, t.arch) else {
                s.add(
                    Category::Compatibility,
                    "D0302",
                    loc.clone(),
                    format!(
                        "{} has no package for {t}; add an OS condition or disable that target",
                        item.name
                    ),
                    vec![],
                );
                continue;
            };
            if pr.acquisition == Acquisition::Automatic {
                match inst_catalog::redundancy(pkg) {
                    Redundancy::Unavailable => s.add(
                        Category::Error,
                        "D0303",
                        loc.clone(),
                        format!("{}: no verifiable download source; embed a verified copy or use custom sources", item.name),
                        vec![Fix::EmbedPrerequisite(pr.id.clone())],
                    ),
                    Redundancy::Degraded { routes } => s.add(
                        Category::Warning,
                        "D0304",
                        loc.clone(),
                        format!("{}: Source redundancy degraded ({routes} independent source(s)); consider an embedded fallback", item.name),
                        vec![Fix::EmbedPrerequisite(pr.id.clone())],
                    ),
                    Redundancy::Full { .. } => {}
                }
                if matches!(pkg.integrity, IntegrityStrategy::PinAtBuild) {
                    s.add(
                        Category::Compatibility,
                        "D0305",
                        loc.clone(),
                        format!("{}: the vendor publishes no checksum; the hash is pinned at build time, so a vendor re-release breaks the download until you rebuild", item.name),
                        vec![Fix::EmbedPrerequisite(pr.id.clone())],
                    );
                }
            }
            if pkg.install.kind == inst_catalog::InstallKind::ExtractUserLocal {
                s.add(
                    Category::Error,
                    "D0306",
                    loc.clone(),
                    format!("{}: per-user archive installation is not supported by this runtime version", item.name),
                    vec![Fix::RemovePrerequisite(pr.id.clone())],
                );
            }
        }
        if let Some(reason) = &pr.suggested_reason {
            s.add(
                Category::Recommendation,
                "D0307",
                loc,
                format!("{} was suggested by the analyzer: {reason}", item.name),
                vec![],
            );
        }
    }

    // Unsupported runtime features.
    for (i, a) in p.actions.iter().enumerate().filter(|(_, a)| a.enabled) {
        if matches!(a.kind, ActionKind::Database(_) | ActionKind::Http(_)) {
            s.add(
                Category::Error,
                "D0401",
                format!("actions[{i}]"),
                format!(
                    "{} actions are not implemented by this runtime version; disable '{}' to build",
                    a.kind.kind_name(),
                    a.id
                ),
                vec![],
            );
        }
    }
    if !p.integration.file_associations.is_empty() || !p.integration.protocols.is_empty() {
        s.add(
            Category::Compatibility,
            "D0402",
            "integration",
            "file and protocol associations are not applied by this runtime version",
            vec![],
        );
    }

    // Optimization.
    if p.compression.profile == CompressionProfile::None {
        s.add(
            Category::Optimization,
            "D0501",
            "compression.profile",
            "compression is disabled; Auto typically makes the setup 2–4× smaller",
            vec![Fix::UseAutoCompression],
        );
    }
    let languages = p.ui.languages.len();
    if languages == 7 {
        s.add(
            Category::Optimization,
            "D0502",
            "ui.languages",
            "all 7 languages are enabled; each unused language adds ~4 KiB of strings to the setup",
            vec![],
        );
    }
    if p.integration.desktop_shortcut {
        s.add(
            Category::Recommendation,
            "D0503",
            "integration.desktop-shortcut",
            "desktop shortcuts are off by default; users can still create one",
            vec![Fix::DisableDesktopShortcut],
        );
    }

    // Compatibility: toolchain for cross builds.
    let host_linux = cfg!(target_os = "linux");
    if host_linux && targets.iter().any(|t| t.os == Os::Windows) {
        let windres = std::process::Command::new("x86_64-w64-mingw32-windres")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
        if !windres {
            s.add(
                Category::Compatibility,
                "D0601",
                "targets.windows-x64",
                "building Windows installers on Linux needs the mingw-w64 toolchain (x86_64-w64-mingw32-windres)",
                vec![],
            );
        }
    }

    s.0.sort_by_key(|f| f.category);
    Report { findings: s.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inst_model::project::PrerequisiteRef;

    fn project(dir: &Path) -> Project {
        let mut p = Project::new(
            "App",
            "",
            inst_model::Version::parse("1.0").expect("v"),
            "app".into(),
        );
        std::fs::create_dir_all(dir.join("app")).expect("mkdir");
        std::fs::write(dir.join("app/app.exe"), b"MZ").expect("write");
        p.targets.linux_x64 = false;
        p
    }

    #[test]
    fn finds_and_fixes_issues() {
        let tmp = tempfile::tempdir().expect("tmp");
        let file = tmp.path().join("p.instproj");
        let mut p = project(tmp.path());
        p.ui.languages = vec![inst_i18n::Language::De];
        p.compression.profile = CompressionProfile::None;
        p.prerequisites.push(PrerequisiteRef {
            id: "nssm".into(),
            version: inst_model::VersionReq::ANY,
            acquisition: Acquisition::Automatic,
            custom_sources: vec![],
            condition: None,
            suggested_reason: None,
        });
        p.prerequisites.push(PrerequisiteRef {
            id: "vc-redist".into(),
            version: inst_model::VersionReq::ANY,
            acquisition: Acquisition::Automatic,
            custom_sources: vec![],
            condition: None,
            suggested_reason: None,
        });
        let catalog = Catalog::builtin();
        let report = examine(&p, &file, &catalog);
        let codes: Vec<&str> = report.findings.iter().map(|f| f.code).collect();
        for c in [
            "M0302", "D0103", "D0201", "D0303", "D0304", "D0305", "D0501",
        ] {
            assert!(codes.contains(&c), "{c} missing in {codes:?}");
        }
        assert!(report.errors() >= 2);
        // Safe fixes: main executable + fallback language; never embedding.
        let applied = report.fix_all_safe(&mut p);
        assert_eq!(applied, 2);
        assert_eq!(p.application.main_executable.as_deref(), Some("app.exe"));
        assert!(p.ui.languages.contains(&inst_i18n::Language::En));
        assert_eq!(
            p.prerequisites[0].acquisition,
            Acquisition::Automatic,
            "unsafe fixes are not applied"
        );
        assert_eq!(
            p.compression.profile,
            CompressionProfile::None,
            "explicit choices are kept"
        );
    }
}
