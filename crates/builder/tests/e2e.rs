//! End-to-end pipeline test: builds the `examples/hello` project into a real
//! Linux installer, runs it to install, checks the installed files, the
//! manifest and log secret redaction, then uninstalls and checks removal.
//!
//! Ignored by default: it invokes `cargo build` for the generated crate and
//! downloads the AppImage runtime on first use, both too slow/networked for
//! a normal `cargo test` run. Run explicitly with:
//! `cargo test -p inst-builder --test e2e -- --ignored`.

use std::path::{Path, PathBuf};
use std::process::Command;

use inst_builder::{BuildObserver, BuildRequest};
use inst_catalog::Catalog;
use inst_model::platform::Target;

struct Silent;
impl BuildObserver for Silent {}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/builder is two levels below the repo root")
        .to_path_buf()
}

#[test]
#[ignore = "compiles a generated crate and downloads the AppImage runtime"]
fn install_and_uninstall_hello() {
    let root = repo_root();
    let project_file = root
        .join("examples/hello/hello.instproj")
        .canonicalize()
        .expect("examples/hello/hello.instproj should exist");
    let text = std::fs::read_to_string(&project_file).expect("reading the project file");
    let project = inst_model::io::from_toml(&text).expect("parsing the project file");

    let work = tempfile::tempdir().expect("creating a temp dir");
    let mut req = BuildRequest::new(project_file, project);
    req.targets = vec![Target::LINUX_X64];
    req.gui = false;
    req.output_dir = work.path().join("out");
    req.work_dir = work.path().join("build");

    let catalog = Catalog::builtin();
    let reports = inst_builder::build(&req, &catalog, &Silent).expect("build failed");
    assert_eq!(reports.len(), 1);

    let installer = req.output_dir.join("Hello-Example-Setup-x86_64.AppImage");
    assert!(
        installer.is_file(),
        "installer not produced at {}",
        installer.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&installer)
            .expect("reading installer metadata")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&installer, perms).expect("marking the installer executable");
    }

    let install_dir = work.path().join("Hello");
    let status = Command::new(&installer)
        .args(["--silent", "--dir"])
        .arg(&install_dir)
        .arg("--set")
        .arg("api_token=secret123")
        .status()
        .expect("failed to run the installer");
    assert!(status.success(), "install exited with {status:?}");

    assert!(install_dir.join("bin/hello").is_file());
    assert!(install_dir.join("config/settings.json").is_file());
    assert_eq!(
        std::fs::read_to_string(install_dir.join("config/greeting.txt"))
            .expect("reading config/greeting.txt")
            .trim(),
        "Merhaba"
    );

    let log = std::fs::read_to_string(install_dir.join(".installer-runtime/install.log"))
        .expect("reading the install log");
    assert!(
        !log.contains("secret123"),
        "the API token leaked into the install log:\n{log}"
    );
    assert!(log.contains("token is ***"));

    let uninstaller = install_dir.join("uninstall");
    assert!(uninstaller.is_file());
    let status = Command::new(&uninstaller)
        .args(["--uninstall", "--silent"])
        .status()
        .expect("failed to run the uninstaller");
    assert!(status.success(), "uninstall exited with {status:?}");

    assert!(
        !install_dir.join("bin").exists(),
        "application files were not removed"
    );
    assert!(!install_dir.join("config/settings.json").exists());
}
