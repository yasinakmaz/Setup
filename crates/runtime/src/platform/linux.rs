//! Linux implementation: XDG base directories, desktop entries, user-local
//! installation without root.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{Folder, ProductRecord, ShortcutSpec, write_external_file};
use crate::journal::{Journal, Undo, UndoPlatform};
use crate::manifest::External;

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

pub fn home_dir() -> PathBuf {
    env_path("HOME").unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn xdg_data_home() -> PathBuf {
    env_path("XDG_DATA_HOME").unwrap_or_else(|| home_dir().join(".local/share"))
}

fn xdg_config_home() -> PathBuf {
    env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home_dir().join(".config"))
}

fn xdg_cache_home() -> PathBuf {
    env_path("XDG_CACHE_HOME").unwrap_or_else(|| home_dir().join(".cache"))
}

/// Base directory under which products are installed.
pub fn install_base(machine: bool) -> io::Result<PathBuf> {
    Ok(if machine {
        PathBuf::from("/opt")
    } else {
        xdg_data_home()
    })
}

pub fn runtime_state_dir(machine: bool) -> PathBuf {
    if machine {
        PathBuf::from("/var/lib").join(inst_brand::RUNTIME_DIR_NAME)
    } else {
        xdg_data_home().join(inst_brand::RUNTIME_DIR_NAME)
    }
}

pub fn cache_dir() -> PathBuf {
    xdg_cache_home().join(inst_brand::RUNTIME_DIR_NAME)
}

/// `XDG_DESKTOP_DIR` from `user-dirs.dirs`, falling back to `~/Desktop`.
fn desktop_dir() -> PathBuf {
    let home = home_dir();
    if let Ok(text) = fs::read_to_string(xdg_config_home().join("user-dirs.dirs")) {
        for line in text.lines() {
            if let Some(v) = line.trim().strip_prefix("XDG_DESKTOP_DIR=") {
                let v = v.trim().trim_matches('"');
                let v = v.replace("$HOME", &home.to_string_lossy());
                let p = PathBuf::from(v);
                if p.is_absolute() {
                    return p;
                }
            }
        }
    }
    home.join("Desktop")
}

pub fn known_folder(folder: Folder, machine: bool, install_dir: &Path) -> PathBuf {
    match folder {
        Folder::Install => install_dir.to_path_buf(),
        Folder::Home => home_dir(),
        Folder::UserConfig => xdg_config_home(),
        Folder::UserData => xdg_data_home(),
        Folder::MachineData => {
            if machine {
                PathBuf::from("/var/lib")
            } else {
                xdg_data_home()
            }
        }
        Folder::Temp => std::env::temp_dir(),
        Folder::Desktop => desktop_dir(),
    }
}

#[allow(unsafe_code)]
pub fn is_elevated() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

pub fn has_display() -> bool {
    std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

// ------------------------------------------------------ product registry

pub fn register_product(
    rec: &ProductRecord<'_>,
    journal: &mut Journal,
    external: &mut Vec<External>,
) -> io::Result<()> {
    let path = super::product_state_path(rec.id, rec.machine);
    write_external_file(
        journal,
        external,
        &path,
        rec.install_dir.as_os_str().as_encoded_bytes(),
    )
}

/// Installation directory of an installed product, if any.
pub fn find_installed(id: &str, machine: bool) -> Option<PathBuf> {
    let bytes = fs::read(super::product_state_path(id, machine)).ok()?;
    let p = PathBuf::from(OsString::from_vec(bytes));
    p.is_absolute().then_some(p)
}

// -------------------------------------------------------- desktop entries

/// Escapes a string value for a desktop entry (`\n`, `\t`, `\\`).
fn desktop_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out
}

/// Quotes one argument of an `Exec=` key per the Desktop Entry spec.
fn exec_arg(arg: &str) -> String {
    let needs_quotes = arg.is_empty()
        || arg
            .chars()
            .any(|c| c.is_whitespace() || "\"'\\><~|&;$*?#()`".contains(c));
    let mut out = String::with_capacity(arg.len() + 2);
    if needs_quotes {
        out.push('"');
    }
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' | '\\' if needs_quotes => {
                out.push('\\');
                out.push(c);
            }
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
    }
    if needs_quotes {
        out.push('"');
    }
    // The whole Exec value is itself a string value: escape backslashes.
    out.replace('\\', "\\\\")
}

pub fn desktop_entry(spec: &ShortcutSpec<'_>, extra: &str) -> String {
    let mut exec = exec_arg(&spec.target.to_string_lossy());
    for a in spec.args {
        exec.push(' ');
        exec.push_str(&exec_arg(a));
    }
    let mut s = String::with_capacity(512);
    s.push_str("[Desktop Entry]\nType=Application\nVersion=1.5\n");
    s.push_str(&format!("Name={}\n", desktop_value(spec.name)));
    if !spec.description.is_empty() {
        s.push_str(&format!("Comment={}\n", desktop_value(spec.description)));
    }
    s.push_str(&format!("Exec={exec}\n"));
    s.push_str(&format!(
        "Path={}\n",
        desktop_value(&spec.working_dir.to_string_lossy())
    ));
    if let Some(icon) = spec.icon {
        s.push_str(&format!(
            "Icon={}\n",
            desktop_value(&icon.to_string_lossy())
        ));
    }
    s.push_str("Terminal=false\nCategories=Utility;\n");
    s.push_str(extra);
    s
}

fn applications_dir(machine: bool) -> PathBuf {
    if machine {
        PathBuf::from("/usr/share/applications")
    } else {
        xdg_data_home().join("applications")
    }
}

fn refresh_desktop_database(dir: &Path) {
    // Best effort; absence of the tool is fine.
    let _ = Command::new("update-desktop-database")
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Application menu entry (`~/.local/share/applications/<id>.desktop`).
pub fn create_menu_entry(
    spec: &ShortcutSpec<'_>,
    journal: &mut Journal,
    external: &mut Vec<External>,
) -> io::Result<()> {
    let dir = applications_dir(spec.machine);
    let path = dir.join(format!("{}.desktop", spec.id));
    write_external_file(journal, external, &path, desktop_entry(spec, "").as_bytes())?;
    refresh_desktop_database(&dir);
    Ok(())
}

pub fn create_desktop_shortcut(
    spec: &ShortcutSpec<'_>,
    journal: &mut Journal,
    external: &mut Vec<External>,
) -> io::Result<()> {
    let path = desktop_dir().join(format!("{}.desktop", spec.id));
    write_external_file(journal, external, &path, desktop_entry(spec, "").as_bytes())?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    // GNOME requires the entry to be marked trusted; best effort.
    let _ = Command::new("gio")
        .args(["set", "-t", "string"])
        .arg(&path)
        .args(["metadata::trusted", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(())
}

pub fn add_autostart(
    spec: &ShortcutSpec<'_>,
    journal: &mut Journal,
    external: &mut Vec<External>,
) -> io::Result<()> {
    let path = xdg_config_home()
        .join("autostart")
        .join(format!("{}.desktop", spec.id));
    write_external_file(
        journal,
        external,
        &path,
        desktop_entry(spec, "X-GNOME-Autostart-enabled=true\n").as_bytes(),
    )
}

/// Makes the main executable available on `PATH` via a symlink in
/// `~/.local/bin` (per user) or `/usr/local/bin` (machine).
pub fn add_to_path(
    exe: &Path,
    machine: bool,
    journal: &mut Journal,
    external: &mut Vec<External>,
) -> io::Result<()> {
    let bin = if machine {
        PathBuf::from("/usr/local/bin")
    } else {
        home_dir().join(".local/bin")
    };
    super::create_dirs_journaled(journal, &bin)?;
    let name = exe
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "executable has no name"))?;
    let link = bin.join(name);
    if fs::symlink_metadata(&link).is_ok() {
        let backup = journal.backup_path(&link);
        fs::rename(&link, &backup)?;
        journal.record(Undo::RestoreFile {
            target: link.clone(),
            backup,
        })?;
    } else {
        journal.record(Undo::RemoveFile(link.clone()))?;
    }
    std::os::unix::fs::symlink(exe, &link)?;
    external.push(External::File(link));
    Ok(())
}

pub fn remove_external(e: &External) -> io::Result<()> {
    match e {
        External::File(p) => {
            super::remove_external_file(p)?;
            if p.extension().is_some_and(|x| x == "desktop")
                && let Some(dir) = p.parent()
            {
                refresh_desktop_database(dir);
            }
            Ok(())
        }
        External::Service { name, user } => crate::services::remove(name, *user),
        // Registry and environment entries do not exist on Linux.
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------- process

/// Starts a program detached from the installer.
pub fn launch_detached(program: &Path, args: &[&str], cwd: &Path) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(drop)
}

/// Opens a file or folder with the desktop's default handler.
pub fn open_path(path: &Path) -> io::Result<()> {
    launch_detached(
        Path::new("xdg-open"),
        &[&path.to_string_lossy()],
        Path::new("/"),
    )
}

/// Copies the running executable (without payload) to `dest`.
pub fn finalize_uninstaller(dest: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dest, fs::Permissions::from_mode(0o755))
}

pub struct Platform;

impl UndoPlatform for Platform {
    fn undo_registry_value(
        &self,
        _: u8,
        _: &str,
        _: &str,
        _: Option<&(u32, Vec<u8>)>,
    ) -> io::Result<()> {
        Ok(())
    }
    fn undo_registry_key(&self, _: u8, _: &str) -> io::Result<()> {
        Ok(())
    }
    fn undo_env_var(&self, _: bool, _: &str, _: Option<&str>) -> io::Result<()> {
        Ok(())
    }
    fn undo_service(&self, name: &str, user: bool) -> io::Result<()> {
        crate::services::remove(name, user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_quoting() {
        let spec = ShortcutSpec {
            id: "com.acme.app",
            name: "Acme\nOrders",
            description: "Sipariş yönetimi",
            target: Path::new("/home/u/Acme Orders/bin/acme"),
            args: &["--mode", "100%", "a\"b"],
            working_dir: Path::new("/home/u/Acme Orders"),
            icon: None,
            machine: false,
        };
        let e = desktop_entry(&spec, "");
        assert!(e.contains("Name=Acme\\nOrders\n"), "{e}");
        assert!(
            e.contains(r#"Exec="/home/u/Acme Orders/bin/acme" --mode 100%% "a\\"b""#),
            "{e}"
        );
    }
}
