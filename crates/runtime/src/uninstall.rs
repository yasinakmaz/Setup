//! Uninstallation driven entirely by the installation manifest.
//!
//! Only what the manifest lists is removed; files created later by the user
//! or the application (data, settings) are kept, and a directory is removed
//! only when it is empty. Every failure is reported.

use std::fs;
use std::path::{Path, PathBuf};

use inst_log::log;

use crate::context::{Common, Uninstall};
use crate::error::{ErrorKind, InstallError};
use crate::events::{Event, EventSink};
use crate::manifest::{METADATA_DIR, Manifest};
use crate::platform;

#[derive(Debug, Default)]
pub struct UninstallReport {
    pub removed_files: usize,
    pub kept_dirs: Vec<PathBuf>,
    pub failures: Vec<String>,
}

pub(crate) fn run(
    c: Common<'_>,
    manifest: &Manifest,
    events: &dyn EventSink,
) -> Result<UninstallReport, InstallError> {
    let app = c.app;
    let install_dir = c.install_dir.clone();
    let logger = c.logger.clone();
    let mut report = UninstallReport::default();
    log!(logger, Info, "uninstall", "uninstalling";
        "product" => manifest.product_id.as_str(), "version" => manifest.version.as_str(),
        "dir" => &install_dir.to_string_lossy().into_owned());
    if manifest.product_id != app.product.id {
        return Err(InstallError::unexpected(format!(
            "manifest belongs to {} , not {}",
            manifest.product_id, app.product.id
        )));
    }

    events.send(Event::StepStarted(0));
    let mut cx = Uninstall { c };
    (app.before_uninstall)(&mut cx)?;

    // External integrations first (services must stop before files go).
    for e in manifest.external.iter().rev() {
        if let Err(err) = platform::remove_external(e) {
            let msg = format!("{e:?}: {err}");
            log!(logger, Error, "uninstall", "could not remove"; "item" => &msg);
            report.failures.push(msg);
        }
    }

    let total = manifest.files.len().max(1);
    for (i, f) in manifest.files.iter().enumerate() {
        let Ok(rel) = inst_fsx::RelPath::new(&f.path) else {
            report
                .failures
                .push(format!("unsafe path in manifest: {}", f.path));
            continue;
        };
        let path = rel.to_path_under(&install_dir);
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() || m.is_file() => match fs::remove_file(&path) {
                Ok(()) => report.removed_files += 1,
                Err(e) => report.failures.push(format!("{}: {e}", path.display())),
            },
            Ok(_) => report
                .failures
                .push(format!("{} is not a file", path.display())),
            Err(_) => {} // already gone
        }
        if i % 64 == 0 {
            events.send(Event::StepProgress {
                step: 0,
                fraction: i as f32 / total as f32,
            });
        }
    }

    let _ = fs::remove_dir_all(install_dir.join(METADATA_DIR));
    let mut dirs: Vec<&String> = manifest.dirs.iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.matches('/').count()));
    for d in dirs {
        if let Ok(rel) = inst_fsx::RelPath::new(d) {
            let p = rel.to_path_under(&install_dir);
            if fs::remove_dir(&p).is_err() && p.exists() {
                report.kept_dirs.push(p);
            }
        }
    }

    remove_uninstaller(&install_dir, manifest, &mut report);
    if fs::remove_dir(&install_dir).is_err() && install_dir.exists() {
        report.kept_dirs.push(install_dir.clone());
    }
    for d in &report.kept_dirs {
        log!(logger, Info, "uninstall", "kept folder with user data"; "dir" => &d.to_string_lossy().into_owned());
    }

    (app.after_uninstall)(&mut cx)?;
    events.send(Event::StepFinished(0));
    log!(logger, Info, "uninstall", "uninstall finished";
        "removed" => report.removed_files, "failures" => report.failures.len());
    if report.failures.is_empty() {
        Ok(report)
    } else {
        Err(InstallError::new(
            ErrorKind::Unexpected,
            report.failures.join("; "),
        ))
    }
}

fn remove_uninstaller(install_dir: &Path, manifest: &Manifest, report: &mut UninstallReport) {
    let Some(rel) = manifest
        .uninstaller
        .as_deref()
        .and_then(|u| inst_fsx::RelPath::new(u).ok())
    else {
        return;
    };
    let path = rel.to_path_under(install_dir);
    let running_self = std::env::current_exe()
        .ok()
        .and_then(|e| e.canonicalize().ok())
        .zip(path.canonicalize().ok())
        .is_some_and(|(a, b)| a == b);
    if !running_self || cfg!(unix) {
        // Unix can unlink a running executable.
        if let Err(e) = fs::remove_file(&path)
            && path.exists()
        {
            report.failures.push(format!("{}: {e}", path.display()));
        }
    }
    #[cfg(windows)]
    if running_self {
        // A running image cannot be deleted but can be renamed on the same
        // volume. Move it next to the installation folder, then delete it
        // after this process exits.
        let parent = install_dir.parent().unwrap_or(install_dir);
        let trash = parent.join(format!(
            ".inst-trash-{:x}.exe",
            inst_fsx::atomic::unique_token()
        ));
        match fs::rename(&path, &trash) {
            Ok(()) => {
                platform::delete_on_reboot(&trash);
                let cmd = std::env::var_os("ComSpec")
                    .map_or_else(|| PathBuf::from("cmd.exe"), PathBuf::from);
                let script = format!(
                    "ping -n 3 127.0.0.1 >NUL & del /f /q \"{}\"",
                    trash.to_string_lossy().replace('"', "")
                );
                let _ = platform::launch_detached(&cmd, &["/d", "/c", &script], parent);
            }
            Err(e) => report.failures.push(format!("{}: {e}", path.display())),
        }
    }
}

/// Finds the manifest for an uninstall: `--dir`, the uninstaller's own
/// folder, or the registered installation.
pub(crate) fn locate(
    app: &'static crate::app::App,
    explicit: Option<&Path>,
) -> Result<(PathBuf, Manifest), InstallError> {
    let machine = app.settings.scope == crate::app::Scope::Machine;
    let candidates: Vec<PathBuf> = explicit
        .map(Path::to_path_buf)
        .into_iter()
        .chain(
            std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(Path::to_path_buf)),
        )
        .chain(platform::find_installed(app.product.id, machine))
        .collect();
    for dir in candidates {
        if let Ok(m) = Manifest::load(&dir)
            && m.product_id == app.product.id
        {
            return Ok((dir, m));
        }
    }
    Err(InstallError::new(
        ErrorKind::Unexpected,
        format!(
            "{} is not installed (no installation manifest found)",
            app.product.name
        ),
    ))
}
