//! Atomic file creation: write to a uniquely named sibling, then rename.
//!
//! The temporary file is created with `create_new` (O_EXCL / CREATE_NEW), so a
//! pre-planted file or symlink with the same name can never be followed or
//! truncated. Readers observe either the old file or the complete new one.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Returns a sibling path `.<name>.<unique>.<suffix>` for `target`.
pub fn sibling_temp_path(target: &Path, suffix: &str) -> io::Result<PathBuf> {
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let unique = unique_token();
    let mut file_name = std::ffi::OsString::with_capacity(name.len() + 32);
    file_name.push(".");
    file_name.push(name);
    file_name.push(format!(".{unique:x}.{suffix}"));
    Ok(target.with_file_name(file_name))
}

/// Process-unique token mixing the pid, a counter and the clock. Uniqueness is
/// only a convenience: callers always create with `create_new` and retry.
pub fn unique_token() -> u64 {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let pid = u64::from(std::process::id());
    // SplitMix64 finalizer for good bit dispersion.
    let mut z = nanos ^ (pid << 32) ^ n.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Creates a new file exclusively, retrying with fresh names on collision.
pub fn create_exclusive_temp(target: &Path, suffix: &str) -> io::Result<(File, PathBuf)> {
    for _ in 0..16 {
        let path = sibling_temp_path(target, suffix)?;
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create a unique temporary file",
    ))
}

/// A file being written atomically. Dropping it without [`commit`] removes
/// the temporary file.
///
/// [`commit`]: AtomicFile::commit
pub struct AtomicFile {
    file: Option<File>,
    temp: PathBuf,
    target: PathBuf,
}

impl AtomicFile {
    pub fn create(target: impl Into<PathBuf>) -> io::Result<Self> {
        let target = target.into();
        let (file, temp) = create_exclusive_temp(&target, "partial")?;
        Ok(AtomicFile {
            file: Some(file),
            temp,
            target,
        })
    }

    #[inline]
    pub fn target(&self) -> &Path {
        &self.target
    }

    #[inline]
    pub fn temp_path(&self) -> &Path {
        &self.temp
    }

    #[inline]
    pub fn file(&mut self) -> &mut File {
        self.file.as_mut().expect("file is present until commit")
    }

    /// Flushes to stable storage (if `durable`) and renames over the target.
    pub fn commit(mut self, durable: bool) -> io::Result<()> {
        let file = self.file.take().expect("file is present until commit");
        if durable {
            file.sync_all()?;
        }
        drop(file);
        fs::rename(&self.temp, &self.target)?;
        if durable {
            sync_parent_dir(&self.target);
        }
        Ok(())
    }
}

impl Write for AtomicFile {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file().write(buf)
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.file().write_all(buf)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        self.file().flush()
    }
}

impl Drop for AtomicFile {
    fn drop(&mut self) {
        if self.file.take().is_some() {
            let _ = fs::remove_file(&self.temp);
        }
    }
}

/// Writes `bytes` to `target` atomically.
pub fn write_atomic(target: &Path, bytes: &[u8], durable: bool) -> io::Result<()> {
    let mut file = AtomicFile::create(target)?;
    file.write_all(bytes)?;
    file.commit(durable)
}

/// Makes a rename durable on POSIX by syncing the directory entry.
fn sync_parent_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent()
        && let Ok(dir) = File::open(parent)
    {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_replaces_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("a.txt");
        fs::write(&target, b"old").expect("write");
        let mut f = AtomicFile::create(&target).expect("create");
        f.write_all(b"new").expect("write");
        assert_eq!(fs::read(&target).expect("read"), b"old");
        f.commit(true).expect("commit");
        assert_eq!(fs::read(&target).expect("read"), b"new");
        assert_eq!(fs::read_dir(dir.path()).expect("ls").count(), 1);
    }

    #[test]
    fn drop_removes_temp() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("a.txt");
        {
            let mut f = AtomicFile::create(&target).expect("create");
            f.write_all(b"partial").expect("write");
        }
        assert!(!target.exists());
        assert_eq!(fs::read_dir(dir.path()).expect("ls").count(), 0);
    }

    #[test]
    fn temp_names_are_unique() {
        let t = Path::new("/x/file.bin");
        let a = sibling_temp_path(t, "partial").expect("path");
        let b = sibling_temp_path(t, "partial").expect("path");
        assert_ne!(a, b);
        assert!(
            a.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(".file.bin."))
        );
    }
}
