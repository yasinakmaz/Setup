//! Windows implementation: known folders, the Uninstall registry key,
//! Start menu shortcuts (IShellLinkW), per-user PATH and autostart.

#![allow(unsafe_code)]

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HANDLE};
use windows_sys::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, KEY_WRITE, REG_DWORD,
    REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW,
    RegDeleteTreeW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::UI::Shell::{
    FOLDERID_CommonPrograms, FOLDERID_Desktop, FOLDERID_LocalAppData, FOLDERID_Profile,
    FOLDERID_ProgramData, FOLDERID_ProgramFiles, FOLDERID_Programs, FOLDERID_RoamingAppData,
    FOLDERID_Startup, SHGetKnownFolderPath,
};

use super::{Folder, HKCU, HKLM, ProductRecord, ShortcutSpec};
use crate::journal::{Journal, Undo, UndoPlatform};
use crate::manifest::External;

pub(crate) fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

fn wide_str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn known(id: &windows_sys::core::GUID) -> Option<PathBuf> {
    let mut out: windows_sys::core::PWSTR = std::ptr::null_mut();
    // SAFETY: valid GUID pointer, null token (current user), out pointer to
    // a PWSTR that the shell allocates; freed with CoTaskMemFree below.
    let hr = unsafe { SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut out) };
    if hr < 0 || out.is_null() {
        return None;
    }
    // SAFETY: SHGetKnownFolderPath returned a NUL-terminated wide string.
    let path = unsafe {
        let mut len = 0usize;
        while *out.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(out, len);
        PathBuf::from(OsString::from_wide(slice))
    };
    // SAFETY: `out` was allocated by the shell with CoTaskMemAlloc.
    unsafe { windows_sys::Win32::System::Com::CoTaskMemFree(out as *const _) };
    Some(path)
}

fn local_app_data() -> PathBuf {
    known(&FOLDERID_LocalAppData)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
}

pub fn home_dir() -> PathBuf {
    known(&FOLDERID_Profile)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
}

pub fn install_base(machine: bool) -> io::Result<PathBuf> {
    if machine {
        known(&FOLDERID_ProgramFiles).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "Program Files folder not found")
        })
    } else {
        Ok(local_app_data().join("Programs"))
    }
}

pub fn runtime_state_dir(machine: bool) -> PathBuf {
    let base = if machine {
        known(&FOLDERID_ProgramData).unwrap_or_else(local_app_data)
    } else {
        local_app_data()
    };
    base.join(inst_brand::RUNTIME_DIR_NAME)
}

pub fn cache_dir() -> PathBuf {
    local_app_data()
        .join(inst_brand::RUNTIME_DIR_NAME)
        .join("cache")
}

pub fn known_folder(folder: Folder, machine: bool, install_dir: &Path) -> PathBuf {
    match folder {
        Folder::Install => install_dir.to_path_buf(),
        Folder::Home => home_dir(),
        Folder::UserConfig => known(&FOLDERID_RoamingAppData).unwrap_or_else(local_app_data),
        Folder::UserData => local_app_data(),
        Folder::MachineData => {
            if machine {
                known(&FOLDERID_ProgramData).unwrap_or_else(local_app_data)
            } else {
                local_app_data()
            }
        }
        Folder::Temp => std::env::temp_dir(),
        Folder::Desktop => known(&FOLDERID_Desktop).unwrap_or_else(|| home_dir().join("Desktop")),
    }
}

pub fn is_elevated() -> bool {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: querying our own process token with valid out pointers.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

pub fn has_display() -> bool {
    true
}

// ---------------------------------------------------------------- registry

pub(crate) struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by us and is closed exactly once.
        unsafe { RegCloseKey(self.0) };
    }
}

fn root(hive: u8) -> HKEY {
    if hive == HKLM {
        HKEY_LOCAL_MACHINE
    } else {
        HKEY_CURRENT_USER
    }
}

fn check(code: u32) -> io::Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}

pub(crate) fn open_key(hive: u8, path: &str, write: bool) -> io::Result<Option<Key>> {
    let mut key: HKEY = std::ptr::null_mut();
    let access = if write {
        KEY_READ | KEY_WRITE
    } else {
        KEY_READ
    } | KEY_WOW64_64KEY;
    let p = wide_str(path);
    // SAFETY: valid NUL-terminated path and out pointer.
    let code = unsafe { RegOpenKeyExW(root(hive), p.as_ptr(), 0, access, &mut key) };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check(code)?;
    Ok(Some(Key(key)))
}

/// Creates (or opens) a key; returns whether it was newly created.
pub(crate) fn create_key(hive: u8, path: &str) -> io::Result<(Key, bool)> {
    let mut key: HKEY = std::ptr::null_mut();
    let mut disposition = 0u32;
    let p = wide_str(path);
    // SAFETY: valid NUL-terminated path, null class/security, valid outs.
    let code = unsafe {
        RegCreateKeyExW(
            root(hive),
            p.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE | KEY_WOW64_64KEY,
            std::ptr::null(),
            &mut key,
            &mut disposition,
        )
    };
    check(code)?;
    // REG_CREATED_NEW_KEY = 1
    Ok((Key(key), disposition == 1))
}

impl Key {
    pub(crate) fn get_raw(&self, name: &str) -> io::Result<Option<(u32, Vec<u8>)>> {
        let n = wide_str(name);
        let mut ty: REG_VALUE_TYPE = 0;
        let mut len = 0u32;
        // SAFETY: size query with null data pointer.
        let code = unsafe {
            RegQueryValueExW(
                self.0,
                n.as_ptr(),
                std::ptr::null(),
                &mut ty,
                std::ptr::null_mut(),
                &mut len,
            )
        };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(code)?;
        let mut data = vec![0u8; len as usize];
        // SAFETY: buffer of `len` bytes.
        let code = unsafe {
            RegQueryValueExW(
                self.0,
                n.as_ptr(),
                std::ptr::null(),
                &mut ty,
                data.as_mut_ptr(),
                &mut len,
            )
        };
        check(code)?;
        data.truncate(len as usize);
        Ok(Some((ty, data)))
    }

    pub(crate) fn get_string(&self, name: &str) -> io::Result<Option<String>> {
        Ok(self.get_raw(name)?.and_then(|(ty, data)| {
            (ty == REG_SZ || ty == REG_EXPAND_SZ).then(|| {
                let wide: Vec<u16> = data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .collect();
                let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
                String::from_utf16_lossy(&wide[..end])
            })
        }))
    }

    pub(crate) fn set_raw(&self, name: &str, ty: u32, data: &[u8]) -> io::Result<()> {
        let n = wide_str(name);
        // SAFETY: valid name and data buffer.
        check(unsafe {
            RegSetValueExW(self.0, n.as_ptr(), 0, ty, data.as_ptr(), data.len() as u32)
        })
    }

    pub(crate) fn set_string(&self, name: &str, value: &str, expand: bool) -> io::Result<()> {
        let bytes: Vec<u8> = wide_str(value)
            .iter()
            .flat_map(|c| c.to_le_bytes())
            .collect();
        self.set_raw(name, if expand { REG_EXPAND_SZ } else { REG_SZ }, &bytes)
    }

    pub(crate) fn delete_value(&self, name: &str) -> io::Result<()> {
        let n = wide_str(name);
        // SAFETY: valid name.
        let code = unsafe { RegDeleteValueW(self.0, n.as_ptr()) };
        if code == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            check(code)
        }
    }
}

pub(crate) fn delete_tree(hive: u8, path: &str) -> io::Result<()> {
    let (parent, leaf) = path.rsplit_once('\\').unwrap_or(("", path));
    let Some(key) = open_key(hive, parent, true)? else {
        return Ok(());
    };
    let l = wide_str(leaf);
    // SAFETY: valid key and NUL-terminated subkey.
    let code = unsafe { RegDeleteTreeW(key.0, l.as_ptr()) };
    if code == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        check(code)
    }
}

/// Sets a value, journaling its previous state.
pub(crate) fn set_value_journaled(
    journal: &mut Journal,
    hive: u8,
    key_path: &str,
    name: &str,
    ty: u32,
    data: &[u8],
) -> io::Result<()> {
    let (key, created) = create_key(hive, key_path)?;
    if created {
        journal.record(Undo::RegistryKey {
            hive,
            key: key_path.to_owned(),
        })?;
    } else {
        let previous = key.get_raw(name)?;
        journal.record(Undo::RegistryValue {
            hive,
            key: key_path.to_owned(),
            name: name.to_owned(),
            previous,
        })?;
    }
    key.set_raw(name, ty, data)
}

fn sz(value: &str) -> Vec<u8> {
    wide_str(value)
        .iter()
        .flat_map(|c| c.to_le_bytes())
        .collect()
}

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

fn uninstall_key(id: &str) -> String {
    format!(r"{UNINSTALL_KEY}\{id}")
}

/// Registers the product in "Installed apps" (Add/Remove Programs).
pub fn register_product(
    rec: &ProductRecord<'_>,
    journal: &mut Journal,
    external: &mut Vec<External>,
) -> io::Result<()> {
    let hive = if rec.machine { HKLM } else { HKCU };
    let key = uninstall_key(rec.id);
    let uninstaller = rec.uninstaller.to_string_lossy();
    let date = {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let (y, m, d) = civil(secs / 86_400);
        format!("{y:04}{m:02}{d:02}")
    };
    let strings: Vec<(&str, String)> = [
        ("DisplayName", rec.name.to_owned()),
        ("DisplayVersion", rec.version.to_owned()),
        ("Publisher", rec.publisher.to_owned()),
        (
            "InstallLocation",
            rec.install_dir.to_string_lossy().into_owned(),
        ),
        ("UninstallString", format!("\"{uninstaller}\" --uninstall")),
        (
            "QuietUninstallString",
            format!("\"{uninstaller}\" --uninstall --silent"),
        ),
        ("InstallDate", date),
    ]
    .into_iter()
    .chain(
        rec.icon
            .map(|i| ("DisplayIcon", i.to_string_lossy().into_owned())),
    )
    .chain(rec.homepage.map(|h| ("URLInfoAbout", h.to_owned())))
    .chain(rec.support_url.map(|h| ("HelpLink", h.to_owned())))
    .collect();
    for (name, value) in &strings {
        set_value_journaled(journal, hive, &key, name, REG_SZ, &sz(value))?;
    }
    let size_kb = u32::try_from(rec.size_bytes / 1024).unwrap_or(u32::MAX);
    for (name, value) in [("EstimatedSize", size_kb), ("NoModify", 1), ("NoRepair", 1)] {
        set_value_journaled(journal, hive, &key, name, REG_DWORD, &value.to_le_bytes())?;
    }
    external.push(External::RegistryKey { hive, key });
    Ok(())
}

pub fn find_installed(id: &str, machine: bool) -> Option<PathBuf> {
    let hive = if machine { HKLM } else { HKCU };
    let key = open_key(hive, &uninstall_key(id), false).ok()??;
    key.get_string("InstallLocation").ok()?.map(PathBuf::from)
}

// --------------------------------------------------------------- shortcuts

fn create_lnk(spec: &ShortcutSpec<'_>, lnk: &Path) -> io::Result<()> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        IPersistFile,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HSTRING, Interface};

    let to_io = |e: windows::core::Error| io::Error::other(e.to_string());
    // SAFETY: COM calls on this thread; CoInitializeEx may return S_FALSE
    // or RPC_E_CHANGED_MODE when already initialized, both are fine.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(to_io)?;
        link.SetPath(&HSTRING::from(spec.target.as_os_str()))
            .map_err(to_io)?;
        let args = spec
            .args
            .iter()
            .map(|a| quote_arg(a))
            .collect::<Vec<_>>()
            .join(" ");
        link.SetArguments(&HSTRING::from(args)).map_err(to_io)?;
        link.SetWorkingDirectory(&HSTRING::from(spec.working_dir.as_os_str()))
            .map_err(to_io)?;
        link.SetDescription(&HSTRING::from(spec.description))
            .map_err(to_io)?;
        let icon = spec.icon.unwrap_or(spec.target);
        link.SetIconLocation(&HSTRING::from(icon.as_os_str()), 0)
            .map_err(to_io)?;
        let file: IPersistFile = link.cast().map_err(to_io)?;
        file.Save(&HSTRING::from(lnk.as_os_str()), true)
            .map_err(to_io)?;
    }
    Ok(())
}

/// Quotes one command-line argument using the MSVCRT rules.
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                out.push(c);
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

fn write_lnk(
    spec: &ShortcutSpec<'_>,
    path: &Path,
    journal: &mut Journal,
    external: &mut Vec<External>,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        super::create_dirs_journaled(journal, parent)?;
    }
    if path.exists() {
        let backup = journal.backup_path(path);
        std::fs::rename(path, &backup)?;
        journal.record(Undo::RestoreFile {
            target: path.to_path_buf(),
            backup,
        })?;
    } else {
        journal.record(Undo::RemoveFile(path.to_path_buf()))?;
    }
    create_lnk(spec, path)?;
    external.push(External::File(path.to_path_buf()));
    Ok(())
}

fn lnk_name(spec: &ShortcutSpec<'_>) -> String {
    let safe: String = spec
        .name
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    format!("{safe}.lnk")
}

pub fn create_menu_entry(
    spec: &ShortcutSpec<'_>,
    journal: &mut Journal,
    external: &mut Vec<External>,
) -> io::Result<()> {
    let dir = if spec.machine {
        known(&FOLDERID_CommonPrograms)
    } else {
        known(&FOLDERID_Programs)
    }
    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Start menu folder not found"))?;
    write_lnk(spec, &dir.join(lnk_name(spec)), journal, external)
}

pub fn create_desktop_shortcut(
    spec: &ShortcutSpec<'_>,
    journal: &mut Journal,
    external: &mut Vec<External>,
) -> io::Result<()> {
    let dir = known(&FOLDERID_Desktop).unwrap_or_else(|| home_dir().join("Desktop"));
    write_lnk(spec, &dir.join(lnk_name(spec)), journal, external)
}

pub fn add_autostart(
    spec: &ShortcutSpec<'_>,
    journal: &mut Journal,
    external: &mut Vec<External>,
) -> io::Result<()> {
    if spec.machine {
        let dir = known(&FOLDERID_Startup).unwrap_or_else(home_dir);
        return write_lnk(spec, &dir.join(lnk_name(spec)), journal, external);
    }
    let key = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let mut command = quote_arg(&spec.target.to_string_lossy());
    for a in spec.args {
        command.push(' ');
        command.push_str(&quote_arg(a));
    }
    set_value_journaled(journal, HKCU, key, spec.id, REG_SZ, &sz(&command))?;
    external.push(External::RegistryValue {
        hive: HKCU,
        key: key.to_owned(),
        name: spec.id.to_owned(),
    });
    Ok(())
}

fn environment_key(machine: bool) -> (u8, &'static str) {
    if machine {
        (
            HKLM,
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
        )
    } else {
        (HKCU, "Environment")
    }
}

fn broadcast_environment_change() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };
    let env = wide_str("Environment");
    let mut result = 0usize;
    // SAFETY: broadcasting a settings change with a valid string pointer.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            env.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            2000,
            &mut result,
        );
    }
}

/// Appends the directory of `exe` to the user (or machine) `Path`.
pub fn add_to_path(
    exe: &Path,
    machine: bool,
    journal: &mut Journal,
    external: &mut Vec<External>,
) -> io::Result<()> {
    let dir = exe.parent().unwrap_or(exe).to_string_lossy().into_owned();
    let (hive, key_path) = environment_key(machine);
    let (key, _) = create_key(hive, key_path)?;
    let current = key.get_string("Path")?;
    let already = current
        .as_deref()
        .is_some_and(|p| p.split(';').any(|e| e.eq_ignore_ascii_case(&dir)));
    if already {
        return Ok(());
    }
    journal.record(Undo::EnvVar {
        machine,
        name: "Path".to_owned(),
        previous: current.clone(),
    })?;
    let new = match current.as_deref() {
        Some(p) if !p.is_empty() => format!("{};{dir}", p.trim_end_matches(';')),
        _ => dir.clone(),
    };
    key.set_string("Path", &new, true)?;
    broadcast_environment_change();
    external.push(External::PathEntry {
        machine,
        name: "Path".to_owned(),
        entry: dir,
    });
    Ok(())
}

pub fn remove_external(e: &External) -> io::Result<()> {
    match e {
        External::File(p) => super::remove_external_file(p),
        External::RegistryKey { hive, key } => delete_tree(*hive, key),
        External::RegistryValue { hive, key, name } => match open_key(*hive, key, true)? {
            Some(k) => k.delete_value(name),
            None => Ok(()),
        },
        External::PathEntry {
            machine,
            name,
            entry,
        } => {
            let (hive, key_path) = environment_key(*machine);
            let Some(key) = open_key(hive, key_path, true)? else {
                return Ok(());
            };
            if let Some(current) = key.get_string(name)? {
                let kept: Vec<&str> = current
                    .split(';')
                    .filter(|e| !e.is_empty() && !e.eq_ignore_ascii_case(entry))
                    .collect();
                key.set_string(name, &kept.join(";"), true)?;
                broadcast_environment_change();
            }
            Ok(())
        }
        External::EnvVar { machine, name } => {
            let (hive, key_path) = environment_key(*machine);
            if let Some(key) = open_key(hive, key_path, true)? {
                key.delete_value(name)?;
                broadcast_environment_change();
            }
            Ok(())
        }
        External::Service { name, user } => crate::services::remove(name, *user),
    }
}

// ---------------------------------------------------------------- process

const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

pub fn launch_detached(program: &Path, args: &[&str], cwd: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map(drop)
}

pub fn open_path(path: &Path) -> io::Result<()> {
    launch_detached(
        Path::new("explorer.exe"),
        &[&path.to_string_lossy()],
        Path::new("C:\\"),
    )
}

/// Clears the Authenticode directory of a copied executable: the uninstaller
/// is the runtime without its payload and signature, and a dangling
/// certificate pointer would make it look tampered.
pub fn finalize_uninstaller(dest: &Path) -> io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dest)?;
    let len = f.metadata()?.len();
    // Locate the security directory entry and zero it if it points past EOF.
    let mut dos = [0u8; 0x40];
    use std::io::Read;
    f.seek(SeekFrom::Start(0))?;
    f.read_exact(&mut dos)?;
    let e_lfanew = u64::from(u32::from_le_bytes([
        dos[0x3c], dos[0x3d], dos[0x3e], dos[0x3f],
    ]));
    let mut magic = [0u8; 2];
    f.seek(SeekFrom::Start(e_lfanew + 24))?;
    f.read_exact(&mut magic)?;
    let dirs = match u16::from_le_bytes(magic) {
        0x10b => 96u64,
        0x20b => 112,
        _ => return Ok(()),
    };
    let entry = e_lfanew + 24 + dirs + 4 * 8;
    let mut d = [0u8; 8];
    f.seek(SeekFrom::Start(entry))?;
    f.read_exact(&mut d)?;
    let offset = u64::from(u32::from_le_bytes([d[0], d[1], d[2], d[3]]));
    if offset != 0 && offset >= len {
        f.seek(SeekFrom::Start(entry))?;
        f.write_all(&[0u8; 8])?;
    }
    Ok(())
}

/// Schedules deletion of a file (the temporary uninstaller copy) at reboot.
pub fn delete_on_reboot(path: &Path) {
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};
    let p = wide(path.as_os_str());
    // SAFETY: valid path; null destination means delete.
    unsafe { MoveFileExW(p.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) };
}

pub struct Platform;

impl UndoPlatform for Platform {
    fn undo_registry_value(
        &self,
        hive: u8,
        key: &str,
        name: &str,
        previous: Option<&(u32, Vec<u8>)>,
    ) -> io::Result<()> {
        let Some(k) = open_key(hive, key, true)? else {
            return Ok(());
        };
        match previous {
            Some((ty, data)) => k.set_raw(name, *ty, data),
            None => k.delete_value(name),
        }
    }

    fn undo_registry_key(&self, hive: u8, key: &str) -> io::Result<()> {
        delete_tree(hive, key)
    }

    fn undo_env_var(&self, machine: bool, name: &str, previous: Option<&str>) -> io::Result<()> {
        let (hive, key_path) = environment_key(machine);
        let Some(key) = open_key(hive, key_path, true)? else {
            return Ok(());
        };
        match previous {
            Some(v) => key.set_string(name, v, true)?,
            None => key.delete_value(name)?,
        }
        broadcast_environment_change();
        Ok(())
    }

    fn undo_service(&self, name: &str, user: bool) -> io::Result<()> {
        crate::services::remove(name, user)
    }
}

fn civil(days: u64) -> (i64, u32, u32) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (
        if m <= 2 {
            yoe + era * 400 + 1
        } else {
            yoe + era * 400
        },
        m,
        d,
    )
}
