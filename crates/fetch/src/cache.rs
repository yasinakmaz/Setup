//! Content-addressed cache for prerequisites and download payloads.
//!
//! Layout:
//!
//! ```text
//! <root>/objects/<algo>/<hex[..2]>/<hex>        verified content
//! <root>/objects/<algo>/<hex[..2]>/<hex>.info   descriptive metadata
//! <root>/partial/<algo>-<hex>.part|.meta        interrupted downloads
//! <root>/locks/<algo>-<hex>.lock                in-use / download locks
//! ```
//!
//! Entries are keyed by content hash; the metadata associates them with
//! vendor, product, version, architecture and platform for display. Every
//! lookup verifies size and hash: a corrupted entry is removed and reported
//! as a miss. Cleaning skips entries whose lock is held, so it never breaks
//! a running build or download.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use inst_wire::{Reader, Writer};

use crate::download::{DownloadError, DownloadObserver, DownloadRequest, Downloader};
use crate::hash::{ContentHash, hash_file};

/// Descriptive metadata of a cached item.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemInfo {
    pub vendor: String,
    pub product: String,
    pub version: String,
    pub arch: String,
    pub platform: String,
    pub file_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheEntry {
    pub hash: ContentHash,
    pub path: PathBuf,
    pub size: u64,
    pub info: ItemInfo,
    pub created: u64,
    pub last_used: u64,
    pub integrity: Integrity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Integrity {
    /// Not checked by this listing (checking is expensive).
    Unchecked,
    Valid,
    Corrupt,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: u64,
    pub partial_bytes: u64,
}

pub struct Cache {
    root: PathBuf,
}

/// An acquired lock; released on drop (also when the process dies).
pub struct CacheLock {
    _file: File,
}

const INFO_MAGIC: &[u8; 8] = b"INSTCINF";

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Cache {
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Cache> {
        let root = root.into();
        for sub in ["objects", "partial", "locks"] {
            fs::create_dir_all(root.join(sub))?;
        }
        Ok(Cache { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn partial_dir(&self) -> PathBuf {
        self.root.join("partial")
    }

    pub fn object_path(&self, hash: &ContentHash) -> PathBuf {
        let hex = hash.hex();
        self.root
            .join("objects")
            .join(hash.algorithm())
            .join(&hex[..2])
            .join(hex)
    }

    fn info_path(&self, hash: &ContentHash) -> PathBuf {
        let mut p = self.object_path(hash).into_os_string();
        p.push(".info");
        PathBuf::from(p)
    }

    fn lock_path(&self, hash: &ContentHash) -> PathBuf {
        self.root.join("locks").join(format!("{}.lock", hash.key()))
    }

    fn open_lock_file(&self, hash: &ContentHash) -> io::Result<File> {
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.lock_path(hash))
    }

    /// Takes a shared lock marking the entry as in use (blocks cleaning).
    pub fn lock_shared(&self, hash: &ContentHash) -> io::Result<CacheLock> {
        let file = self.open_lock_file(hash)?;
        file.lock_shared()?;
        Ok(CacheLock { _file: file })
    }

    /// Takes the exclusive lock used while downloading or deleting.
    pub fn lock_exclusive(&self, hash: &ContentHash) -> io::Result<CacheLock> {
        let file = self.open_lock_file(hash)?;
        file.lock()?;
        Ok(CacheLock { _file: file })
    }

    fn try_lock_exclusive(&self, hash: &ContentHash) -> io::Result<Option<CacheLock>> {
        let file = self.open_lock_file(hash)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(CacheLock { _file: file })),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(e)) => Err(e),
        }
    }

    /// Returns the path of a verified entry, or `None` if missing. A corrupt
    /// entry is deleted and reported as missing.
    pub fn get_verified(&self, hash: &ContentHash, size: u64) -> io::Result<Option<PathBuf>> {
        let path = self.object_path(hash);
        match fs::metadata(&path) {
            Ok(m) if m.len() == size => {}
            Ok(_) => {
                self.remove_entry(hash)?;
                return Ok(None);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        }
        let (actual, len) = hash_file(&path, hash)?;
        if actual != *hash || len != size {
            self.remove_entry(hash)?;
            return Ok(None);
        }
        self.touch(hash);
        Ok(Some(path))
    }

    /// Returns the verified entry, downloading it first when missing.
    /// Holds the exclusive lock for the duration of the download so two
    /// processes never download the same object concurrently.
    pub fn fetch(
        &self,
        downloader: &mut Downloader<'_>,
        req: &DownloadRequest<'_>,
        info: &ItemInfo,
        observer: &mut dyn DownloadObserver,
    ) -> Result<PathBuf, DownloadError> {
        let _lock = self.lock_exclusive(&req.expected_hash)?;
        if let Some(path) = self.get_verified(&req.expected_hash, req.expected_size)? {
            return Ok(path);
        }
        let dest = self.object_path(&req.expected_hash);
        downloader.download(req, &self.partial_dir(), &dest, observer)?;
        self.write_info(&req.expected_hash, info, req.expected_size)?;
        Ok(dest)
    }

    /// Inserts an existing verified file (e.g. an embedded package).
    pub fn insert_file(&self, source: &Path, hash: &ContentHash, info: &ItemInfo) -> io::Result<PathBuf> {
        let _lock = self.lock_exclusive(hash)?;
        let (actual, size) = hash_file(source, hash)?;
        if actual != *hash {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "hash mismatch on insert"));
        }
        let dest = self.object_path(hash);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = inst_fsx::atomic::sibling_temp_path(&dest, "insert")?;
        fs::copy(source, &tmp)?;
        fs::rename(&tmp, &dest)?;
        self.write_info(hash, info, size)?;
        Ok(dest)
    }

    fn write_info(&self, hash: &ContentHash, info: &ItemInfo, size: u64) -> io::Result<()> {
        let now = now_secs();
        let mut buf = Vec::with_capacity(256);
        Writer::new(&mut buf)
            .raw(INFO_MAGIC)
            .str(&info.vendor)
            .str(&info.product)
            .str(&info.version)
            .str(&info.arch)
            .str(&info.platform)
            .str(&info.file_name)
            .u64(size)
            .u64(now)
            .u64(now);
        inst_fsx::write_atomic(&self.info_path(hash), &buf, false)
    }

    fn read_info(path: &Path) -> Option<(ItemInfo, u64, u64, u64)> {
        let bytes = fs::read(path).ok()?;
        let mut r = Reader::new(&bytes).with_max_len(4096);
        if r.take(8).ok()? != INFO_MAGIC {
            return None;
        }
        let info = ItemInfo {
            vendor: r.str().ok()?.to_owned(),
            product: r.str().ok()?.to_owned(),
            version: r.str().ok()?.to_owned(),
            arch: r.str().ok()?.to_owned(),
            platform: r.str().ok()?.to_owned(),
            file_name: r.str().ok()?.to_owned(),
        };
        Some((info, r.u64().ok()?, r.u64().ok()?, r.u64().ok()?))
    }

    /// Updates the last-used timestamp (best effort).
    fn touch(&self, hash: &ContentHash) {
        let path = self.info_path(hash);
        if let Ok(mut bytes) = fs::read(&path)
            && bytes.len() >= 8
        {
            let len = bytes.len();
            bytes[len - 8..].copy_from_slice(&now_secs().to_le_bytes());
            let _ = inst_fsx::write_atomic(&path, &bytes, false);
        }
    }

    fn remove_entry(&self, hash: &ContentHash) -> io::Result<()> {
        for p in [self.object_path(hash), self.info_path(hash)] {
            match fs::remove_file(&p) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Lists cached entries. With `verify`, every entry is re-hashed.
    pub fn entries(&self, verify: bool) -> io::Result<Vec<CacheEntry>> {
        let mut out = Vec::new();
        let objects = self.root.join("objects");
        for algo_dir in read_dir_sorted(&objects)? {
            for shard in read_dir_sorted(&algo_dir)? {
                for file in read_dir_sorted(&shard)? {
                    let Some(name) = file.file_name().and_then(|n| n.to_str()) else { continue };
                    if name.contains('.') {
                        continue; // .info files and temporaries
                    }
                    let algo = algo_dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                    let Ok(hash) = ContentHash::parse(&format!("{algo}:{name}")) else { continue };
                    let size = fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
                    let (info, _, created, last_used) =
                        Cache::read_info(&self.info_path(&hash)).unwrap_or_default();
                    let integrity = if verify {
                        match hash_file(&file, &hash) {
                            Ok((h, _)) if h == hash => Integrity::Valid,
                            _ => Integrity::Corrupt,
                        }
                    } else {
                        Integrity::Unchecked
                    };
                    out.push(CacheEntry {
                        hash,
                        path: file,
                        size,
                        info,
                        created,
                        last_used,
                        integrity,
                    });
                }
            }
        }
        Ok(out)
    }

    pub fn stats(&self) -> io::Result<CacheStats> {
        let entries = self.entries(false)?;
        let partial_bytes = read_dir_sorted(&self.partial_dir())?
            .iter()
            .filter_map(|p| fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
        Ok(CacheStats {
            entries: entries.len(),
            bytes: entries.iter().map(|e| e.size).sum(),
            partial_bytes,
        })
    }

    /// Removes entries not used for `max_age`. Locked entries are skipped.
    /// Returns the number of bytes freed.
    pub fn clear_unused(&self, max_age: Duration) -> io::Result<u64> {
        let cutoff = now_secs().saturating_sub(max_age.as_secs());
        self.clear_where(|e| e.last_used < cutoff || e.integrity == Integrity::Corrupt)
    }

    /// Removes every entry that is not locked, plus stale partial downloads.
    pub fn clear_all(&self) -> io::Result<u64> {
        let freed = self.clear_where(|_| true)?;
        let mut partial = 0u64;
        for p in read_dir_sorted(&self.partial_dir())? {
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else { continue };
            let key = name.trim_end_matches(".part").trim_end_matches(".meta");
            let Some((algo, hex)) = key.split_once('-') else { continue };
            let Ok(hash) = ContentHash::parse(&format!("{algo}:{hex}")) else { continue };
            if let Some(_lock) = self.try_lock_exclusive(&hash)? {
                partial += fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                let _ = fs::remove_file(&p);
            }
        }
        Ok(freed + partial)
    }

    fn clear_where(&self, pred: impl Fn(&CacheEntry) -> bool) -> io::Result<u64> {
        let mut freed = 0;
        for entry in self.entries(false)? {
            if !pred(&entry) {
                continue;
            }
            if let Some(_lock) = self.try_lock_exclusive(&entry.hash)? {
                self.remove_entry(&entry.hash)?;
                freed += entry.size;
            }
        }
        Ok(freed)
    }
}

fn read_dir_sorted(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = match fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).collect::<Vec<_>>(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    out.sort();
    Ok(out)
}
