//! Installation manifest: the record of everything an installation created.
//!
//! Uninstall, repair, upgrade and existing-version detection all work from
//! this manifest; nothing is guessed. It is stored as
//! `<install>/<METADATA_DIR>/manifest` in the compact wire format.

use std::path::{Path, PathBuf};

use inst_wire::{Reader, WireError, Writer};

use crate::app::Scope;

/// Hidden metadata directory inside the installation directory.
pub const METADATA_DIR: &str = ".installer-runtime";
const MAGIC: &[u8; 8] = b"INSTMANI";
const VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestFile {
    /// `/`-separated path relative to the installation directory.
    pub path: String,
    pub size: u64,
    pub hash: [u8; 32],
}

/// A change made outside the installation directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum External {
    /// A file we created (shortcut, `.desktop` entry, icon, autostart entry,
    /// symlink in `~/.local/bin`).
    File(PathBuf),
    /// A registry key we created and own entirely (Windows).
    RegistryKey {
        hive: u8,
        key: String,
    },
    /// A registry value we set in a key we do not own.
    RegistryValue {
        hive: u8,
        key: String,
        name: String,
    },
    /// A directory entry appended to a PATH-like variable.
    PathEntry {
        machine: bool,
        name: String,
        entry: String,
    },
    EnvVar {
        machine: bool,
        name: String,
    },
    Service {
        name: String,
        user: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub product_id: String,
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub install_dir: PathBuf,
    pub scope: Scope,
    pub language: String,
    pub installed_at: u64,
    pub files: Vec<ManifestFile>,
    /// Directories created by the installation, relative, parents first.
    pub dirs: Vec<String>,
    pub external: Vec<External>,
    /// Prerequisites installed by this product (`id`, version). They are not
    /// removed on uninstall because other software may depend on them.
    pub prerequisites: Vec<(String, String)>,
    /// Uninstaller path relative to the installation directory.
    pub uninstaller: Option<String>,
}

impl Manifest {
    pub fn path_in(install_dir: &Path) -> PathBuf {
        install_dir.join(METADATA_DIR).join("manifest")
    }

    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(256 + self.files.len() * 80);
        let mut w = Writer::new(&mut buf);
        w.raw(MAGIC)
            .u16(VERSION)
            .str(&self.product_id)
            .str(&self.name)
            .str(&self.publisher)
            .str(&self.version)
            .str(&self.install_dir.to_string_lossy())
            .u8(match self.scope {
                Scope::User => 0,
                Scope::Machine => 1,
            })
            .str(&self.language)
            .u64(self.installed_at);
        w.varint(self.files.len() as u64);
        for f in &self.files {
            w.str(&f.path).varint(f.size).hash(&f.hash);
        }
        w.varint(self.dirs.len() as u64);
        for d in &self.dirs {
            w.str(d);
        }
        w.varint(self.external.len() as u64);
        for e in &self.external {
            match e {
                External::File(p) => {
                    w.u8(1).str(&p.to_string_lossy());
                }
                External::RegistryKey { hive, key } => {
                    w.u8(2).u8(*hive).str(key);
                }
                External::RegistryValue { hive, key, name } => {
                    w.u8(3).u8(*hive).str(key).str(name);
                }
                External::PathEntry {
                    machine,
                    name,
                    entry,
                } => {
                    w.u8(4).bool(*machine).str(name).str(entry);
                }
                External::EnvVar { machine, name } => {
                    w.u8(5).bool(*machine).str(name);
                }
                External::Service { name, user } => {
                    w.u8(6).str(name).bool(*user);
                }
            }
        }
        w.varint(self.prerequisites.len() as u64);
        for (id, v) in &self.prerequisites {
            w.str(id).str(v);
        }
        w.opt_str(self.uninstaller.as_deref());
        buf
    }

    pub fn decode(bytes: &[u8]) -> Result<Manifest, WireError> {
        let mut r = Reader::new(bytes);
        if r.take(8)? != MAGIC || r.u16()? != VERSION {
            return Err(WireError::BadMagic);
        }
        let s = |r: &mut Reader<'_>| r.str().map(str::to_owned);
        let product_id = s(&mut r)?;
        let name = s(&mut r)?;
        let publisher = s(&mut r)?;
        let version = s(&mut r)?;
        let install_dir = PathBuf::from(r.str()?);
        let scope = match r.u8()? {
            0 => Scope::User,
            1 => Scope::Machine,
            tag => {
                return Err(WireError::InvalidTag {
                    what: "scope",
                    tag: u64::from(tag),
                });
            }
        };
        let language = s(&mut r)?;
        let installed_at = r.u64()?;
        let n = r.count(34)?;
        let mut files = Vec::with_capacity(n);
        for _ in 0..n {
            let path = s(&mut r)?;
            inst_fsx::relpath::validate(&path).map_err(|_| WireError::InvalidUtf8)?;
            files.push(ManifestFile {
                path,
                size: r.varint()?,
                hash: r.hash()?,
            });
        }
        let n = r.count(2)?;
        let mut dirs = Vec::with_capacity(n);
        for _ in 0..n {
            let d = s(&mut r)?;
            inst_fsx::relpath::validate(&d).map_err(|_| WireError::InvalidUtf8)?;
            dirs.push(d);
        }
        let n = r.count(2)?;
        let mut external = Vec::with_capacity(n);
        for _ in 0..n {
            external.push(match r.u8()? {
                1 => External::File(PathBuf::from(r.str()?)),
                2 => External::RegistryKey {
                    hive: r.u8()?,
                    key: s(&mut r)?,
                },
                3 => External::RegistryValue {
                    hive: r.u8()?,
                    key: s(&mut r)?,
                    name: s(&mut r)?,
                },
                4 => External::PathEntry {
                    machine: r.bool()?,
                    name: s(&mut r)?,
                    entry: s(&mut r)?,
                },
                5 => External::EnvVar {
                    machine: r.bool()?,
                    name: s(&mut r)?,
                },
                6 => External::Service {
                    name: s(&mut r)?,
                    user: r.bool()?,
                },
                tag => {
                    return Err(WireError::InvalidTag {
                        what: "external",
                        tag: u64::from(tag),
                    });
                }
            });
        }
        let n = r.count(2)?;
        let mut prerequisites = Vec::with_capacity(n);
        for _ in 0..n {
            prerequisites.push((s(&mut r)?, s(&mut r)?));
        }
        let uninstaller = r.opt_str()?.map(str::to_owned);
        r.finish()?;
        Ok(Manifest {
            product_id,
            name,
            publisher,
            version,
            install_dir,
            scope,
            language,
            installed_at,
            files,
            dirs,
            external,
            prerequisites,
            uninstaller,
        })
    }

    pub fn load(install_dir: &Path) -> std::io::Result<Manifest> {
        let bytes = std::fs::read(Manifest::path_in(install_dir))?;
        Manifest::decode(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    pub fn store(&self, install_dir: &Path) -> std::io::Result<()> {
        let path = Manifest::path_in(install_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        inst_fsx::write_atomic(&path, &self.encode(), true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> Manifest {
        Manifest {
            product_id: "com.acme.app".into(),
            name: "Acme".into(),
            publisher: "Acme Ltd".into(),
            version: "1.4.0".into(),
            install_dir: "/home/u/.local/share/Acme".into(),
            scope: Scope::User,
            language: "tr".into(),
            installed_at: 1_790_000_000,
            files: vec![ManifestFile {
                path: "bin/app".into(),
                size: 10,
                hash: [3; 32],
            }],
            dirs: vec!["bin".into()],
            external: vec![
                External::File("/home/u/.local/share/applications/com.acme.app.desktop".into()),
                External::RegistryKey {
                    hive: 1,
                    key: "Software\\Acme".into(),
                },
                External::RegistryValue {
                    hive: 1,
                    key: "Software\\Microsoft\\Windows\\CurrentVersion\\Run".into(),
                    name: "Acme".into(),
                },
                External::PathEntry {
                    machine: false,
                    name: "Path".into(),
                    entry: "C:\\Acme\\bin".into(),
                },
                External::EnvVar {
                    machine: true,
                    name: "ACME_HOME".into(),
                },
                External::Service {
                    name: "acme".into(),
                    user: true,
                },
            ],
            prerequisites: vec![("dotnet-runtime".into(), "8.0.31".into())],
            uninstaller: Some("uninstall".into()),
        }
    }

    #[test]
    fn roundtrip_and_reject_corruption() {
        let m = sample();
        let bytes = m.encode();
        assert_eq!(Manifest::decode(&bytes), Ok(m));
        for i in 0..bytes.len() {
            let mut b = bytes.clone();
            b[i] ^= 0xff;
            let _ = Manifest::decode(&b); // must not panic
        }
        assert!(Manifest::decode(&bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn rejects_traversal_paths() {
        let mut m = sample();
        m.files[0].path = "../../etc/passwd".into();
        assert!(Manifest::decode(&m.encode()).is_err());
    }
}
