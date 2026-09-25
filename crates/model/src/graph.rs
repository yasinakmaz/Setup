//! The installation graph.
//!
//! The core model is a directed acyclic graph of typed steps, not a linear
//! wizard. Nodes and edges can carry [`Condition`]s:
//!
//! * a node is **active** when any incoming edge comes from an active node
//!   and that edge's condition holds (`Start` is always active);
//! * an active node **executes** when its own condition holds; a skipped node
//!   still passes activity downstream.
//!
//! Code generation linearizes the graph in a deterministic topological order
//! and emits straight-line Rust with one boolean per node.

use serde::{Deserialize, Serialize};
use std::collections::BinaryHeap;
use std::cmp::Reverse;
use std::fmt;

use crate::action::{ActionKind, LifecycleEvent};
use crate::condition::Condition;
use crate::ids::validate_local_id;
use crate::project::Project;
use crate::text::LocalizedText;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "kebab-case", rename_all_fields = "kebab-case")]
pub enum Step {
    Start,
    /// OS, architecture, disk space, existing installation.
    CheckEnvironment,
    /// Detect which prerequisites are missing.
    CheckPrerequisites,
    /// Download missing prerequisites (resumable, verified, cached).
    AcquirePrerequisites,
    InstallPrerequisites,
    /// Resolve (or let the user choose) the installation directory.
    ResolveLocation,
    /// Run the actions attached to a lifecycle event.
    RunActions { event: LifecycleEvent },
    /// Run one specific action.
    RunAction { action: String },
    ExtractApplication,
    /// Database actions attached to `after-install`.
    ConfigureDatabase,
    /// Service actions attached to `after-install`.
    InstallServices,
    CreateShortcuts,
    /// Registry, environment, file/protocol associations, firewall rules.
    ApplyIntegration,
    /// Installation manifest, uninstaller, Installed Apps entry.
    RegisterUninstaller,
    VerifyInstallation,
    Finish,
}

impl Step {
    pub fn label(&self) -> &'static str {
        match self {
            Step::Start => "Start",
            Step::CheckEnvironment => "Check Environment",
            Step::CheckPrerequisites => "Check Prerequisites",
            Step::AcquirePrerequisites => "Download Missing Prerequisites",
            Step::InstallPrerequisites => "Install Prerequisites",
            Step::ResolveLocation => "Resolve Install Path",
            Step::RunActions { event } => event.label(),
            Step::RunAction { .. } => "Run Action",
            Step::ExtractApplication => "Extract Application",
            Step::ConfigureDatabase => "Configure Database",
            Step::InstallServices => "Install Services",
            Step::CreateShortcuts => "Create Shortcuts",
            Step::ApplyIntegration => "Platform Integration",
            Step::RegisterUninstaller => "Register Uninstaller",
            Step::VerifyInstallation => "Verify Installation",
            Step::Finish => "Finish",
        }
    }

    /// Steps that may appear only once.
    pub fn is_singleton(&self) -> bool {
        !matches!(self, Step::RunAction { .. } | Step::RunActions { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Node {
    pub id: String,
    #[serde(flatten)]
    pub step: Step,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Condition>,
    #[serde(default, skip_serializing_if = "LocalizedText::is_empty")]
    pub label: LocalizedText,
    /// Designer canvas position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f32; 2]>,
}

impl Node {
    pub fn new(id: &str, step: Step) -> Node {
        Node {
            id: id.to_owned(),
            step,
            condition: None,
            label: LocalizedText::Empty,
            position: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Edge {
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Condition>,
}

type Adjacency = Vec<Vec<usize>>;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InstallGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphIssue {
    InvalidId(String),
    DuplicateId(String),
    MissingStart,
    MissingFinish,
    Duplicate(&'static str),
    UnknownNode(String),
    SelfLoop(String),
    Cycle(Vec<String>),
    Unreachable(String),
    DeadEnd(String),
    IncomingToStart,
    OutgoingFromFinish,
    UnknownAction(String),
    OrderViolation {
        before: &'static str,
        after: &'static str,
    },
    ConditionalCriticalStep(&'static str),
}

impl GraphIssue {
    /// Warnings do not block a build.
    pub fn is_warning(&self) -> bool {
        matches!(self, GraphIssue::ConditionalCriticalStep(_))
    }
}

impl fmt::Display for GraphIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphIssue::InvalidId(id) => write!(f, "invalid node id {id:?}"),
            GraphIssue::DuplicateId(id) => write!(f, "duplicate node id {id:?}"),
            GraphIssue::MissingStart => f.write_str("graph has no Start node"),
            GraphIssue::MissingFinish => f.write_str("graph has no Finish node"),
            GraphIssue::Duplicate(step) => write!(f, "step '{step}' appears more than once"),
            GraphIssue::UnknownNode(id) => write!(f, "edge references unknown node {id:?}"),
            GraphIssue::SelfLoop(id) => write!(f, "node {id:?} links to itself"),
            GraphIssue::Cycle(ids) => write!(f, "cycle between nodes {}", ids.join(" → ")),
            GraphIssue::Unreachable(id) => write!(f, "node {id:?} is not reachable from Start"),
            GraphIssue::DeadEnd(id) => write!(f, "Finish is not reachable from node {id:?}"),
            GraphIssue::IncomingToStart => f.write_str("Start must not have incoming edges"),
            GraphIssue::OutgoingFromFinish => f.write_str("Finish must not have outgoing edges"),
            GraphIssue::UnknownAction(id) => write!(f, "node references unknown action {id:?}"),
            GraphIssue::OrderViolation { before, after } => {
                write!(f, "'{before}' must come before '{after}'")
            }
            GraphIssue::ConditionalCriticalStep(step) => {
                write!(f, "'{step}' is conditional; the installation may be incomplete")
            }
        }
    }
}

/// Where an action runs when it is not placed explicitly in the graph.
pub fn implicit_step(kind: &ActionKind, event: LifecycleEvent) -> Step {
    match (kind, event) {
        (ActionKind::Database(_), LifecycleEvent::AfterInstall) => Step::ConfigureDatabase,
        (ActionKind::Service(_), LifecycleEvent::AfterInstall) => Step::InstallServices,
        (_, event) => Step::RunActions { event },
    }
}

impl InstallGraph {
    /// Derives the default graph for `project`. Steps with nothing to do are
    /// omitted, so the generated installer contains no dead code.
    pub fn default_for(project: &Project) -> InstallGraph {
        let has_prereqs = !project.prerequisites.is_empty();
        let needs_download = project
            .prerequisites
            .iter()
            .any(|p| p.acquisition != crate::project::Acquisition::Embedded);
        let has = |step: Step| {
            project.actions.iter().any(|a| {
                a.enabled && a.event.is_some_and(|e| implicit_step(&a.kind, e) == step)
            })
        };
        let integ = &project.integration;
        let has_integration = !integ.file_associations.is_empty()
            || !integ.protocols.is_empty()
            || !integ.firewall_rules.is_empty()
            || integ.add_to_path
            || integ.launch_at_startup
            || project.actions.iter().any(|a| {
                a.enabled
                    && matches!(a.kind, ActionKind::Registry(_) | ActionKind::Environment(_))
                    && a.event.is_none()
            });

        let mut steps: Vec<(&str, Step)> = Vec::with_capacity(20);
        steps.push(("start", Step::Start));
        steps.push(("check-environment", Step::CheckEnvironment));
        if has_prereqs {
            steps.push(("check-prerequisites", Step::CheckPrerequisites));
            if needs_download {
                steps.push(("download-prerequisites", Step::AcquirePrerequisites));
            }
            steps.push(("install-prerequisites", Step::InstallPrerequisites));
        }
        steps.push(("resolve-location", Step::ResolveLocation));
        let before = Step::RunActions {
            event: LifecycleEvent::BeforeInstall,
        };
        if has(before.clone()) {
            steps.push(("before-install", before));
        }
        steps.push(("extract", Step::ExtractApplication));
        if has(Step::ConfigureDatabase) {
            steps.push(("configure-database", Step::ConfigureDatabase));
        }
        if has(Step::InstallServices) {
            steps.push(("install-services", Step::InstallServices));
        }
        if integ.start_menu || integ.desktop_shortcut {
            steps.push(("create-shortcuts", Step::CreateShortcuts));
        }
        if has_integration {
            steps.push(("integration", Step::ApplyIntegration));
        }
        if project.policy.uninstaller {
            steps.push(("register", Step::RegisterUninstaller));
        }
        for (id, event) in [
            ("after-install", LifecycleEvent::AfterInstall),
            ("on-repair", LifecycleEvent::OnRepair),
            ("on-update", LifecycleEvent::OnUpdate),
        ] {
            let step = Step::RunActions { event };
            if has(step.clone()) {
                steps.push((id, step));
            }
        }
        steps.push(("verify", Step::VerifyInstallation));
        steps.push(("finish", Step::Finish));

        let nodes: Vec<Node> = steps
            .iter()
            .enumerate()
            .map(|(i, (id, step))| Node {
                position: Some([0.0, i as f32 * 80.0]),
                ..Node::new(id, step.clone())
            })
            .collect();
        let edges = nodes
            .windows(2)
            .map(|w| Edge {
                from: w[0].id.clone(),
                to: w[1].id.clone(),
                condition: None,
            })
            .collect();
        InstallGraph { nodes, edges }
    }

    pub fn node_index(&self, id: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }

    /// Outgoing and incoming adjacency lists.
    fn adjacency(&self) -> Result<(Adjacency, Adjacency), GraphIssue> {
        let mut out = vec![Vec::new(); self.nodes.len()];
        let mut inc = vec![Vec::new(); self.nodes.len()];
        for e in &self.edges {
            let from = self
                .node_index(&e.from)
                .ok_or_else(|| GraphIssue::UnknownNode(e.from.clone()))?;
            let to = self
                .node_index(&e.to)
                .ok_or_else(|| GraphIssue::UnknownNode(e.to.clone()))?;
            out[from].push(to);
            inc[to].push(from);
        }
        Ok((out, inc))
    }

    /// Deterministic topological order (Kahn's algorithm, ties broken by
    /// declaration order). Fails on cycles.
    pub fn topo_order(&self) -> Result<Vec<usize>, GraphIssue> {
        let (out, inc) = self.adjacency()?;
        let mut indegree: Vec<usize> = inc.iter().map(Vec::len).collect();
        let mut ready: BinaryHeap<Reverse<usize>> = indegree
            .iter()
            .enumerate()
            .filter(|(_, d)| **d == 0)
            .map(|(i, _)| Reverse(i))
            .collect();
        let mut order = Vec::with_capacity(self.nodes.len());
        while let Some(Reverse(n)) = ready.pop() {
            order.push(n);
            for &m in &out[n] {
                indegree[m] -= 1;
                if indegree[m] == 0 {
                    ready.push(Reverse(m));
                }
            }
        }
        if order.len() != self.nodes.len() {
            let cyclic = indegree
                .iter()
                .enumerate()
                .filter(|(_, d)| **d > 0)
                .map(|(i, _)| self.nodes[i].id.clone())
                .collect();
            return Err(GraphIssue::Cycle(cyclic));
        }
        Ok(order)
    }

    /// Full validation against the project.
    pub fn validate(&self, project: &Project) -> Vec<GraphIssue> {
        let mut issues = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for n in &self.nodes {
            if validate_local_id(&n.id).is_err() {
                issues.push(GraphIssue::InvalidId(n.id.clone()));
            }
            if !seen.insert(n.id.as_str()) {
                issues.push(GraphIssue::DuplicateId(n.id.clone()));
            }
            if let Step::RunAction { action } = &n.step
                && project.action(action).is_none()
            {
                issues.push(GraphIssue::UnknownAction(action.clone()));
            }
        }
        for e in &self.edges {
            if e.from == e.to {
                issues.push(GraphIssue::SelfLoop(e.from.clone()));
            }
        }
        let mut counts: std::collections::HashMap<&'static str, usize> = Default::default();
        for n in &self.nodes {
            if n.step.is_singleton() {
                *counts.entry(n.step.label()).or_default() += 1;
            }
        }
        for (label, count) in &counts {
            if *count > 1 {
                issues.push(GraphIssue::Duplicate(label));
            }
        }
        let find = |step: &Step| self.nodes.iter().position(|n| &n.step == step);
        let start = find(&Step::Start);
        let finish = find(&Step::Finish);
        if start.is_none() {
            issues.push(GraphIssue::MissingStart);
        }
        if finish.is_none() {
            issues.push(GraphIssue::MissingFinish);
        }
        let (out, inc) = match self.adjacency() {
            Ok(adj) => adj,
            Err(e) => {
                issues.push(e);
                return issues;
            }
        };
        if let Err(e) = self.topo_order() {
            issues.push(e);
            return issues;
        }
        let (Some(start), Some(finish)) = (start, finish) else {
            return issues;
        };
        if !inc[start].is_empty() {
            issues.push(GraphIssue::IncomingToStart);
        }
        if !out[finish].is_empty() {
            issues.push(GraphIssue::OutgoingFromFinish);
        }
        let from_start = reach(&out, start);
        let to_finish = reach(&inc, finish);
        for (i, n) in self.nodes.iter().enumerate() {
            if !from_start[i] {
                issues.push(GraphIssue::Unreachable(n.id.clone()));
            } else if !to_finish[i] {
                issues.push(GraphIssue::DeadEnd(n.id.clone()));
            }
        }

        // Ordering constraints between critical steps.
        let order_rules: [(&Step, &[Step]); 2] = [
            (
                &Step::ResolveLocation,
                &[Step::ExtractApplication, Step::CreateShortcuts, Step::RegisterUninstaller],
            ),
            (
                &Step::ExtractApplication,
                &[
                    Step::ConfigureDatabase,
                    Step::InstallServices,
                    Step::CreateShortcuts,
                    Step::RegisterUninstaller,
                    Step::VerifyInstallation,
                ],
            ),
        ];
        for (before, afters) in order_rules {
            let Some(b) = find(before) else { continue };
            let reachable = reach(&out, b);
            for after in afters {
                if let Some(a) = find(after)
                    && !reachable[a]
                {
                    issues.push(GraphIssue::OrderViolation {
                        before: before.label(),
                        after: after.label(),
                    });
                }
            }
        }
        for critical in [Step::ExtractApplication, Step::RegisterUninstaller] {
            if let Some(i) = find(&critical) {
                let conditional_edge = self
                    .edges
                    .iter()
                    .any(|e| e.to == self.nodes[i].id && e.condition.is_some());
                if self.nodes[i].condition.is_some() || conditional_edge {
                    issues.push(GraphIssue::ConditionalCriticalStep(critical.label()));
                }
            }
        }
        issues
    }
}

fn reach(adj: &[Vec<usize>], from: usize) -> Vec<bool> {
    let mut seen = vec![false; adj.len()];
    let mut stack = vec![from];
    while let Some(n) = stack.pop() {
        if std::mem::replace(&mut seen[n], true) {
            continue;
        }
        stack.extend(adj[n].iter().copied().filter(|m| !seen[*m]));
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::Version;
    use crate::platform::Os;

    fn project() -> Project {
        Project::new(
            "Acme Orders",
            "Acme",
            Version::parse("1.4.0").expect("version"),
            "dist".into(),
        )
    }

    #[test]
    fn default_graph_is_valid_and_minimal() {
        let p = project();
        let g = InstallGraph::default_for(&p);
        assert_eq!(g.validate(&p), vec![]);
        let steps: Vec<_> = g.nodes.iter().map(|n| n.step.label()).collect();
        assert_eq!(
            steps,
            [
                "Start",
                "Check Environment",
                "Resolve Install Path",
                "Extract Application",
                "Create Shortcuts",
                "Register Uninstaller",
                "Verify Installation",
                "Finish"
            ]
        );
        assert_eq!(g.topo_order().expect("dag"), (0..g.nodes.len()).collect::<Vec<_>>());
    }

    #[test]
    fn detects_cycles_and_unreachable_nodes() {
        let p = project();
        let mut g = InstallGraph::default_for(&p);
        g.edges.push(Edge {
            from: "verify".into(),
            to: "extract".into(),
            condition: None,
        });
        assert!(g.validate(&p).iter().any(|i| matches!(i, GraphIssue::Cycle(_))));

        let mut g = InstallGraph::default_for(&p);
        g.nodes.push(Node::new("orphan", Step::RunActions { event: LifecycleEvent::AfterInstall }));
        assert!(g.validate(&p).contains(&GraphIssue::Unreachable("orphan".into())));
    }

    #[test]
    fn conditional_branch_with_join_is_valid() {
        let p = project();
        let mut g = InstallGraph::default_for(&p);
        // extract → (Windows) win-task → shortcuts ; extract → (Linux) linux-task → shortcuts
        g.edges.retain(|e| !(e.from == "extract" && e.to == "create-shortcuts"));
        for (id, os) in [("win-task", Os::Windows), ("linux-task", Os::Linux)] {
            g.nodes.push(Node::new(id, Step::RunActions { event: LifecycleEvent::AfterInstall }));
            g.edges.push(Edge {
                from: "extract".into(),
                to: id.into(),
                condition: Some(Condition::Os { os }),
            });
            g.edges.push(Edge {
                from: id.into(),
                to: "create-shortcuts".into(),
                condition: None,
            });
        }
        assert_eq!(g.validate(&p), vec![]);
        let order = g.topo_order().expect("dag");
        let pos = |id: &str| order.iter().position(|&i| g.nodes[i].id == id).expect("node");
        assert!(pos("extract") < pos("win-task") && pos("win-task") < pos("create-shortcuts"));
    }

    #[test]
    fn enforces_critical_ordering() {
        let p = project();
        let mut g = InstallGraph::default_for(&p);
        // Move shortcut creation before extraction.
        g.edges = vec![
            ("start", "check-environment"),
            ("check-environment", "resolve-location"),
            ("resolve-location", "create-shortcuts"),
            ("create-shortcuts", "register"),
            ("register", "extract"),
            ("extract", "verify"),
            ("verify", "finish"),
        ]
        .into_iter()
        .map(|(a, b)| Edge {
            from: a.into(),
            to: b.into(),
            condition: None,
        })
        .collect();
        let issues = g.validate(&p);
        assert!(issues.iter().any(|i| matches!(i, GraphIssue::OrderViolation { after: "Create Shortcuts", .. })), "{issues:?}");
    }

    #[test]
    fn toml_roundtrip() {
        let p = project();
        let g = InstallGraph::default_for(&p);
        let text = toml::to_string(&g).expect("ser");
        let back: InstallGraph = toml::from_str(&text).expect("de");
        assert_eq!(back, g);
    }
}
