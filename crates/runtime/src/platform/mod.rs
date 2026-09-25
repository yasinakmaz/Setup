//! Platform abstraction layer.
//!
//! The rest of the runtime never branches on the OS; it calls the functions
//! re-exported here, implemented once per platform in `linux.rs` and
//! `windows.rs`. Every change outside the installation directory is recorded
//! in the transaction [`Journal`] (for rollback) and returned as an
//! [`External`] entry (for uninstall).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::journal::{Journal, Undo};
use crate::manifest::External;

#[cfg(unix)]
mod linux;
#[cfg(unix)]
pub use linux::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

/// Registry hive codes used in journals and manifests.
pub const HKCU: u8 = 0;
pub const HKLM: u8 = 1;

/// Well-known folders (mirrors the model's `KnownFolder`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Folder {
    Install,
    Home,
    UserConfig,
    UserData,
    MachineData,
    Temp,
    Desktop,
}

/// Product information for the OS "installed programs" registry.
pub struct ProductRecord<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub publisher: &'a str,
    pub version: &'a str,
    pub install_dir: &'a Path,
    pub uninstaller: &'a Path,
    pub icon: Option<&'a Path>,
    pub size_bytes: u64,
    pub homepage: Option<&'a str>,
    pub support_url: Option<&'a str>,
    pub machine: bool,
}

/// A launcher entry (Start menu, application menu, desktop, autostart).
pub struct ShortcutSpec<'a> {
    /// Product id, used for file names of `.desktop` entries.
    pub id: &'a str,
    pub name: &'a str,
    pub description: &'a str,
    pub target: &'a Path,
    pub args: &'a [&'a str],
    pub working_dir: &'a Path,
    pub icon: Option<&'a Path>,
    pub machine: bool,
}

/// Writes `bytes` to a file outside the installation directory, journaling
/// the change (backing up an existing file) and recording it for uninstall.
pub fn write_external_file(
    journal: &mut Journal,
    external: &mut Vec<External>,
    path: &Path,
    bytes: &[u8],
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        create_dirs_journaled(journal, parent)?;
    }
    if inst_fsx::dirs::check_not_symlink(path)? {
        let backup = journal.backup_path(path);
        fs::rename(path, &backup)?;
        journal.record(Undo::RestoreFile {
            target: path.to_path_buf(),
            backup,
        })?;
    } else {
        journal.record(Undo::RemoveFile(path.to_path_buf()))?;
    }
    inst_fsx::write_atomic(path, bytes, false)?;
    if !external
        .iter()
        .any(|e| matches!(e, External::File(p) if p == path))
    {
        external.push(External::File(path.to_path_buf()));
    }
    Ok(())
}

/// Creates `dir` and missing parents, journaling each created directory.
pub fn create_dirs_journaled(journal: &mut Journal, dir: &Path) -> io::Result<()> {
    for created in inst_fsx::dirs::create_root(dir)? {
        journal.record(Undo::RemoveDir(created))?;
    }
    Ok(())
}

/// Removes a file recorded as [`External::File`]; missing files are fine.
pub fn remove_external_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Path of the product registry entry (Linux) or its moral equivalent.
pub fn product_state_path(id: &str, machine: bool) -> PathBuf {
    runtime_state_dir(machine).join("products").join(id)
}
