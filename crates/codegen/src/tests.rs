use std::path::PathBuf;

use inst_model::action::*;
use inst_model::condition::Condition;
use inst_model::graph::{Edge, Node, Step};
use inst_model::platform::{Os, Target};
use inst_model::privileges::{self, NoFacts};
use inst_model::project::{InputField, InputKind, Project};
use inst_model::value::{PathExpr, SecretRef, Value, ValueRef};
use inst_model::{LocalizedText, Version};

use super::*;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn rich_project() -> Project {
    let mut p = Project::new("Acme \"Orders\"", "Acme Ltd.", Version::parse("1.4.0").expect("v"), "dist".into());
    p.application.main_executable = Some("bin/orders".into());
    p.product.description = LocalizedText::plain("Order management");
    p.product.description.set(inst_i18n::Language::Tr, "Sipariş yönetimi".into());
    p.ui.languages = vec![inst_i18n::Language::En, inst_i18n::Language::Tr, inst_i18n::Language::Ar];
    p.ui.fields = vec![
        InputField {
            id: "port".into(),
            label: "Port".into(),
            kind: InputKind::Text { default: "8080".into() },
            required: true,
            prominent: false,
        },
        InputField {
            id: "admin_password".into(),
            label: "Password".into(),
            kind: InputKind::Password,
            required: false,
            prominent: false,
        },
    ];
    p.actions = vec![
        Action {
            id: "configure".into(),
            name: "Configure \"server\"".into(),
            event: Some(LifecycleEvent::AfterInstall),
            condition: Some(Condition::Os { os: Os::Linux }),
            on_failure: FailurePolicy::Rollback,
            enabled: true,
            kind: ActionKind::Script(ScriptAction {
                shell: Shell::Sh,
                source: ScriptSource::Inline {
                    code: "#!/bin/sh\necho \"port=$PORT\" > \"$INST_INSTALL_DIR/port.txt\"\n".into(),
                },
                args: vec![Value::literal("--quiet"), Value::Ref(ValueRef::ProductVersion)],
                env: [
                    ("PORT".to_owned(), Value::Ref(ValueRef::Input { field: "port".into() })),
                    (
                        "PASS".to_owned(),
                        Value::Ref(ValueRef::Secret {
                            secret: SecretRef::Input {
                                field: "admin_password".into(),
                            },
                        }),
                    ),
                ]
                .into(),
                working_dir: Some(PathExpr::install("")),
                timeout_secs: 30,
                elevation: Elevation::Inherit,
                allowed_exit_codes: vec![0],
                capture_stdout: true,
                capture_stderr: true,
            }),
        },
        Action {
            id: "win-only".into(),
            name: String::new(),
            event: Some(LifecycleEvent::AfterInstall),
            condition: Some(Condition::Os { os: Os::Windows }),
            on_failure: FailurePolicy::Continue,
            enabled: true,
            kind: ActionKind::Script(ScriptAction {
                shell: Shell::PowerShell,
                source: ScriptSource::Inline {
                    code: "Write-Output 'windows'".into(),
                },
                args: vec![],
                env: Default::default(),
                working_dir: None,
                timeout_secs: 30,
                elevation: Elevation::Inherit,
                allowed_exit_codes: vec![0],
                capture_stdout: true,
                capture_stderr: true,
            }),
        },
        Action {
            id: "svc".into(),
            name: String::new(),
            event: Some(LifecycleEvent::AfterInstall),
            condition: None,
            on_failure: FailurePolicy::Rollback,
            enabled: true,
            kind: ActionKind::Service(ServiceAction {
                name: "acme-orders".into(),
                display_name: "Acme Orders".into(),
                description: "Background worker".into(),
                executable: "bin/orders".into(),
                args: vec![Value::literal("--service")],
                start: ServiceStart::Automatic,
                scope: ServiceScope::User,
                restart_on_failure: true,
                start_after_install: true,
            }),
        },
        Action {
            id: "cleanup".into(),
            name: String::new(),
            event: Some(LifecycleEvent::BeforeUninstall),
            condition: Some(Condition::Elevated),
            on_failure: FailurePolicy::Continue,
            enabled: true,
            kind: ActionKind::Script(ScriptAction {
                shell: Shell::Sh,
                source: ScriptSource::Payload { path: "scripts/cleanup.sh".into() },
                args: vec![],
                env: Default::default(),
                working_dir: None,
                timeout_secs: 30,
                elevation: Elevation::Inherit,
                allowed_exit_codes: vec![0],
                capture_stdout: true,
                capture_stderr: true,
            }),
        },
    ];
    p.targets.windows_x64 = false;
    p
}

fn input_for<'a>(
    p: &'a Project,
    analysis: &'a privileges::PrivilegeAnalysis,
    runtime: &'a RuntimeSources,
) -> CodegenInput<'a> {
    CodegenInput {
        project: p,
        target: Target::LINUX_X64,
        privileges: analysis,
        installed_size: 1234,
        prerequisites: &[],
        runtime,
        codec_features: &["zstd"],
        gui: false,
        profile: OptProfile::Smallest,
        project_scripts: &[],
        logo_png: None,
        windows_icon: None,
        installed_icon: None,
    }
}

#[test]
fn prunes_platform_branches_and_quotes_literals() {
    let p = rich_project();
    let analysis = privileges::analyze(&p, Target::LINUX_X64, &NoFacts);
    let runtime = RuntimeSources {
        runtime: workspace_root().join("crates/runtime"),
        runtime_ui: None,
    };
    let g = generate(&input_for(&p, &analysis, &runtime)).expect("generate");
    let main = String::from_utf8(g.files.iter().find(|f| f.path == "src/main.rs").expect("main").contents.clone())
        .expect("utf8");
    assert!(main.contains(r#"name: "Acme \"Orders\"""#), "{main}");
    assert!(!main.contains("Write-Output"), "Windows-only script must be pruned on Linux");
    assert!(main.contains("cx.install_service(&"));
    assert!(main.contains("fn before_uninstall"));
    assert!(main.contains("if cx.elevated()"));
    let features: Vec<&str> = g.features.iter().map(|(n, _)| n.as_str()).collect();
    assert!(features.contains(&"scripts") && features.contains(&"services") && features.contains(&"lang-tr"));
    assert!(!features.contains(&"lang-de"), "disabled languages are not compiled");
    assert!(!features.contains(&"registry"));
}

#[test]
fn rejects_unimplemented_actions_honestly() {
    let mut p = rich_project();
    p.actions.push(Action {
        id: "api".into(),
        name: String::new(),
        event: Some(LifecycleEvent::AfterInstall),
        condition: None,
        on_failure: FailurePolicy::Rollback,
        enabled: true,
        kind: ActionKind::Http(HttpAction {
            method: HttpMethod::Get,
            url: "https://example.com".into(),
            query: vec![],
            headers: vec![],
            body: None,
            auth: None,
            timeout_secs: 10,
            retry: Retry::default(),
            expected_status: vec![],
            capture: vec![],
            allow_cross_origin_redirects: false,
        }),
    });
    let analysis = privileges::analyze(&p, Target::LINUX_X64, &NoFacts);
    let runtime = RuntimeSources {
        runtime: workspace_root().join("crates/runtime"),
        runtime_ui: None,
    };
    let err = generate(&input_for(&p, &analysis, &runtime)).expect_err("unsupported");
    assert!(matches!(err, CodegenError::Unsupported { ref action, .. } if action == "api"));
}

#[test]
fn conditional_graph_edges_become_boolean_flow() {
    let mut p = rich_project();
    let mut g = p.effective_graph();
    g.edges.retain(|e| !(e.from == "extract" && e.to == "install-services"));
    g.nodes.push(Node {
        condition: Some(Condition::FreshInstall),
        ..Node::new("first-run", Step::RunAction { action: "configure".into() })
    });
    g.edges.push(Edge {
        from: "extract".into(),
        to: "first-run".into(),
        condition: Some(Condition::OptionSelected { option: "port".into() }),
    });
    g.edges.push(Edge {
        from: "first-run".into(),
        to: "install-services".into(),
        condition: None,
    });
    g.edges.push(Edge {
        from: "extract".into(),
        to: "install-services".into(),
        condition: Some(Condition::not(Condition::OptionSelected { option: "port".into() })),
    });
    p.graph = Some(g);
    let analysis = privileges::analyze(&p, Target::LINUX_X64, &NoFacts);
    let runtime = RuntimeSources {
        runtime: workspace_root().join("crates/runtime"),
        runtime_ui: None,
    };
    let generated = generate(&input_for(&p, &analysis, &runtime)).expect("generate");
    let main = String::from_utf8(generated.files[4].contents.clone()).expect("utf8");
    assert!(main.contains("cx.option(\"port\")"), "{main}");
    assert!(main.contains("if cx.is_fresh_install()"), "{main}");
    assert!(main.contains("|| (a"), "join node must OR its incoming edges:\n{main}");
}

/// Writes the generated crate and type-checks it against the real runtime.
#[test]
fn generated_crate_compiles() {
    let p = rich_project();
    let analysis = privileges::analyze(&p, Target::LINUX_X64, &NoFacts);
    let root = workspace_root();
    let runtime = RuntimeSources {
        runtime: root.join("crates/runtime"),
        runtime_ui: None,
    };
    let generated = generate(&input_for(&p, &analysis, &runtime)).expect("generate");
    let dir = tempfile::tempdir().expect("tmp");
    for f in &generated.files {
        let path = dir.path().join(f.path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, &f.contents).expect("write");
    }
    // Reuse the workspace lockfile for reproducible, offline-friendly builds.
    std::fs::copy(root.join("Cargo.lock"), dir.path().join("Cargo.lock")).expect("lockfile");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = std::process::Command::new(cargo)
        .arg("check")
        .arg("--quiet")
        .current_dir(dir.path())
        .env("CARGO_TARGET_DIR", root.join("target/codegen-check"))
        .output()
        .expect("run cargo");
    assert!(
        output.status.success(),
        "generated crate failed to compile:\n{}\n--- main.rs ---\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&generated.files[4].contents)
    );
}
