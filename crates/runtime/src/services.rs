//! Native service installation: Windows Service Control Manager and systemd
//! (system units, or user units that need no root).

use std::io;
use std::path::PathBuf;

use crate::journal::{Journal, Undo};
use crate::manifest::External;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartMode {
    Automatic,
    Manual,
    Disabled,
}

#[derive(Clone, Debug)]
pub struct ServiceSpec {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub start: StartMode,
    /// systemd user unit (Linux only).
    pub user: bool,
    pub restart_on_failure: bool,
    pub start_after_install: bool,
}

/// Validates a service name: ASCII letters, digits, `-`, `_`, `.`, `@`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'))
}

pub fn install(
    spec: &ServiceSpec,
    journal: &mut Journal,
    external: &mut Vec<External>,
) -> io::Result<()> {
    if !valid_name(&spec.name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid service name",
        ));
    }
    journal.record(Undo::Service {
        name: spec.name.clone(),
        user: spec.user,
    })?;
    sys::install(spec, journal, external)?;
    external.push(External::Service {
        name: spec.name.clone(),
        user: spec.user,
    });
    Ok(())
}

pub fn remove(name: &str, user: bool) -> io::Result<()> {
    if !valid_name(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid service name",
        ));
    }
    sys::remove(name, user)
}

#[cfg(unix)]
mod sys {
    use super::*;
    use std::process::{Command, Stdio};

    fn unit_dir(user: bool) -> PathBuf {
        if user {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| crate::platform::home_dir().join(".config"))
                .join("systemd/user")
        } else {
            PathBuf::from("/etc/systemd/system")
        }
    }

    fn systemctl(user: bool, args: &[&str]) -> io::Result<bool> {
        let mut cmd = Command::new("systemctl");
        if user {
            cmd.arg("--user");
        }
        let status = cmd
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        Ok(status.success())
    }

    /// Quotes a word for `ExecStart=` (systemd.syntax: C-style escapes in
    /// double quotes, `%` doubled, `$` doubled to suppress expansion).
    pub(super) fn exec_word(word: &str) -> String {
        let mut out = String::with_capacity(word.len() + 2);
        out.push('"');
        for c in word.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '%' => out.push_str("%%"),
                '$' => out.push_str("$$"),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    pub fn install(
        spec: &ServiceSpec,
        journal: &mut Journal,
        external: &mut Vec<External>,
    ) -> io::Result<()> {
        let mut exec = exec_word(&spec.executable.to_string_lossy());
        for a in &spec.args {
            exec.push(' ');
            exec.push_str(&exec_word(a));
        }
        let wd = spec
            .executable
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let unit = format!(
            "[Unit]\nDescription={}\n\n[Service]\nType=simple\nExecStart={exec}\nWorkingDirectory={}\nRestart={}\n\n[Install]\nWantedBy={}\n",
            spec.display_name.replace('\n', " "),
            exec_word(&wd),
            if spec.restart_on_failure {
                "on-failure"
            } else {
                "no"
            },
            if spec.user {
                "default.target"
            } else {
                "multi-user.target"
            },
        );
        let path = unit_dir(spec.user).join(format!("{}.service", spec.name));
        crate::platform::write_external_file(journal, external, &path, unit.as_bytes())?;
        systemctl(spec.user, &["daemon-reload"])?;
        let unit_name = format!("{}.service", spec.name);
        if spec.start == StartMode::Automatic && !systemctl(spec.user, &["enable", &unit_name])? {
            return Err(io::Error::other(format!(
                "systemctl enable {unit_name} failed"
            )));
        }
        if spec.start_after_install
            && spec.start != StartMode::Disabled
            && !systemctl(spec.user, &["start", &unit_name])?
        {
            return Err(io::Error::other(format!(
                "systemctl start {unit_name} failed"
            )));
        }
        Ok(())
    }

    pub fn remove(name: &str, user: bool) -> io::Result<()> {
        let unit_name = format!("{name}.service");
        let _ = systemctl(user, &["disable", "--now", &unit_name]);
        let path = unit_dir(user).join(&unit_name);
        crate::platform::remove_external_file(&path)?;
        let _ = systemctl(user, &["daemon-reload"]);
        Ok(())
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod sys {
    use super::*;
    use crate::platform::quote_arg;
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, ControlService, CreateServiceW, DeleteService, OpenSCManagerW,
        OpenServiceW, SC_HANDLE, SC_MANAGER_ALL_ACCESS, SERVICE_ALL_ACCESS, SERVICE_AUTO_START,
        SERVICE_CONTROL_STOP, SERVICE_DEMAND_START, SERVICE_DISABLED, SERVICE_ERROR_NORMAL,
        SERVICE_STATUS, SERVICE_WIN32_OWN_PROCESS, StartServiceW,
    };

    struct Handle(SC_HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: handle owned by us, closed once.
            unsafe { CloseServiceHandle(self.0) };
        }
    }

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn manager() -> io::Result<Handle> {
        // SAFETY: local machine, default database.
        let h =
            unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_ALL_ACCESS) };
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Handle(h))
    }

    pub fn install(
        spec: &ServiceSpec,
        _journal: &mut Journal,
        _external: &mut Vec<External>,
    ) -> io::Result<()> {
        let scm = manager()?;
        let mut command = quote_arg(&spec.executable.to_string_lossy());
        for a in &spec.args {
            command.push(' ');
            command.push_str(&quote_arg(a));
        }
        let start = match spec.start {
            StartMode::Automatic => SERVICE_AUTO_START,
            StartMode::Manual => SERVICE_DEMAND_START,
            StartMode::Disabled => SERVICE_DISABLED,
        };
        let (name, display, cmd) = (w(&spec.name), w(&spec.display_name), w(&command));
        // SAFETY: all strings are NUL-terminated; optional params are null.
        let svc = unsafe {
            CreateServiceW(
                scm.0,
                name.as_ptr(),
                display.as_ptr(),
                SERVICE_ALL_ACCESS,
                SERVICE_WIN32_OWN_PROCESS,
                start,
                SERVICE_ERROR_NORMAL,
                cmd.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if svc.is_null() {
            return Err(io::Error::last_os_error());
        }
        let svc = Handle(svc);
        if spec.start_after_install && spec.start != StartMode::Disabled {
            // SAFETY: valid service handle, no arguments.
            if unsafe { StartServiceW(svc.0, 0, std::ptr::null()) } == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    pub fn remove(name: &str, _user: bool) -> io::Result<()> {
        let scm = manager()?;
        let n = w(name);
        // SAFETY: valid manager handle and name.
        let svc = unsafe { OpenServiceW(scm.0, n.as_ptr(), SERVICE_ALL_ACCESS) };
        if svc.is_null() {
            return Ok(()); // already gone
        }
        let svc = Handle(svc);
        let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        // SAFETY: valid handle and status buffer; failure (not running) is fine.
        unsafe { ControlService(svc.0, SERVICE_CONTROL_STOP, &mut status) };
        // SAFETY: valid handle.
        if unsafe { DeleteService(svc.0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn systemd_quoting() {
        assert_eq!(super::sys::exec_word("/opt/a b/x"), "\"/opt/a b/x\"");
        assert_eq!(
            super::sys::exec_word("100%$HOME\"\\"),
            "\"100%%$$HOME\\\"\\\\\""
        );
    }
}
