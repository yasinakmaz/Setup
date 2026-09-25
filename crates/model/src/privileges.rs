//! Automatic privilege detection.
//!
//! The installer asks for administrator/root rights only when the
//! installation graph actually contains a machine-wide operation. Every such
//! operation is reported as a [`PrivilegeReason`] so the Studio can explain
//! the decision ("Administrator required because: Windows service `acme`").

use std::fmt;

use crate::action::{ActionKind, Elevation, EnvScope, RegistryHive, ServiceScope};
use crate::platform::{Os, Target};
use crate::project::{InstallBase, Privileges, Project};

/// Facts about prerequisites that only the catalog knows.
pub trait PrerequisiteFacts {
    /// Whether installing prerequisite `id` on `os` needs elevation.
    /// `None` when unknown (treated as not requiring it, with a Doctor note).
    fn requires_admin(&self, id: &str, os: Os) -> Option<bool>;
}

/// No catalog available (unit tests, early analysis).
pub struct NoFacts;
impl PrerequisiteFacts for NoFacts {
    fn requires_admin(&self, _id: &str, _os: Os) -> Option<bool> {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    User,
    Admin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrivilegeReason {
    PerMachineLocation,
    MachineRegistry { key: String },
    SystemService { name: String },
    MachineEnvironment { name: String },
    FirewallRule { name: String },
    ElevatedScript { action: String },
    Prerequisite { id: String },
}

impl fmt::Display for PrivilegeReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrivilegeReason::PerMachineLocation => {
                f.write_str("installs for all users (Program Files / /opt)")
            }
            PrivilegeReason::MachineRegistry { key } => write!(f, "writes HKLM\\{key}"),
            PrivilegeReason::SystemService { name } => write!(f, "installs system service '{name}'"),
            PrivilegeReason::MachineEnvironment { name } => {
                write!(f, "changes system environment variable {name}")
            }
            PrivilegeReason::FirewallRule { name } => write!(f, "adds firewall rule '{name}'"),
            PrivilegeReason::ElevatedScript { action } => {
                write!(f, "script '{action}' requires elevation")
            }
            PrivilegeReason::Prerequisite { id } => {
                write!(f, "prerequisite '{id}' installs machine-wide")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivilegeAnalysis {
    pub target: Target,
    pub requested: Privileges,
    /// Level needed by the operations in the graph.
    pub required: Level,
    /// Level the generated installer will request.
    pub effective: Level,
    pub reasons: Vec<PrivilegeReason>,
    /// The developer forced `CurrentUser` although machine-wide operations
    /// exist. This is a build error.
    pub conflict: bool,
    /// Resolved installation base for `InstallBase::Auto`.
    pub install_base: InstallBase,
}

/// Analyzes the privileges `project` needs on `target`.
pub fn analyze(project: &Project, target: Target, facts: &dyn PrerequisiteFacts) -> PrivilegeAnalysis {
    let os = target.os;
    let mut reasons = Vec::new();
    let applies = |cond: &Option<crate::condition::Condition>| {
        cond.as_ref()
            .and_then(|c| c.resolve_static(os, target.arch))
            .unwrap_or(true)
    };

    if project.install.location.base == InstallBase::PerMachine {
        reasons.push(PrivilegeReason::PerMachineLocation);
    }
    for action in project.actions.iter().filter(|a| a.enabled && applies(&a.condition)) {
        match &action.kind {
            ActionKind::Registry(r) if os == Os::Windows && r.hive == RegistryHive::LocalMachine => {
                reasons.push(PrivilegeReason::MachineRegistry { key: r.key.clone() });
            }
            ActionKind::Service(s) if os == Os::Windows || s.scope == ServiceScope::System => {
                reasons.push(PrivilegeReason::SystemService { name: s.name.clone() });
            }
            ActionKind::Environment(e) if e.scope == EnvScope::Machine => {
                reasons.push(PrivilegeReason::MachineEnvironment { name: e.name.clone() });
            }
            ActionKind::Script(s) if s.elevation == Elevation::Required => {
                reasons.push(PrivilegeReason::ElevatedScript {
                    action: action.id.clone(),
                });
            }
            _ => {}
        }
    }
    for rule in &project.integration.firewall_rules {
        reasons.push(PrivilegeReason::FirewallRule {
            name: rule.name.clone(),
        });
    }
    for prereq in &project.prerequisites {
        if !applies(&prereq.condition) {
            continue;
        }
        if facts.requires_admin(&prereq.id, os) == Some(true) {
            reasons.push(PrivilegeReason::Prerequisite {
                id: prereq.id.clone(),
            });
        }
    }

    let required = if reasons.is_empty() {
        Level::User
    } else {
        Level::Admin
    };
    let requested = project.install.privileges;
    let effective = match requested {
        Privileges::Auto => required,
        Privileges::CurrentUser => Level::User,
        Privileges::Administrator => Level::Admin,
    };
    let install_base = match &project.install.location.base {
        InstallBase::Auto if effective == Level::Admin => InstallBase::PerMachine,
        InstallBase::Auto => InstallBase::PerUser,
        other => other.clone(),
    };
    PrivilegeAnalysis {
        target,
        requested,
        required,
        effective,
        conflict: requested == Privileges::CurrentUser && required == Level::Admin,
        reasons,
        install_base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{Action, FailurePolicy, LifecycleEvent, ServiceAction, ServiceStart};
    use crate::condition::Condition;
    use crate::ids::Version;

    fn project() -> Project {
        Project::new("App", "Acme", Version::parse("1.0").expect("v"), "dist".into())
    }

    fn service(scope: ServiceScope) -> Action {
        Action {
            id: "svc".into(),
            name: String::new(),
            event: Some(LifecycleEvent::AfterInstall),
            condition: None,
            on_failure: FailurePolicy::Rollback,
            enabled: true,
            kind: ActionKind::Service(ServiceAction {
                name: "acme".into(),
                display_name: "Acme".into(),
                description: String::new(),
                executable: "acme-svc".into(),
                args: vec![],
                start: ServiceStart::Automatic,
                scope,
                restart_on_failure: true,
                start_after_install: true,
            }),
        }
    }

    #[test]
    fn user_local_by_default() {
        let a = analyze(&project(), Target::WINDOWS_X64, &NoFacts);
        assert_eq!(a.effective, Level::User);
        assert_eq!(a.install_base, InstallBase::PerUser);
        assert!(a.reasons.is_empty());
    }

    #[test]
    fn service_requires_admin_on_windows_but_user_unit_not_on_linux() {
        let mut p = project();
        p.actions.push(service(ServiceScope::User));
        let win = analyze(&p, Target::WINDOWS_X64, &NoFacts);
        assert_eq!(win.effective, Level::Admin);
        assert_eq!(win.install_base, InstallBase::PerMachine);
        let linux = analyze(&p, Target::LINUX_X64, &NoFacts);
        assert_eq!(linux.effective, Level::User);
    }

    #[test]
    fn platform_conditions_are_respected() {
        let mut p = project();
        let mut a = service(ServiceScope::System);
        a.condition = Some(Condition::Os { os: Os::Linux });
        p.actions.push(a);
        assert_eq!(analyze(&p, Target::WINDOWS_X64, &NoFacts).effective, Level::User);
        assert_eq!(analyze(&p, Target::LINUX_X64, &NoFacts).effective, Level::Admin);
    }

    #[test]
    fn forced_current_user_conflicts() {
        let mut p = project();
        p.install.privileges = Privileges::CurrentUser;
        p.install.location.base = InstallBase::PerMachine;
        let a = analyze(&p, Target::LINUX_X64, &NoFacts);
        assert!(a.conflict);
        assert_eq!(a.effective, Level::User);
    }
}
