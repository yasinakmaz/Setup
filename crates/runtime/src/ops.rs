//! Built-in installation steps. Each is a method on [`Install`] called from
//! the generated graph function.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use inst_fsx::RelPath;
use inst_log::log;
use inst_payload::FileEntry;
use inst_payload::read::{self, ExtractBuffers, ExtractObserver, ExtractOptions, FileAction};

use crate::app::{SameVersion, Scope};
use crate::context::Install;
use crate::detect::Ver;
use crate::error::{ErrorKind, InstallError};
use crate::events::Mode;
use crate::journal::{Journal, Undo};
use crate::manifest::{METADATA_DIR, Manifest, ManifestFile};
use crate::platform::{self, ProductRecord, ShortcutSpec};

/// Component id reserved for embedded prerequisite packages; never extracted
/// into the installation directory.
pub const PREREQ_COMPONENT: u16 = u16::MAX;

/// File name of the uninstaller inside the installation directory.
pub const UNINSTALLER_NAME: &str = if cfg!(windows) {
    "uninstall.exe"
} else {
    "uninstall"
};

fn io_err(context: &str) -> impl Fn(io::Error) -> InstallError + '_ {
    move |e| InstallError::io(context, &e)
}

impl Install<'_> {
    /// Elevation, version policy, disk space and crash recovery.
    pub fn check_environment(&mut self) -> Result<(), InstallError> {
        let app = self.c.app;
        let settings = app.settings;
        let policy = settings.policy;
        log!(self.c.logger, Info, "env", "environment";
            "os" => &format!("{:?}", self.c.env.os), "arch" => &format!("{:?}", self.c.env.arch),
            "elevated" => self.c.env.elevated, "mode" => &format!("{:?}", self.options.mode));

        if (settings.requires_elevation || settings.scope == Scope::Machine) && !self.c.env.elevated {
            return Err(InstallError::new(
                ErrorKind::RequiresAdmin,
                "a machine-wide installation needs administrator/root rights",
            ));
        }
        let blocked = |version: &str| {
            InstallError::new(
                ErrorKind::AlreadyInstalled {
                    name: app.product.name.to_owned(),
                    version: version.to_owned(),
                },
                "installed version policy",
            )
        };
        match &self.options.mode {
            Mode::Downgrade { from } if !policy.allow_downgrade => return Err(blocked(from)),
            Mode::Repair if policy.same_version == SameVersion::Block => {
                return Err(blocked(app.product.version));
            }
            _ => {}
        }

        if policy.failure_recovery {
            for report in crate::journal::Journal::recover(
                &self.c.install_dir,
                &platform::Platform,
                &self.c.logger,
            ) {
                if !report.is_complete() {
                    log!(self.c.logger, Warn, "recovery", "recovery left changes behind";
                        "failures" => report.failures.len());
                }
            }
        }

        if policy.disk_space_check {
            let previous = self
                .options
                .previous
                .as_ref()
                .map_or(0, Manifest::total_size);
            let size = settings.installed_size;
            // 10% + 32 MiB headroom for backups, logs and file-system overhead.
            let required = size.saturating_sub(previous) + size / 10 + (32 << 20);
            match inst_fsx::available_space(&self.c.install_dir) {
                Ok(available) if available < required => {
                    return Err(InstallError::new(
                        ErrorKind::DiskSpace {
                            required,
                            available,
                        },
                        format!("{} bytes required, {} available", required, available),
                    ));
                }
                Ok(_) => {}
                Err(e) => {
                    log!(self.c.logger, Warn, "env", "cannot query free space"; "error" => &e.to_string())
                }
            }
        }
        Ok(())
    }

    /// Creates the installation directory (journaled).
    pub fn resolve_location(&mut self) -> Result<(), InstallError> {
        let dir = self.c.install_dir.clone();
        if !dir.is_absolute() {
            return Err(InstallError::new(
                ErrorKind::AccessDenied,
                "installation directory must be absolute",
            ));
        }
        let parent = dir
            .parent()
            .ok_or_else(|| {
                InstallError::new(
                    ErrorKind::AccessDenied,
                    "installation directory has no parent",
                )
            })?
            .to_path_buf();
        let created_parents = inst_fsx::dirs::create_root(&parent)
            .map_err(io_err("creating the installation folder"))?;
        for p in created_parents {
            self.record(Undo::RemoveDir(p))?;
        }
        self.journal()?;
        match fs::symlink_metadata(&dir) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                return Err(InstallError::new(
                    ErrorKind::AccessDenied,
                    format!("{} exists and is not a folder", dir.display()),
                ));
            }
            Err(_) => {
                self.record(Undo::RemoveDir(dir.clone()))?;
                fs::create_dir(&dir).map_err(io_err("creating the installation folder"))?;
            }
        }
        // Probe writability early for a clear error instead of a failure
        // half-way through extraction.
        let probe = dir.join(format!(
            ".write-probe-{:x}",
            inst_fsx::atomic::unique_token()
        ));
        File::create(&probe).map_err(io_err("writing to the installation folder"))?;
        let _ = fs::remove_file(&probe);
        log!(self.c.logger, Info, "location", "installation folder ready";
            "dir" => &dir.to_string_lossy().into_owned());
        Ok(())
    }

    /// Extracts the application payload (parallel, journaled, verified).
    pub fn extract_application(&mut self) -> Result<(), InstallError> {
        let root = self.c.install_dir.clone();
        let total: u64 = self
            .payload
            .index
            .files
            .iter()
            .filter(|f| f.component != PREREQ_COMPONENT)
            .map(|f| f.size)
            .sum();
        let blocks: Vec<usize> = (0..self.payload.index.blocks.len())
            .filter(|&i| {
                self.payload
                    .index
                    .block_files(&self.payload.index.blocks[i])
                    .iter()
                    .any(|f| f.component != PREREQ_COMPONENT)
            })
            .collect();
        let journal = Mutex::new(
            self.journal
                .take()
                .expect("journal exists after resolve_location"),
        );
        let progress = AtomicU64::new(0);
        let next = AtomicUsize::new(0);
        let failed = AtomicBool::new(false);
        let first_error: Mutex<Option<InstallError>> = Mutex::new(None);
        let results: Mutex<(Vec<ManifestFile>, Vec<String>)> = Mutex::new((Vec::new(), Vec::new()));
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(4)
            .min(blocks.len().max(1));
        let cancel = self.c.cancel;
        let payload = &self.payload;
        let exe = &self.exe;

        {
            let mut bufs = ExtractBuffers::default();
            read::extract_dirs(
                payload,
                &root,
                &mut JournalObserver::new(&journal, &root, &progress, cancel, &failed),
                &mut bufs,
            )?;
        }

        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    let mut file = match File::open(exe) {
                        Ok(f) => f,
                        Err(e) => {
                            failed.store(true, Ordering::Relaxed);
                            *first_error.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(InstallError::io("opening the setup file", &e));
                            return;
                        }
                    };
                    let mut bufs = ExtractBuffers::default();
                    let mut obs = JournalObserver::new(&journal, &root, &progress, cancel, &failed);
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        if k >= blocks.len() || failed.load(Ordering::Relaxed) {
                            break;
                        }
                        let block = blocks[k];
                        let result = payload
                            .open_block(&mut file, block)
                            .map_err(inst_payload::PayloadError::from)
                            .and_then(|raw| {
                                read::extract_block(
                                    payload,
                                    block,
                                    raw,
                                    &root,
                                    &mut obs,
                                    &mut bufs,
                                    ExtractOptions::default(),
                                )
                            });
                        if let Err(e) = result {
                            failed.store(true, Ordering::Relaxed);
                            let mut slot = first_error.lock().unwrap_or_else(|p| p.into_inner());
                            if slot.is_none() {
                                *slot = Some(match obs.error.take() {
                                    Some(e) => InstallError::io("recording changes", &e),
                                    None => InstallError::from(e),
                                });
                            }
                            break;
                        }
                    }
                    let mut r = results.lock().unwrap_or_else(|p| p.into_inner());
                    r.0.append(&mut obs.files);
                    r.1.append(&mut obs.dirs);
                });
            }
            // Report progress from this thread while workers run.
            loop {
                let done = progress.load(Ordering::Relaxed);
                self.c.events.send(crate::events::Event::StepProgress {
                    step: self.current_step,
                    fraction: if total == 0 {
                        1.0
                    } else {
                        (done as f64 / total as f64) as f32
                    },
                });
                if next.load(Ordering::Relaxed) >= blocks.len() + threads
                    || failed.load(Ordering::Relaxed)
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });

        self.journal = Some(journal.into_inner().unwrap_or_else(|p| p.into_inner()));
        if let Some(e) = first_error.into_inner().unwrap_or_else(|p| p.into_inner()) {
            return Err(e);
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(InstallError::cancelled());
        }
        let (mut files, dirs) = results.into_inner().unwrap_or_else(|p| p.into_inner());
        files.sort_by(|a, b| a.path.cmp(&b.path));
        self.files = files;
        self.dirs = dirs;
        self.remove_obsolete_files()?;
        self.progress(total, total);
        log!(self.c.logger, Info, "extract", "application files installed";
            "files" => self.files.len(), "bytes" => total);
        Ok(())
    }

    /// On upgrade, removes files of the previous version that the new
    /// version no longer ships (backed up for rollback).
    fn remove_obsolete_files(&mut self) -> Result<(), InstallError> {
        let Some(previous) = self.options.previous.clone() else {
            return Ok(());
        };
        let current: std::collections::HashSet<&str> =
            self.files.iter().map(|f| f.path.as_str()).collect();
        let obsolete: Vec<PathBuf> = previous
            .files
            .iter()
            .filter(|f| !current.contains(f.path.as_str()))
            .filter_map(|f| {
                RelPath::new(&f.path)
                    .ok()
                    .map(|r| r.to_path_under(&self.c.install_dir))
            })
            .collect();
        for path in obsolete {
            if !path.exists() {
                continue;
            }
            let journal = self.journal()?;
            let backup = journal.backup_path(&path);
            fs::rename(&path, &backup).map_err(io_err("removing obsolete files"))?;
            self.record(Undo::RestoreFile {
                target: path,
                backup,
            })?;
        }
        Ok(())
    }

    fn main_executable(&self) -> Option<PathBuf> {
        self.c
            .app
            .product
            .main_executable
            .and_then(|m| RelPath::new(m).ok())
            .map(|r| r.to_path_under(&self.c.install_dir))
    }

    fn icon_path(&self) -> Option<PathBuf> {
        self.c
            .app
            .product
            .icon
            .and_then(|i| RelPath::new(i).ok())
            .map(|r| r.to_path_under(&self.c.install_dir))
    }

    /// Start menu / application menu entry and optional desktop shortcut.
    pub fn create_shortcuts(&mut self) -> Result<(), InstallError> {
        let Some(exe) = self.main_executable() else {
            log!(
                self.c.logger,
                Warn,
                "shortcuts",
                "no main executable; shortcuts skipped"
            );
            return Ok(());
        };
        let app = self.c.app;
        let icon = self.icon_path();
        let dir = self.c.install_dir.clone();
        let description = app.product.description(self.c.language);
        let integ = app.settings.integration;
        let machine = self.c.machine();
        let spec = ShortcutSpec {
            id: app.product.id,
            name: app.product.name,
            description,
            target: &exe,
            args: app.product.arguments,
            working_dir: &dir,
            icon: icon.as_deref(),
            machine,
        };
        let mut journal = self.journal.take().expect("journal exists");
        let mut external = std::mem::take(&mut self.external);
        let mut result = Ok(());
        if integ.start_menu {
            result = platform::create_menu_entry(&spec, &mut journal, &mut external);
        }
        if result.is_ok() && self.options.desktop_shortcut {
            result = platform::create_desktop_shortcut(&spec, &mut journal, &mut external);
        }
        self.journal = Some(journal);
        self.external = external;
        result.map_err(io_err("creating shortcuts"))
    }

    /// PATH, autostart, associations.
    pub fn apply_integration(&mut self) -> Result<(), InstallError> {
        let app = self.c.app;
        let integ = app.settings.integration;
        let Some(exe) = self.main_executable() else {
            return Ok(());
        };
        let icon = self.icon_path();
        let dir = self.c.install_dir.clone();
        let machine = self.c.machine();
        let spec = ShortcutSpec {
            id: app.product.id,
            name: app.product.name,
            description: app.product.description(self.c.language),
            target: &exe,
            args: app.product.arguments,
            working_dir: &dir,
            icon: icon.as_deref(),
            machine,
        };
        let mut journal = self.journal.take().expect("journal exists");
        let mut external = std::mem::take(&mut self.external);
        let mut result = Ok(());
        if integ.add_to_path {
            result = platform::add_to_path(&exe, machine, &mut journal, &mut external);
        }
        if result.is_ok() && integ.launch_at_startup {
            result = platform::add_autostart(&spec, &mut journal, &mut external);
        }
        self.journal = Some(journal);
        self.external = external;
        if !integ.file_associations.is_empty() || !integ.protocols.is_empty() {
            log!(
                self.c.logger,
                Warn,
                "integration",
                "file and protocol associations are not supported by this runtime version; skipped"
            );
        }
        result.map_err(io_err("applying desktop integration"))
    }

    /// Writes the uninstaller, the installation manifest and the
    /// Installed Apps / product registry entry.
    pub fn register_uninstaller(&mut self) -> Result<(), InstallError> {
        let dir = self.c.install_dir.clone();
        let uninstaller = dir.join(UNINSTALLER_NAME);
        self.write_uninstaller(&uninstaller)?;

        let meta_dir = dir.join(METADATA_DIR);
        if !meta_dir.exists() {
            self.record(Undo::RemoveDir(meta_dir.clone()))?;
            fs::create_dir(&meta_dir).map_err(io_err("creating metadata folder"))?;
        }
        let app = self.c.app;
        let manifest = self.build_manifest();
        let manifest_path = Manifest::path_in(&dir);
        self.backup_or_mark_new(&manifest_path)?;
        manifest
            .store(&dir)
            .map_err(io_err("writing the installation manifest"))?;

        let icon = self.icon_path().or_else(|| self.main_executable());
        let record = ProductRecord {
            id: app.product.id,
            name: app.product.name,
            publisher: app.product.publisher,
            version: app.product.version,
            install_dir: &dir,
            uninstaller: &uninstaller,
            icon: icon.as_deref(),
            size_bytes: manifest.total_size(),
            homepage: app.product.homepage,
            support_url: app.product.support_url,
            machine: self.c.machine(),
        };
        let mut journal = self.journal.take().expect("journal exists");
        let mut external = std::mem::take(&mut self.external);
        let result = platform::register_product(&record, &mut journal, &mut external);
        self.journal = Some(journal);
        self.external = external;
        result.map_err(io_err("registering the application"))?;
        // Re-store the manifest so it includes the registration entries.
        self.build_manifest()
            .store(&dir)
            .map_err(io_err("writing the installation manifest"))?;
        Ok(())
    }

    fn backup_or_mark_new(&mut self, path: &Path) -> Result<(), InstallError> {
        if path.exists() {
            let journal = self.journal()?;
            let backup = journal.backup_path(path);
            fs::copy(path, &backup).map_err(io_err("backing up"))?;
            self.record(Undo::RestoreFile {
                target: path.to_path_buf(),
                backup,
            })
        } else {
            self.record(Undo::RemoveFile(path.to_path_buf()))
        }
    }

    /// Copies the running executable without its payload.
    fn write_uninstaller(&mut self, dest: &Path) -> Result<(), InstallError> {
        self.backup_or_mark_new(dest)?;
        let mut src = File::open(&self.exe).map_err(io_err("reading the setup file"))?;
        let mut out =
            inst_fsx::AtomicFile::create(dest).map_err(io_err("writing the uninstaller"))?;
        let mut remaining = self.payload.base;
        let mut buf = vec![0u8; 256 << 10];
        src.seek(SeekFrom::Start(0))
            .map_err(io_err("reading the setup file"))?;
        while remaining > 0 {
            let want = buf.len().min(remaining as usize);
            let n = src
                .read(&mut buf[..want])
                .map_err(io_err("reading the setup file"))?;
            if n == 0 {
                return Err(InstallError::new(
                    ErrorKind::Damaged,
                    "setup file ended early",
                ));
            }
            out.write_all(&buf[..n])
                .map_err(io_err("writing the uninstaller"))?;
            remaining -= n as u64;
        }
        out.commit(true)
            .map_err(io_err("writing the uninstaller"))?;
        platform::finalize_uninstaller(dest).map_err(io_err("writing the uninstaller"))?;
        Ok(())
    }

    pub(crate) fn build_manifest(&self) -> Manifest {
        let app = self.c.app;
        let mut dirs = self.dirs.clone();
        let root = &self.c.install_dir;
        dirs.sort_by_key(|d| d.matches('/').count());
        dirs.dedup();
        // Keep previously installed directories on upgrade.
        if let Some(prev) = &self.options.previous {
            for d in &prev.dirs {
                if !dirs.contains(d)
                    && RelPath::new(d).is_ok_and(|r| r.to_path_under(root).is_dir())
                {
                    dirs.push(d.clone());
                }
            }
        }
        let mut prerequisites = self
            .options
            .previous
            .as_ref()
            .map(|p| p.prerequisites.clone())
            .unwrap_or_default();
        for p in &self.prereqs_installed {
            if !prerequisites.contains(p) {
                prerequisites.push(p.clone());
            }
        }
        Manifest {
            product_id: app.product.id.to_owned(),
            name: app.product.name.to_owned(),
            publisher: app.product.publisher.to_owned(),
            version: app.product.version.to_owned(),
            install_dir: root.clone(),
            scope: app.settings.scope,
            language: self.c.language.code().to_owned(),
            installed_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            files: self.files.clone(),
            dirs,
            external: self.external.clone(),
            prerequisites,
            uninstaller: Some(UNINSTALLER_NAME.to_owned()),
        }
    }

    /// Checks that every installed file is present with the right size.
    /// Content hashes were verified during extraction.
    pub fn verify_installation(&mut self) -> Result<(), InstallError> {
        let root = self.c.install_dir.clone();
        let total = self.files.len() as u64;
        for (i, f) in self.files.iter().enumerate() {
            let path = RelPath::new(&f.path)
                .map_err(|e| InstallError::unexpected(e.to_string()))?
                .to_path_under(&root);
            match fs::metadata(&path) {
                Ok(m) if m.len() == f.size => {}
                Ok(m) => {
                    return Err(InstallError::unexpected(format!(
                        "{} has size {} instead of {}",
                        f.path,
                        m.len(),
                        f.size
                    )));
                }
                Err(e) => return Err(InstallError::io(&format!("verifying {}", f.path), &e)),
            }
            if i % 256 == 0 {
                self.c.events.send(crate::events::Event::StepProgress {
                    step: self.current_step,
                    fraction: i as f32 / total.max(1) as f32,
                });
            }
        }
        Ok(())
    }

    /// Makes the installation permanent: commits the journal and removes
    /// empty directories left by an upgrade.
    pub(crate) fn commit(&mut self) -> Result<(), InstallError> {
        if let Some(journal) = self.journal.take() {
            journal
                .commit()
                .map_err(io_err("finishing the installation"))?;
        }
        if let Some(prev) = &self.options.previous {
            let mut dirs: Vec<&String> = prev.dirs.iter().collect();
            dirs.sort_by_key(|d| std::cmp::Reverse(d.matches('/').count()));
            for d in dirs {
                if let Ok(r) = RelPath::new(d) {
                    let _ = fs::remove_dir(r.to_path_under(&self.c.install_dir));
                }
            }
        }
        Ok(())
    }

    /// Compares the installed version with this installer's version.
    pub fn classify(previous: Option<&Manifest>, version: &str) -> Mode {
        match previous {
            None => Mode::Fresh,
            Some(p) => {
                let old = Ver::parse(&p.version).unwrap_or_default();
                let new = Ver::parse(version).unwrap_or_default();
                match old.cmp(&new) {
                    std::cmp::Ordering::Less => Mode::Upgrade {
                        from: p.version.clone(),
                    },
                    std::cmp::Ordering::Equal => Mode::Repair,
                    std::cmp::Ordering::Greater => Mode::Downgrade {
                        from: p.version.clone(),
                    },
                }
            }
        }
    }
}

/// Journals every file operation of payload extraction.
struct JournalObserver<'a> {
    journal: &'a Mutex<Journal>,
    root: &'a Path,
    progress: &'a AtomicU64,
    cancel: &'a AtomicBool,
    failed: &'a AtomicBool,
    files: Vec<ManifestFile>,
    dirs: Vec<String>,
    error: Option<io::Error>,
}

impl<'a> JournalObserver<'a> {
    fn new(
        journal: &'a Mutex<Journal>,
        root: &'a Path,
        progress: &'a AtomicU64,
        cancel: &'a AtomicBool,
        failed: &'a AtomicBool,
    ) -> Self {
        JournalObserver {
            journal,
            root,
            progress,
            cancel,
            failed,
            files: Vec::new(),
            dirs: Vec::new(),
            error: None,
        }
    }

    fn record(&mut self, undo: Undo) -> io::Result<()> {
        let r = self
            .journal
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .record(undo);
        if let Err(e) = &r {
            self.error = Some(io::Error::new(e.kind(), e.to_string()));
        }
        r
    }
}

impl ExtractObserver for JournalObserver<'_> {
    fn begin_file(
        &mut self,
        _path: RelPath<'_>,
        entry: &FileEntry,
        _target: &Path,
    ) -> io::Result<FileAction> {
        Ok(if entry.component == PREREQ_COMPONENT {
            FileAction::Skip
        } else {
            FileAction::Extract
        })
    }

    fn dirs_created(&mut self, dirs: &[PathBuf]) {
        for d in dirs {
            if self.record(Undo::RemoveDir(d.clone())).is_err() {
                self.failed.store(true, Ordering::Relaxed);
            }
            if let Ok(rel) = inst_fsx::relpath::relative_to(self.root, d) {
                self.dirs.push(rel);
            }
        }
    }

    fn before_commit(&mut self, _path: RelPath<'_>, target: &Path, exists: bool) -> io::Result<()> {
        if exists {
            let backup = self
                .journal
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .backup_path(target);
            fs::rename(target, &backup)?;
            self.record(Undo::RestoreFile {
                target: target.to_path_buf(),
                backup,
            })
        } else {
            self.record(Undo::RemoveFile(target.to_path_buf()))
        }
    }

    fn committed(
        &mut self,
        path: RelPath<'_>,
        entry: &FileEntry,
        _target: &Path,
    ) -> io::Result<()> {
        self.files.push(ManifestFile {
            path: path.as_str().to_owned(),
            size: entry.size,
            hash: entry.hash,
        });
        Ok(())
    }

    fn progress(&mut self, bytes: u64) {
        self.progress.fetch_add(bytes, Ordering::Relaxed);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || self.failed.load(Ordering::Relaxed)
    }
}
