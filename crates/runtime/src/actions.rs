//! Typed system actions: services, registry values, environment variables.

use inst_log::log;

use crate::context::Install;
use crate::error::{ErrorKind, InstallError};
#[cfg(windows)]
use crate::manifest::External;
use crate::spec::{EnvOp, EnvWrite, OnFailure, RegData, RegistryWrite, Service, Value};

impl Install<'_> {
    fn handle(
        &self,
        id: &str,
        on_failure: OnFailure,
        r: Result<(), InstallError>,
    ) -> Result<(), InstallError> {
        match r {
            Err(e) if on_failure == OnFailure::Continue => {
                log!(self.c.logger, Warn, "action", "action failed; continuing"; "action" => id, "error" => &e.to_string());
                Ok(())
            }
            other => other,
        }
    }

    pub fn install_service(&mut self, s: &Service) -> Result<(), InstallError> {
        let r = self.install_service_inner(s);
        self.handle(s.id, s.on_failure, r)
    }

    fn install_service_inner(&mut self, s: &Service) -> Result<(), InstallError> {
        let executable = inst_fsx::RelPath::new(s.executable)
            .map_err(|e| InstallError::unexpected(e.to_string()))?
            .to_path_under(&self.c.install_dir);
        let mut args = Vec::with_capacity(s.args.len());
        for a in s.args {
            args.push(self.resolve(a)?);
        }
        let spec = crate::services::ServiceSpec {
            name: s.name.to_owned(),
            display_name: s.display_name.to_owned(),
            description: s.description.to_owned(),
            executable,
            args,
            start: s.start,
            user: s.user,
            restart_on_failure: s.restart_on_failure,
            start_after_install: s.start_after_install,
        };
        self.journal()?;
        let mut journal = self.journal.take().expect("journal exists");
        let mut external = std::mem::take(&mut self.external);
        let r = crate::services::install(&spec, &mut journal, &mut external);
        self.journal = Some(journal);
        self.external = external;
        r.map_err(|e| {
            InstallError::new(
                ErrorKind::TaskFailed {
                    name: s.display_name.to_owned(),
                },
                e.to_string(),
            )
        })
    }

    pub fn set_registry(&mut self, w: &RegistryWrite) -> Result<(), InstallError> {
        let r = self.set_registry_inner(w);
        self.handle(w.id, w.on_failure, r)
    }

    #[cfg(windows)]
    fn set_registry_inner(&mut self, w: &RegistryWrite) -> Result<(), InstallError> {
        use windows_sys::Win32::System::Registry::{
            REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_QWORD, REG_SZ,
        };
        let utf16 = |s: &str| -> Vec<u8> {
            s.encode_utf16()
                .chain(Some(0))
                .flat_map(u16::to_le_bytes)
                .collect()
        };
        let (ty, data) = match w.data {
            RegData::Sz(v) => (REG_SZ, utf16(&self.resolve(&v)?)),
            RegData::ExpandSz(v) => (REG_EXPAND_SZ, utf16(&self.resolve(&v)?)),
            RegData::MultiSz(values) => {
                let mut data = Vec::new();
                for v in values {
                    data.extend(utf16(&self.resolve(v)?));
                }
                data.extend([0, 0]);
                (REG_MULTI_SZ, data)
            }
            RegData::Dword(d) => (REG_DWORD, d.to_le_bytes().to_vec()),
            RegData::Qword(q) => (REG_QWORD, q.to_le_bytes().to_vec()),
        };
        let journal = self.journal()?;
        crate::platform::set_value_journaled(journal, w.hive, w.key, w.name, ty, &data)
            .map_err(|e| InstallError::io("writing the registry", &e))?;
        if w.remove_on_uninstall {
            self.external.push(External::RegistryValue {
                hive: w.hive,
                key: w.key.to_owned(),
                name: w.name.to_owned(),
            });
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn set_registry_inner(&mut self, w: &RegistryWrite) -> Result<(), InstallError> {
        let _ = (&w.data, RegData::Dword(0));
        log!(self.c.logger, Debug, "registry", "registry action ignored on this platform"; "action" => w.id);
        Ok(())
    }

    pub fn set_env(&mut self, w: &EnvWrite) -> Result<(), InstallError> {
        let r = self.set_env_inner(w);
        self.handle(w.id, w.on_failure, r)
    }

    #[cfg(windows)]
    fn set_env_inner(&mut self, w: &EnvWrite) -> Result<(), InstallError> {
        use crate::journal::Undo;
        let (hive, key_path) = if w.machine {
            (
                crate::platform::HKLM,
                r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
            )
        } else {
            (crate::platform::HKCU, "Environment")
        };
        let (key, _) = crate::platform::create_key(hive, key_path)
            .map_err(|e| InstallError::io("opening the environment", &e))?;
        let previous = key
            .get_string(w.name)
            .map_err(|e| InstallError::io("reading the environment", &e))?;
        let (value, external) = match w.op {
            EnvOp::Set(v) => (
                self.resolve(&v)?,
                External::EnvVar {
                    machine: w.machine,
                    name: w.name.to_owned(),
                },
            ),
            EnvOp::Append(folder, rel) | EnvOp::Prepend(folder, rel) => {
                let entry = self.resolve(&Value::Path(folder, rel))?;
                let current = previous.clone().unwrap_or_default();
                let joined = if current.is_empty() {
                    entry.clone()
                } else if matches!(w.op, EnvOp::Append(..)) {
                    format!("{};{entry}", current.trim_end_matches(';'))
                } else {
                    format!("{entry};{current}")
                };
                (
                    joined,
                    External::PathEntry {
                        machine: w.machine,
                        name: w.name.to_owned(),
                        entry,
                    },
                )
            }
        };
        self.record(Undo::EnvVar {
            machine: w.machine,
            name: w.name.to_owned(),
            previous,
        })?;
        key.set_string(w.name, &value, true)
            .map_err(|e| InstallError::io("writing the environment", &e))?;
        self.external.push(external);
        Ok(())
    }

    /// On Linux there is no persistent, session-independent environment
    /// store; PATH integration uses `~/.local/bin` symlinks instead.
    #[cfg(not(windows))]
    fn set_env_inner(&mut self, w: &EnvWrite) -> Result<(), InstallError> {
        let _ = matches!(w.op, EnvOp::Set(Value::Lit(_)));
        log!(self.c.logger, Warn, "environment",
            "environment variable actions are not supported on Linux; use PATH integration"; "action" => w.id);
        Ok(())
    }
}
