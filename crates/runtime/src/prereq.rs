//! Smart prerequisites at install time: detect → acquire (cache, resumable
//! verified download, or embedded) → install silently.

use std::path::{Path, PathBuf};

use inst_log::log;

use crate::context::Install;
use crate::detect::{self, Detected, Detection, Ver};
use crate::error::{ErrorKind, InstallError};

/// How the package reaches the machine.
#[derive(Clone, Copy, Debug)]
pub enum Source {
    /// Downloaded when missing. `hash` is `sha256:…`/`sha512:…`/`blake3:…`.
    Download {
        urls: &'static [&'static str],
        hash: &'static str,
        size: u64,
    },
    /// Shipped inside the setup payload at this path.
    Embedded { path: &'static str },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallKind {
    Exe,
    Msi,
}

#[derive(Clone, Copy, Debug)]
pub enum Arg {
    Lit(&'static str),
    /// Value of a password field (redacted in logs).
    Secret(&'static str),
    Input(&'static str),
}

/// A prerequisite resolved at build time.
#[derive(Debug)]
pub struct Prerequisite {
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    /// Minimum acceptable installed version.
    pub min_version: Option<&'static str>,
    /// Installed version must have the same major version (.NET).
    pub same_major: bool,
    pub detection: Detection,
    pub source: Source,
    pub file_name: &'static str,
    pub kind: InstallKind,
    pub args: &'static [Arg],
    pub success_codes: &'static [i32],
    pub reboot_codes: &'static [i32],
    pub already_installed_codes: &'static [i32],
    /// Expected Authenticode publisher (Windows).
    pub publisher: Option<&'static str>,
}

impl Prerequisite {
    pub fn is_satisfied(&self) -> bool {
        let min = self.min_version.and_then(Ver::parse);
        if let Detection::DotnetSharedFramework { framework } = self.detection {
            return detect::dotnet_framework_versions(framework)
                .into_iter()
                .any(|v| detect::satisfies(v, min, self.same_major));
        }
        match detect::detect(&self.detection) {
            Detected::Missing => false,
            Detected::Present(None) => true,
            Detected::Present(Some(v)) => detect::satisfies(v, min, self.same_major),
        }
    }
}

impl Install<'_> {
    /// Detects which prerequisites are missing.
    pub fn check_prerequisites(&mut self) -> Result<(), InstallError> {
        let list = self.c.app.prerequisites;
        for (i, p) in list.iter().enumerate() {
            let missing = !p.is_satisfied();
            self.prereq_missing[i] = missing;
            log!(self.c.logger, Info, "prereq", "prerequisite detection";
                "id" => p.id, "missing" => missing);
            self.progress(i as u64 + 1, list.len() as u64);
        }
        Ok(())
    }

    /// Downloads (or extracts) the missing prerequisites.
    pub fn acquire_prerequisites(&mut self) -> Result<(), InstallError> {
        let list = self.c.app.prerequisites;
        for (i, p) in list.iter().enumerate() {
            if !self.prereq_missing[i] {
                continue;
            }
            let path = match p.source {
                Source::Download { urls, hash, size } => self.download(p, urls, hash, size)?,
                Source::Embedded { path } => self.extract_embedded(p, path)?,
            };
            verify_signature(&path, p)?;
            self.prereq_files[i] = Some(path);
        }
        Ok(())
    }

    #[cfg(feature = "download")]
    fn download(
        &mut self,
        p: &Prerequisite,
        urls: &[&str],
        hash: &str,
        size: u64,
    ) -> Result<PathBuf, InstallError> {
        use inst_fetch::download::{DownloadObserver, Progress};
        use inst_fetch::transport::{TransportConfig, UreqTransport};
        use inst_fetch::{Cache, ContentHash, DownloadRequest, Downloader, ItemInfo, RetryPolicy};

        let expected =
            ContentHash::parse(hash).map_err(|e| InstallError::unexpected(e.to_string()))?;
        let cache = Cache::open(crate::platform::cache_dir())
            .map_err(|e| InstallError::io("opening the download cache", &e))?;
        let transport =
            UreqTransport::new(&TransportConfig::default()).map_err(InstallError::unexpected)?;
        let mut downloader = Downloader::new(&transport, RetryPolicy::default());
        let req = DownloadRequest {
            sources: urls,
            expected_hash: expected,
            expected_size: size,
        };
        let info = ItemInfo {
            vendor: String::new(),
            product: p.name.to_owned(),
            version: p.version.to_owned(),
            arch: format!("{:?}", self.c.env.arch),
            platform: format!("{:?}", self.c.env.os),
            file_name: p.file_name.to_owned(),
        };
        struct Obs<'x, 'y> {
            cx: &'x mut Install<'y>,
            name: &'static str,
            last: u32,
        }
        impl DownloadObserver for Obs<'_, '_> {
            fn progress(&mut self, pr: Progress) {
                let pct = (pr.downloaded * 100 / pr.total.max(1)) as u32;
                if pct != self.last {
                    self.last = pct;
                    self.cx.progress(pr.downloaded, pr.total);
                    self.cx.detail(format!("{} {pct}%", self.name));
                }
            }
            fn cancelled(&self) -> bool {
                self.cx.c.is_cancelled()
            }
            fn note(&mut self, message: &str) {
                log!(self.cx.c.logger, Info, "download", "{message}");
            }
        }
        log!(self.c.logger, Info, "download", "downloading prerequisite"; "id" => p.id, "size" => size);
        let name = p.name;
        let mut obs = Obs {
            cx: self,
            name,
            last: u32::MAX,
        };
        let result = cache.fetch(&mut downloader, &req, &info, &mut obs);
        match result {
            Ok(path) => {
                // Copy out of the cache so cleaning cannot race the installer.
                let dir = std::env::temp_dir().join(format!(
                    "inst-prereq-{:x}",
                    inst_fsx::atomic::unique_token()
                ));
                std::fs::create_dir_all(&dir)
                    .map_err(|e| InstallError::io("preparing a prerequisite", &e))?;
                let dest = dir.join(p.file_name);
                std::fs::copy(&path, &dest)
                    .map_err(|e| InstallError::io("preparing a prerequisite", &e))?;
                Ok(dest)
            }
            Err(inst_fetch::DownloadError::Cancelled) => Err(InstallError::cancelled()),
            Err(e) if e.is_integrity_failure() => Err(InstallError::new(
                ErrorKind::PrerequisiteVerify {
                    name: name.to_owned(),
                },
                e.to_string(),
            )),
            Err(e) => Err(InstallError::new(
                ErrorKind::PrerequisiteDownload {
                    name: name.to_owned(),
                },
                e.to_string(),
            )),
        }
    }

    #[cfg(not(feature = "download"))]
    fn download(
        &mut self,
        p: &Prerequisite,
        _: &[&str],
        _: &str,
        _: u64,
    ) -> Result<PathBuf, InstallError> {
        Err(InstallError::new(
            ErrorKind::PrerequisiteDownload {
                name: p.name.to_owned(),
            },
            "this installer was built without download support",
        ))
    }

    fn extract_embedded(
        &mut self,
        p: &Prerequisite,
        rel: &'static str,
    ) -> Result<PathBuf, InstallError> {
        use inst_payload::read::{
            self, ExtractBuffers, ExtractObserver, ExtractOptions, FileAction,
        };
        struct Only(&'static str);
        impl ExtractObserver for Only {
            fn begin_file(
                &mut self,
                path: inst_fsx::RelPath<'_>,
                _: &inst_payload::FileEntry,
                _: &Path,
            ) -> std::io::Result<FileAction> {
                Ok(if path.as_str() == self.0 {
                    FileAction::Extract
                } else {
                    FileAction::Skip
                })
            }
        }
        let index = &self.payload.index;
        let file = index
            .files
            .iter()
            .find(|f| index.file_path(f) == rel)
            .ok_or_else(|| {
                InstallError::new(
                    ErrorKind::Damaged,
                    format!("embedded package {rel} missing"),
                )
            })?;
        let dir = std::env::temp_dir().join(format!(
            "inst-prereq-{:x}",
            inst_fsx::atomic::unique_token()
        ));
        std::fs::create_dir_all(&dir)
            .map_err(|e| InstallError::io("preparing a prerequisite", &e))?;
        let mut f = std::fs::File::open(&self.exe)
            .map_err(|e| InstallError::io("opening the setup file", &e))?;
        let block = file.block as usize;
        let raw = self
            .payload
            .open_block(&mut f, block)
            .map_err(|e| InstallError::io("reading the setup file", &e))?;
        read::extract_block(
            &self.payload,
            block,
            raw,
            &dir,
            &mut Only(rel),
            &mut ExtractBuffers::default(),
            ExtractOptions::default(),
        )?;
        log!(self.c.logger, Info, "prereq", "embedded package extracted"; "id" => p.id);
        Ok(inst_fsx::RelPath::new(rel)
            .map_err(|e| InstallError::unexpected(e.to_string()))?
            .to_path_under(&dir))
    }

    /// Runs the silent installers of missing prerequisites.
    pub fn install_prerequisites(&mut self) -> Result<(), InstallError> {
        let list = self.c.app.prerequisites;
        for (i, p) in list.iter().enumerate() {
            if !self.prereq_missing[i] {
                continue;
            }
            let Some(file) = self.prereq_files[i].clone() else {
                return Err(InstallError::new(
                    ErrorKind::PrerequisiteInstall {
                        name: p.name.to_owned(),
                    },
                    "package was not acquired",
                ));
            };
            self.detail(p.name);
            let mut args = Vec::with_capacity(p.args.len() + 4);
            let (program, mut base): (PathBuf, Vec<String>) = match p.kind {
                InstallKind::Exe => (file.clone(), Vec::new()),
                InstallKind::Msi => (
                    msiexec(),
                    vec![
                        "/i".into(),
                        file.to_string_lossy().into_owned(),
                        "/qn".into(),
                        "/norestart".into(),
                    ],
                ),
            };
            for a in p.args {
                args.push(match a {
                    Arg::Lit(s) => (*s).to_owned(),
                    Arg::Input(id) => self.resolve(&crate::spec::Value::Input(id))?,
                    Arg::Secret(id) => {
                        self.resolve(&crate::spec::Value::Secret(crate::spec::Secret::Input(id)))?
                    }
                });
            }
            base.extend(args);
            log!(self.c.logger, Info, "prereq", "installing prerequisite"; "id" => p.id, "version" => p.version);
            let status = std::process::Command::new(&program)
                .args(&base)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map_err(|e| {
                    InstallError::new(
                        ErrorKind::PrerequisiteInstall {
                            name: p.name.to_owned(),
                        },
                        e.to_string(),
                    )
                })?;
            let _ = std::fs::remove_dir_all(file.parent().unwrap_or(&file));
            let code = status.code().unwrap_or(-1);
            if p.reboot_codes.contains(&code) {
                self.reboot_required = true;
            } else if !(p.success_codes.contains(&code)
                || p.already_installed_codes.contains(&code))
            {
                return Err(InstallError::new(
                    ErrorKind::PrerequisiteInstall {
                        name: p.name.to_owned(),
                    },
                    format!("installer exited with code {code}"),
                ));
            }
            self.prereqs_installed
                .push((p.id.to_owned(), p.version.to_owned()));
            self.prereq_missing[i] = false;
            self.progress(i as u64 + 1, list.len() as u64);
        }
        Ok(())
    }
}

fn msiexec() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(|r| PathBuf::from(r).join("System32").join("msiexec.exe"))
        .unwrap_or_else(|| PathBuf::from("msiexec.exe"))
}

/// Verifies the Authenticode signature on Windows when a publisher is
/// expected. The content hash was already verified against the value pinned
/// at build time, so this is defence in depth.
#[cfg(windows)]
#[allow(unsafe_code)]
fn verify_signature(path: &Path, p: &Prerequisite) -> Result<(), InstallError> {
    use windows_sys::Win32::Security::WinTrust::{
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
        WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY,
        WTD_UI_NONE, WinVerifyTrust,
    };
    if p.publisher.is_none() {
        return Ok(());
    }
    let wide = crate::platform::wide(path.as_os_str());
    let mut file_info: WINTRUST_FILE_INFO = unsafe { std::mem::zeroed() };
    file_info.cbStruct = std::mem::size_of::<WINTRUST_FILE_INFO>() as u32;
    file_info.pcwszFilePath = wide.as_ptr();
    let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
    data.dwUIChoice = WTD_UI_NONE;
    data.fdwRevocationChecks = WTD_REVOKE_NONE;
    data.dwUnionChoice = WTD_CHOICE_FILE;
    data.Anonymous = WINTRUST_DATA_0 {
        pFile: &mut file_info,
    };
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: structures are fully initialized and outlive the calls.
    let status = unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        )
    };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: releases the state allocated by the verify call.
    unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        )
    };
    if status != 0 {
        return Err(InstallError::new(
            ErrorKind::PrerequisiteVerify {
                name: p.name.to_owned(),
            },
            format!("Authenticode verification failed (0x{status:08x})"),
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn verify_signature(_path: &Path, _p: &Prerequisite) -> Result<(), InstallError> {
    Ok(())
}
