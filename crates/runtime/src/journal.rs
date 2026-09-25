//! Transaction journal with rollback and crash recovery.
//!
//! Every reversible change is recorded *before it becomes visible* as an
//! [`Undo`] record. Records are appended to an on-disk journal immediately
//! (one `write` per record, no buffering), so a killed installer leaves a
//! journal that the next run rolls back ("failure recovery").
//!
//! The journal directory is a sibling of the installation directory, which
//! guarantees that backups are on the same volume and can be restored with a
//! rename.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use inst_log::{Logger, log};
use inst_wire::{Reader, Writer};

const MAGIC: &[u8; 8] = b"INSTJRNL";
const VERSION: u16 = 1;

/// A reversible change and how to revert it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Undo {
    /// A new file was created; revert by deleting it.
    RemoveFile(PathBuf),
    /// An existing file was replaced; its previous content was moved to
    /// `backup`.
    RestoreFile { target: PathBuf, backup: PathBuf },
    /// A directory was created; revert by removing it if empty.
    RemoveDir(PathBuf),
    /// A registry value was written (Windows). `previous` holds the old
    /// value type and data, `None` if it did not exist.
    RegistryValue {
        hive: u8,
        key: String,
        name: String,
        previous: Option<(u32, Vec<u8>)>,
    },
    /// A registry key was created (Windows); revert by deleting the tree.
    RegistryKey { hive: u8, key: String },
    /// An environment variable was changed.
    EnvVar {
        machine: bool,
        name: String,
        previous: Option<String>,
    },
    /// A service was installed; revert by stopping and deleting it.
    Service { name: String, user: bool },
}

impl Undo {
    fn encode(&self, w: &mut Writer<'_>) {
        let path = |p: &Path| p.to_string_lossy().into_owned();
        match self {
            Undo::RemoveFile(p) => {
                w.u8(1).str(&path(p));
            }
            Undo::RestoreFile { target, backup } => {
                w.u8(2).str(&path(target)).str(&path(backup));
            }
            Undo::RemoveDir(p) => {
                w.u8(3).str(&path(p));
            }
            Undo::RegistryValue {
                hive,
                key,
                name,
                previous,
            } => {
                w.u8(4).u8(*hive).str(key).str(name);
                match previous {
                    Some((ty, data)) => w.u8(1).u32(*ty).bytes(data),
                    None => w.u8(0),
                };
            }
            Undo::RegistryKey { hive, key } => {
                w.u8(5).u8(*hive).str(key);
            }
            Undo::EnvVar {
                machine,
                name,
                previous,
            } => {
                w.u8(6)
                    .bool(*machine)
                    .str(name)
                    .opt_str(previous.as_deref());
            }
            Undo::Service { name, user } => {
                w.u8(7).str(name).bool(*user);
            }
        }
    }

    fn decode(r: &mut Reader<'_>) -> Option<Undo> {
        let p = |s: &str| PathBuf::from(s);
        Some(match r.u8().ok()? {
            1 => Undo::RemoveFile(p(r.str().ok()?)),
            2 => Undo::RestoreFile {
                target: p(r.str().ok()?),
                backup: p(r.str().ok()?),
            },
            3 => Undo::RemoveDir(p(r.str().ok()?)),
            4 => {
                let hive = r.u8().ok()?;
                let key = r.str().ok()?.to_owned();
                let name = r.str().ok()?.to_owned();
                let previous = if r.bool().ok()? {
                    Some((r.u32().ok()?, r.bytes().ok()?.to_vec()))
                } else {
                    None
                };
                Undo::RegistryValue {
                    hive,
                    key,
                    name,
                    previous,
                }
            }
            5 => Undo::RegistryKey {
                hive: r.u8().ok()?,
                key: r.str().ok()?.to_owned(),
            },
            6 => Undo::EnvVar {
                machine: r.bool().ok()?,
                name: r.str().ok()?.to_owned(),
                previous: r.opt_str().ok()?.map(str::to_owned),
            },
            7 => Undo::Service {
                name: r.str().ok()?.to_owned(),
                user: r.bool().ok()?,
            },
            _ => return None,
        })
    }
}

/// Outcome of a rollback. Failures are never hidden.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RollbackReport {
    pub reverted: usize,
    pub failures: Vec<String>,
}

impl RollbackReport {
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Platform hooks for undo records that are not plain file operations.
pub trait UndoPlatform {
    fn undo_registry_value(
        &self,
        hive: u8,
        key: &str,
        name: &str,
        previous: Option<&(u32, Vec<u8>)>,
    ) -> io::Result<()>;
    fn undo_registry_key(&self, hive: u8, key: &str) -> io::Result<()>;
    fn undo_env_var(&self, machine: bool, name: &str, previous: Option<&str>) -> io::Result<()>;
    fn undo_service(&self, name: &str, user: bool) -> io::Result<()>;
}

pub struct Journal {
    dir: PathBuf,
    file: File,
    records: Vec<Undo>,
    backups: u64,
    buf: Vec<u8>,
}

impl Journal {
    /// Directory name prefix of journals next to an installation directory.
    pub const PREFIX: &'static str = ".inst-txn-";

    /// Creates a journal next to `install_dir` (in its parent directory).
    pub fn create(install_dir: &Path, product_id: &str) -> io::Result<Journal> {
        let parent = install_dir.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "install dir has no parent")
        })?;
        let folder = install_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut dir;
        loop {
            dir = parent.join(format!(
                "{}{folder}-{:x}",
                Journal::PREFIX,
                inst_fsx::atomic::unique_token()
            ));
            match fs::create_dir(&dir) {
                Ok(()) => break,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(dir.join("journal"))?;
        let mut header = Vec::with_capacity(64);
        Writer::new(&mut header)
            .raw(MAGIC)
            .u16(VERSION)
            .str(product_id)
            .str(&install_dir.to_string_lossy());
        file.write_all(&header)?;
        file.sync_all()?;
        Ok(Journal {
            dir,
            file,
            records: Vec::new(),
            backups: 0,
            buf: Vec::with_capacity(256),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// A fresh path inside the journal for backing up `original`.
    pub fn backup_path(&mut self, original: &Path) -> PathBuf {
        self.backups += 1;
        let name = original
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.dir.join(format!("b{:06}-{name}", self.backups))
    }

    /// Appends a record. It is written to disk before this returns.
    pub fn record(&mut self, undo: Undo) -> io::Result<()> {
        self.buf.clear();
        self.buf.extend_from_slice(&[0; 4]);
        undo.encode(&mut Writer::new(&mut self.buf));
        let len = (self.buf.len() - 4) as u32;
        self.buf[..4].copy_from_slice(&len.to_le_bytes());
        self.file.write_all(&self.buf)?;
        self.records.push(undo);
        Ok(())
    }

    /// Makes the recorded changes permanent: deletes backups and the journal.
    pub fn commit(self) -> io::Result<()> {
        drop(self.file);
        fs::remove_dir_all(&self.dir)
    }

    /// Reverts every recorded change in reverse order.
    pub fn rollback(self, platform: &dyn UndoPlatform, logger: &Logger) -> RollbackReport {
        let Journal {
            dir, file, records, ..
        } = self;
        drop(file);
        rollback_records(&records, &dir, platform, logger)
    }

    /// Rolls back journals left behind by an interrupted run next to
    /// `install_dir`. Returns one report per recovered journal.
    pub fn recover(
        install_dir: &Path,
        platform: &dyn UndoPlatform,
        logger: &Logger,
    ) -> Vec<RollbackReport> {
        let Some(parent) = install_dir.parent() else {
            return Vec::new();
        };
        let folder = install_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let prefix = format!("{}{folder}-", Journal::PREFIX);
        let Ok(entries) = fs::read_dir(parent) else {
            return Vec::new();
        };
        let mut reports = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(&prefix) {
                continue;
            }
            let dir = entry.path();
            match read_records(&dir.join("journal")) {
                Some(records) => {
                    log!(logger, Warn, "recovery", "rolling back interrupted installation";
                        "journal" => &dir.to_string_lossy().into_owned(), "records" => records.len());
                    reports.push(rollback_records(&records, &dir, platform, logger));
                }
                None => {
                    log!(logger, Warn, "recovery", "discarding unreadable journal";
                        "journal" => &dir.to_string_lossy().into_owned());
                    let _ = fs::remove_dir_all(&dir);
                }
            }
        }
        reports
    }
}

fn read_records(path: &Path) -> Option<Vec<Undo>> {
    let bytes = fs::read(path).ok()?;
    let mut r = Reader::new(&bytes);
    if r.take(8).ok()? != MAGIC || r.u16().ok()? != VERSION {
        return None;
    }
    let _product = r.str().ok()?;
    let _install_dir = r.str().ok()?;
    let mut records = Vec::new();
    while r.remaining() >= 4 {
        let len = r.u32().ok()? as usize;
        let Ok(body) = r.take(len) else {
            break; // torn final record: the change it describes never happened
        };
        match Undo::decode(&mut Reader::new(body)) {
            Some(u) => records.push(u),
            None => break,
        }
    }
    Some(records)
}

fn rollback_records(
    records: &[Undo],
    journal_dir: &Path,
    platform: &dyn UndoPlatform,
    logger: &Logger,
) -> RollbackReport {
    let mut report = RollbackReport::default();
    // Directories that contain the journal can only be removed after it.
    let mut deferred = Vec::new();
    for undo in records.iter().rev() {
        if let Undo::RemoveDir(d) = undo
            && journal_dir.starts_with(d)
        {
            deferred.push(d.clone());
            continue;
        }
        let result = apply(undo, platform);
        match result {
            Ok(()) => report.reverted += 1,
            Err(e) => {
                let msg = format!("{undo:?}: {e}");
                log!(logger, Error, "rollback", "could not revert change"; "change" => &msg);
                report.failures.push(msg);
            }
        }
    }
    if let Err(e) = fs::remove_dir_all(journal_dir) {
        report
            .failures
            .push(format!("remove journal {}: {e}", journal_dir.display()));
    }
    for d in deferred {
        match remove_dir_if_empty(&d) {
            Ok(()) => report.reverted += 1,
            Err(e) => report.failures.push(format!("remove {}: {e}", d.display())),
        }
    }
    log!(logger, Info, "rollback", "rollback finished";
        "reverted" => report.reverted, "failures" => report.failures.len());
    report
}

fn remove_dir_if_empty(dir: &Path) -> io::Result<()> {
    match fs::remove_dir(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        // Not empty: something that is not ours lives there; keep it.
        Err(_)
            if fs::read_dir(dir)
                .map(|mut d| d.next().is_some())
                .unwrap_or(false) =>
        {
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn apply(undo: &Undo, platform: &dyn UndoPlatform) -> io::Result<()> {
    match undo {
        Undo::RemoveFile(p) => match fs::remove_file(p) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        },
        Undo::RestoreFile { target, backup } => {
            if !backup.exists() {
                return Err(io::Error::new(io::ErrorKind::NotFound, "backup is missing"));
            }
            fs::rename(backup, target)
        }
        Undo::RemoveDir(d) => remove_dir_if_empty(d),
        Undo::RegistryValue {
            hive,
            key,
            name,
            previous,
        } => platform.undo_registry_value(*hive, key, name, previous.as_ref()),
        Undo::RegistryKey { hive, key } => platform.undo_registry_key(*hive, key),
        Undo::EnvVar {
            machine,
            name,
            previous,
        } => platform.undo_env_var(*machine, name, previous.as_deref()),
        Undo::Service { name, user } => platform.undo_service(name, *user),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoPlatform;
    impl UndoPlatform for NoPlatform {
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
        fn undo_service(&self, _: &str, _: bool) -> io::Result<()> {
            Ok(())
        }
    }

    fn setup() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().expect("tmp");
        let install = tmp.path().join("App");
        (tmp, install)
    }

    fn simulate_install(j: &mut Journal, install: &Path) {
        fs::create_dir(install).expect("mkdir");
        j.record(Undo::RemoveDir(install.to_path_buf()))
            .expect("rec");
        let new = install.join("new.txt");
        j.record(Undo::RemoveFile(new.clone())).expect("rec");
        fs::write(&new, b"new").expect("write");
    }

    #[test]
    fn rollback_restores_everything() {
        let (tmp, install) = setup();
        // Pre-existing file outside the install dir that we overwrite.
        let shared = tmp.path().join("shared.cfg");
        fs::write(&shared, b"original").expect("write");

        let mut j = Journal::create(&install, "com.acme.app").expect("journal");
        simulate_install(&mut j, &install);
        let backup = j.backup_path(&shared);
        fs::rename(&shared, &backup).expect("backup");
        j.record(Undo::RestoreFile {
            target: shared.clone(),
            backup,
        })
        .expect("rec");
        fs::write(&shared, b"modified").expect("write");

        let report = j.rollback(&NoPlatform, &Logger::disabled());
        assert!(report.is_complete(), "{report:?}");
        assert_eq!(fs::read(&shared).expect("read"), b"original");
        assert!(!install.exists());
        assert_eq!(
            fs::read_dir(tmp.path()).expect("ls").count(),
            1,
            "only shared.cfg remains"
        );
    }

    #[test]
    fn recovers_after_crash() {
        let (tmp, install) = setup();
        {
            let mut j = Journal::create(&install, "com.acme.app").expect("journal");
            simulate_install(&mut j, &install);
            // Simulate a torn trailing record from a crash mid-write.
            let mut f = OpenOptions::new()
                .append(true)
                .open(j.dir().join("journal"))
                .expect("open");
            f.write_all(&[200, 0, 0, 0, 1]).expect("write");
            std::mem::forget(j); // process "dies" without rollback
        }
        assert!(install.join("new.txt").exists());
        let reports = Journal::recover(&install, &NoPlatform, &Logger::disabled());
        assert_eq!(reports.len(), 1);
        assert!(reports[0].is_complete(), "{:?}", reports[0]);
        assert!(!install.exists());
        assert_eq!(fs::read_dir(tmp.path()).expect("ls").count(), 0);
    }

    #[test]
    fn rollback_reports_failures_and_keeps_foreign_files() {
        let (_tmp, install) = setup();
        let mut j = Journal::create(&install, "p").expect("journal");
        simulate_install(&mut j, &install);
        // A file we did not create appears in the install dir.
        fs::write(install.join("user-data.db"), b"keep").expect("write");
        j.record(Undo::RestoreFile {
            target: install.join("x"),
            backup: install.join("missing-backup"),
        })
        .expect("rec");
        let report = j.rollback(&NoPlatform, &Logger::disabled());
        assert_eq!(report.failures.len(), 1);
        assert!(install.join("user-data.db").exists());
    }

    #[test]
    fn commit_removes_journal() {
        let (tmp, install) = setup();
        let mut j = Journal::create(&install, "p").expect("journal");
        simulate_install(&mut j, &install);
        j.commit().expect("commit");
        assert!(install.join("new.txt").exists());
        assert_eq!(fs::read_dir(tmp.path()).expect("ls").count(), 1);
    }
}
