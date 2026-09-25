//! Engine scenarios on the real file system: fresh install, upgrade,
//! failure + rollback, crash recovery and uninstall.
//!
//! Runs as a single test because it points the XDG directories at a
//! temporary home via environment variables.

#![allow(unsafe_code)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use inst_i18n::Language;
use inst_log::Logger;
use inst_payload::write::{BlockPlan, InputFile, NoObserver, WriteOptions};
use inst_payload::{CodecParams, Payload};

use crate::app::*;
use crate::cli::CliOptions;
use crate::context::Install;
use crate::error::{ErrorKind, InstallError};
use crate::events::{Mode, NullSink};
use crate::manifest::Manifest;
use crate::session::Session;

const POLICY: Policy = Policy {
    integrity_verification: true,
    rollback: true,
    logging: false,
    silent_install: true,
    silent_uninstall: true,
    uninstaller: true,
    detect_existing_version: true,
    disk_space_check: true,
    failure_recovery: true,
    allow_downgrade: false,
    same_version: SameVersion::Repair,
    reboot: Reboot::Never,
};

static SETTINGS: Settings = Settings {
    scope: Scope::User,
    requires_elevation: false,
    folder: "Acme Test",
    allow_change_location: true,
    languages: &[Language::En],
    fallback_language: Language::En,
    language_selector: false,
    policy: POLICY,
    integration: Integration {
        start_menu: true,
        desktop_shortcut: false,
        launch_at_startup: false,
        add_to_path: true,
        offer_launch: true,
        file_associations: &[],
        protocols: &[],
    },
    fields: &[],
    installed_size: 1 << 20,
    template: "modern",
    accent: 0x3b82f6,
};

static P1: Product = Product { ..PRODUCT_TEMPLATE };
static P2: Product = Product {
    version: "2.0.0",
    ..PRODUCT_TEMPLATE
};
const PRODUCT_TEMPLATE: Product = Product {
    id: "com.example.engine-test",
    name: "Acme Test",
    publisher: "Acme",
    version: "1.0.0",
    description: &[(Language::En, "Test application")],
    homepage: None,
    support_url: None,
    main_executable: Some("bin/app"),
    arguments: &[],
    icon: None,
    logo_png: None,
};

static STEPS: &[StepInfo] = &[
    StepInfo {
        id: "check-environment",
        label: crate::Msg::StepPreparing,
        weight: 1,
    },
    StepInfo {
        id: "resolve-location",
        label: crate::Msg::StepPreparing,
        weight: 1,
    },
    StepInfo {
        id: "extract",
        label: crate::Msg::StepExtracting,
        weight: 90,
    },
    StepInfo {
        id: "create-shortcuts",
        label: crate::Msg::StepShortcuts,
        weight: 1,
    },
    StepInfo {
        id: "integration",
        label: crate::Msg::StepShortcuts,
        weight: 1,
    },
    StepInfo {
        id: "register",
        label: crate::Msg::StepRegistering,
        weight: 1,
    },
    StepInfo {
        id: "verify",
        label: crate::Msg::StepVerifying,
        weight: 1,
    },
];

fn graph(cx: &mut Install<'_>) -> Result<(), InstallError> {
    cx.step(0, |cx| cx.check_environment())?;
    cx.step(1, |cx| cx.resolve_location())?;
    cx.step(2, |cx| cx.extract_application())?;
    cx.step(3, |cx| cx.create_shortcuts())?;
    cx.step(4, |cx| cx.apply_integration())?;
    cx.step(5, |cx| cx.register_uninstaller())?;
    cx.step(6, |cx| cx.verify_installation())?;
    Ok(())
}

fn failing_graph(cx: &mut Install<'_>) -> Result<(), InstallError> {
    graph(cx)?;
    Err(InstallError::new(
        ErrorKind::TaskFailed {
            name: "post-install".into(),
        },
        "injected failure",
    ))
}

static APP_V1: App = App {
    product: &P1,
    settings: &SETTINGS,
    steps: STEPS,
    install: graph,
    before_uninstall: no_uninstall_hook,
    after_uninstall: no_uninstall_hook,
    prerequisites: &[],
};
static APP_V2: App = App {
    product: &P2,
    install: graph,
    ..APP_V1_TEMPLATE
};
static APP_V2_FAILING: App = App {
    product: &P2,
    install: failing_graph,
    ..APP_V1_TEMPLATE
};
const APP_V1_TEMPLATE: App = App {
    product: &PRODUCT_TEMPLATE,
    settings: &SETTINGS,
    steps: STEPS,
    install: graph,
    before_uninstall: no_uninstall_hook,
    after_uninstall: no_uninstall_hook,
    prerequisites: &[],
};

/// Writes `<stub><payload>` to `dest` from `(path, content)` pairs.
fn build_setup(dir: &Path, dest: &Path, files: &[(&str, &[u8])]) {
    let src = dir.join(format!(
        "src-{}",
        dest.file_name().and_then(|n| n.to_str()).unwrap_or("x")
    ));
    let mut inputs = Vec::new();
    for (rel, content) in files {
        let p = src.join(rel);
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        fs::write(&p, content).expect("write");
        inputs.push(InputFile {
            rel_path: (*rel).to_owned(),
            source: p,
            size: content.len() as u64,
            executable: rel.starts_with("bin/"),
            component: 0,
        });
    }
    let stub = b"#!runtime-stub\n";
    let mut out = Vec::from(&stub[..]);
    let plans = vec![BlockPlan {
        params: CodecParams::zstd(3),
        files: (0..inputs.len()).collect(),
    }];
    inst_payload::write::write_payload(
        &mut out,
        stub.len() as u64,
        &inputs,
        &plans,
        &["logs".to_owned()],
        &WriteOptions {
            threads: 2,
            temp_dir: dir.to_path_buf(),
            buffer_size: 64 << 10,
        },
        &NoObserver,
    )
    .expect("payload");
    fs::File::create(dest)
        .expect("create")
        .write_all(&out)
        .expect("write");
}

fn session(app: &'static App, exe: &Path, dir: &Path) -> Session {
    let payload =
        Payload::locate(&mut fs::File::open(exe).expect("open")).map_err(InstallError::from);
    let previous = crate::platform::find_installed(app.product.id, false)
        .and_then(|d| Manifest::load(&d).ok());
    Session {
        app,
        cli: CliOptions {
            dir: Some(dir.to_path_buf()),
            ..CliOptions::default()
        },
        logger: Logger::disabled(),
        log_path: None,
        language: Language::En,
        mode: Install::classify(previous.as_ref(), app.product.version),
        previous,
        default_dir: dir.to_path_buf(),
        exe: exe.to_path_buf(),
        payload_file: exe.to_path_buf(),
        payload,
        cancel: Arc::new(AtomicBool::new(false)),
    }
}

fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<PathBuf> = fs::read_dir(dir)
            .expect("ls")
            .map(|e| e.expect("entry").path())
            .collect();
        entries.sort();
        for p in entries {
            let rel = p
                .strip_prefix(root)
                .expect("prefix")
                .to_string_lossy()
                .into_owned();
            if p.is_dir() {
                out.push((format!("{rel}/"), Vec::new()));
                walk(root, &p, out);
            } else {
                out.push((rel, fs::read(&p).expect("read")));
            }
        }
    }
    walk(dir, dir, &mut out);
    out
}

#[test]
fn install_upgrade_rollback_recovery_uninstall() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).expect("home");
    // SAFETY: this is the only test in the crate that touches these
    // variables, and it does so before spawning any thread.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_DATA_HOME", home.join(".local/share"));
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::set_var("XDG_CACHE_HOME", home.join(".cache"));
    }
    let install_dir = home.join(".local/share/Acme Test");

    // ---- fresh install -------------------------------------------------
    let v1 = tmp.path().join("setup-v1");
    build_setup(
        tmp.path(),
        &v1,
        &[
            ("bin/app", b"#!/bin/sh\necho v1\n"),
            ("data/config.json", b"{\"v\":1}"),
            ("data/obsolete.txt", b"only in v1"),
            ("readme.txt", b"hello"),
        ],
    );
    let s = session(&APP_V1, &v1, &install_dir);
    assert_eq!(s.mode, Mode::Fresh);
    let outcome = s.install(&s.default_options(), &NullSink);
    let ok = outcome.expect("fresh install succeeds");
    assert_eq!(ok.install_dir, install_dir);
    assert!(ok.launch.is_some());
    assert_eq!(
        fs::read(install_dir.join("readme.txt")).expect("read"),
        b"hello"
    );
    assert!(install_dir.join("logs").is_dir());
    assert!(install_dir.join(crate::ops::UNINSTALLER_NAME).exists());
    let desktop = home.join(".local/share/applications/com.example.engine-test.desktop");
    assert!(desktop.exists(), "menu entry");
    let link = home.join(".local/bin/app");
    assert!(fs::symlink_metadata(&link).is_ok(), "PATH symlink");
    let m1 = Manifest::load(&install_dir).expect("manifest");
    assert_eq!(m1.version, "1.0.0");
    assert_eq!(m1.files.len(), 4);
    assert_eq!(
        crate::platform::find_installed("com.example.engine-test", false),
        Some(install_dir.clone())
    );
    // No journal left behind.
    assert!(
        !fs::read_dir(install_dir.parent().expect("parent"))
            .expect("ls")
            .any(|e| e
                .expect("e")
                .file_name()
                .to_string_lossy()
                .starts_with(crate::journal::Journal::PREFIX))
    );

    // User data created by the application after installation.
    fs::write(install_dir.join("data/user.db"), b"precious").expect("user data");

    // ---- upgrade -------------------------------------------------------
    let v2 = tmp.path().join("setup-v2");
    build_setup(
        tmp.path(),
        &v2,
        &[
            ("bin/app", b"#!/bin/sh\necho v2\n"),
            ("data/config.json", b"{\"v\":2}"),
            ("readme.txt", b"hello"),
            ("docs/new.md", b"# new in v2"),
        ],
    );
    let s = session(&APP_V2, &v2, &install_dir);
    assert_eq!(
        s.mode,
        Mode::Upgrade {
            from: "1.0.0".into()
        }
    );
    s.install(&s.default_options(), &NullSink)
        .expect("upgrade succeeds");
    assert_eq!(
        fs::read(install_dir.join("bin/app")).expect("read"),
        b"#!/bin/sh\necho v2\n"
    );
    assert!(
        !install_dir.join("data/obsolete.txt").exists(),
        "obsolete file removed"
    );
    assert_eq!(
        fs::read(install_dir.join("data/user.db")).expect("read"),
        b"precious"
    );
    assert_eq!(
        Manifest::load(&install_dir).expect("manifest").version,
        "2.0.0"
    );
    let before_failure = tree(&install_dir);

    // ---- failed upgrade rolls back completely ---------------------------
    let v3 = tmp.path().join("setup-v3");
    build_setup(
        tmp.path(),
        &v3,
        &[
            ("bin/app", b"#!/bin/sh\necho v3 broken\n"),
            ("data/config.json", b"{\"v\":3}"),
            ("extra/only-v3.bin", b"x"),
        ],
    );
    let s = session(&APP_V2_FAILING, &v3, &install_dir);
    let failure = s
        .install(&s.default_options(), &NullSink)
        .expect_err("injected failure");
    assert!(matches!(failure.error.kind, ErrorKind::TaskFailed { .. }));
    let report = failure.rollback.expect("rolled back");
    assert!(report.is_complete(), "{report:?}");
    assert_eq!(
        tree(&install_dir),
        before_failure,
        "installation restored byte for byte"
    );

    // ---- crash recovery ------------------------------------------------
    {
        let mut journal = crate::journal::Journal::create(&install_dir, "com.example.engine-test")
            .expect("journal");
        let stray = install_dir.join("half-written.tmp");
        journal
            .record(crate::journal::Undo::RemoveFile(stray.clone()))
            .expect("record");
        fs::write(&stray, b"partial").expect("write");
        std::mem::forget(journal); // installer killed
    }
    let s = session(&APP_V2, &v2, &install_dir);
    assert_eq!(s.mode, Mode::Repair);
    s.install(&s.default_options(), &NullSink)
        .expect("repair succeeds after recovery");
    assert!(
        !install_dir.join("half-written.tmp").exists(),
        "interrupted transaction rolled back"
    );

    // ---- uninstall -----------------------------------------------------
    let s = session(&APP_V2, &v2, &install_dir);
    s.uninstall(&NullSink).expect("uninstall succeeds");
    assert!(!desktop.exists());
    assert!(fs::symlink_metadata(&link).is_err());
    assert!(!install_dir.join("bin").exists());
    assert!(!install_dir.join(crate::ops::UNINSTALLER_NAME).exists());
    assert_eq!(
        fs::read(install_dir.join("data/user.db")).expect("user data kept"),
        b"precious"
    );
    assert_eq!(
        crate::platform::find_installed("com.example.engine-test", false),
        None
    );
}
