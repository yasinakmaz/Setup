//! Resumable, verified downloads with mirror fallback.
//!
//! A partial download lives in `<dir>/<algo>-<hex>.part` next to a metadata
//! file `<…>.meta`. The file name is derived from the **expected content
//! hash**, so a partial can only ever be resumed towards the exact same
//! bytes; a different binary with the same name cannot be continued by
//! mistake. The metadata records the expected size, the source URL and the
//! server validator (ETag / Last-Modified) used for `If-Range`.
//!
//! Completion is atomic: the partial is verified (size + hash) and only then
//! renamed to its destination. A binary that fails verification is deleted
//! and never returned.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use inst_wire::{Reader, Writer};

use crate::hash::{ContentHash, StreamHasher};
use crate::transport::{Transport, TransportError};

/// What to download.
#[derive(Clone, Debug)]
pub struct DownloadRequest<'a> {
    /// Candidate sources in priority order.
    pub sources: &'a [&'a str],
    pub expected_hash: ContentHash,
    pub expected_size: u64,
}

#[derive(Clone, Debug)]
pub struct RetryPolicy {
    /// Attempts per source that make no progress before moving on.
    pub attempts_per_source: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            attempts_per_source: 4,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(8),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    pub downloaded: u64,
    pub total: u64,
    /// Bytes that were already present from an earlier, interrupted run.
    pub resumed_from: u64,
    pub source_index: usize,
}

/// Why one source was abandoned.
#[derive(Debug)]
pub enum SourceFailure {
    /// HTTP status that will not change by retrying (404, 403, 410…).
    Status(u16),
    /// Retries exhausted on transient errors.
    Transient(String),
    /// Server reports a different size than expected.
    SizeMismatch {
        expected: u64,
        reported: u64,
    },
    /// Content did not match the expected hash.
    Integrity {
        actual: ContentHash,
    },
    Forbidden(String),
}

impl std::fmt::Display for SourceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceFailure::Status(s) => write!(f, "HTTP {s}"),
            SourceFailure::Transient(e) => write!(f, "{e}"),
            SourceFailure::SizeMismatch { expected, reported } => {
                write!(
                    f,
                    "size mismatch: expected {expected} bytes, server reports {reported}"
                )
            }
            SourceFailure::Integrity { actual } => write!(f, "checksum mismatch (got {actual})"),
            SourceFailure::Forbidden(e) => write!(f, "{e}"),
        }
    }
}

#[derive(Debug)]
pub enum DownloadError {
    NoSources,
    /// Every source failed; per-source reasons in priority order.
    AllSourcesFailed(Vec<(String, SourceFailure)>),
    Cancelled,
    Io(io::Error),
}

impl DownloadError {
    /// True when at least one source delivered bytes that failed
    /// verification — shown to the user as "could not be verified".
    pub fn is_integrity_failure(&self) -> bool {
        matches!(self, DownloadError::AllSourcesFailed(v)
            if v.iter().any(|(_, f)| matches!(f, SourceFailure::Integrity { .. } | SourceFailure::SizeMismatch { .. })))
    }
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::NoSources => f.write_str("no download source configured"),
            DownloadError::AllSourcesFailed(v) => {
                f.write_str("all download sources failed")?;
                for (url, why) in v {
                    write!(f, "; {url}: {why}")?;
                }
                Ok(())
            }
            DownloadError::Cancelled => f.write_str("download cancelled"),
            DownloadError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for DownloadError {}

impl From<io::Error> for DownloadError {
    fn from(e: io::Error) -> Self {
        DownloadError::Io(e)
    }
}

const META_MAGIC: &[u8; 8] = b"INSTPART";
const META_VERSION: u16 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PartialMeta {
    hash_key: String,
    size: u64,
    url: String,
    validator: Option<String>,
}

impl PartialMeta {
    fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(128);
        Writer::new(&mut buf)
            .raw(META_MAGIC)
            .u16(META_VERSION)
            .str(&self.hash_key)
            .u64(self.size)
            .str(&self.url)
            .opt_str(self.validator.as_deref());
        buf
    }

    fn decode(bytes: &[u8]) -> Option<PartialMeta> {
        let mut r = Reader::new(bytes).with_max_len(64 << 10);
        if r.take(8).ok()? != META_MAGIC || r.u16().ok()? != META_VERSION {
            return None;
        }
        let meta = PartialMeta {
            hash_key: r.str().ok()?.to_owned(),
            size: r.u64().ok()?,
            url: r.str().ok()?.to_owned(),
            validator: r.opt_str().ok()?.map(str::to_owned),
        };
        r.finish().ok()?;
        Some(meta)
    }
}

/// Paths of the partial download for a request.
pub fn partial_paths(dir: &Path, hash: &ContentHash) -> (PathBuf, PathBuf) {
    let key = hash.key();
    (
        dir.join(format!("{key}.part")),
        dir.join(format!("{key}.meta")),
    )
}

/// Cancellation and progress hooks.
pub trait DownloadObserver {
    fn progress(&mut self, _p: Progress) {}
    fn cancelled(&self) -> bool {
        false
    }
    /// Informational events (retries, source switches) for logging.
    fn note(&mut self, _message: &str) {}
}

pub struct NoopObserver;
impl DownloadObserver for NoopObserver {}

pub struct Downloader<'t> {
    transport: &'t dyn Transport,
    policy: RetryPolicy,
    buffer: Vec<u8>,
}

enum Attempt {
    Complete,
    /// Made progress; reconnect immediately without counting a failure.
    Progressed,
    Retry(String),
    Abandon(SourceFailure),
}

impl<'t> Downloader<'t> {
    pub fn new(transport: &'t dyn Transport, policy: RetryPolicy) -> Self {
        Downloader {
            transport,
            policy,
            buffer: vec![0u8; 256 << 10],
        }
    }

    /// Downloads to `dest`, resuming a partial download in `partial_dir`.
    /// On success `dest` contains exactly the expected bytes.
    pub fn download(
        &mut self,
        req: &DownloadRequest<'_>,
        partial_dir: &Path,
        dest: &Path,
        observer: &mut dyn DownloadObserver,
    ) -> Result<(), DownloadError> {
        if req.sources.is_empty() {
            return Err(DownloadError::NoSources);
        }
        fs::create_dir_all(partial_dir)?;
        let (part_path, meta_path) = partial_paths(partial_dir, &req.expected_hash);
        let hash_key = req.expected_hash.key();

        // Validate existing partial state; discard anything inconsistent.
        let mut meta = fs::read(&meta_path)
            .ok()
            .and_then(|b| PartialMeta::decode(&b))
            .filter(|m| m.hash_key == hash_key && m.size == req.expected_size);
        let existing_len = fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);
        if meta.is_none() || existing_len > req.expected_size {
            let _ = fs::remove_file(&part_path);
            meta = None;
        }
        let mut meta = meta.unwrap_or_else(|| PartialMeta {
            hash_key: hash_key.clone(),
            size: req.expected_size,
            ..PartialMeta::default()
        });
        let resumed_from = fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);

        let mut part = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&part_path)?;
        // (hasher over part[..hashed_len]) kept across reconnects.
        let mut hash_state: Option<(StreamHasher, u64)> = None;
        let mut failures = Vec::new();

        for (source_index, url) in req.sources.iter().enumerate() {
            let mut failed_attempts = 0u32;
            let mut backoff = self.policy.initial_backoff;
            let outcome = loop {
                if observer.cancelled() {
                    return Err(DownloadError::Cancelled);
                }
                let attempt = self.attempt(
                    req,
                    url,
                    source_index,
                    resumed_from,
                    &mut part,
                    &mut meta,
                    &meta_path,
                    &mut hash_state,
                    observer,
                )?;
                match attempt {
                    Attempt::Complete => break Ok(()),
                    Attempt::Progressed => {
                        failed_attempts = 0;
                        backoff = self.policy.initial_backoff;
                    }
                    Attempt::Retry(why) => {
                        failed_attempts += 1;
                        if failed_attempts >= self.policy.attempts_per_source {
                            break Err(SourceFailure::Transient(why));
                        }
                        observer.note(&format!("retrying {url} in {backoff:?}: {why}"));
                        sleep_cancellable(backoff, observer)?;
                        backoff = (backoff * 2).min(self.policy.max_backoff);
                    }
                    Attempt::Abandon(failure) => break Err(failure),
                }
            };
            match outcome {
                Ok(()) => {
                    drop(part);
                    // Atomic completion.
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::rename(&part_path, dest)?;
                    let _ = fs::remove_file(&meta_path);
                    return Ok(());
                }
                Err(failure) => {
                    if matches!(failure, SourceFailure::Integrity { .. }) {
                        // Never resume bytes that produced a bad hash.
                        part.set_len(0)?;
                        hash_state = None;
                        meta.validator = None;
                    }
                    observer.note(&format!("source {url} failed: {failure}"));
                    failures.push(((*url).to_owned(), failure));
                }
            }
        }
        Err(DownloadError::AllSourcesFailed(failures))
    }

    #[allow(clippy::too_many_arguments)]
    fn attempt(
        &mut self,
        req: &DownloadRequest<'_>,
        url: &str,
        source_index: usize,
        resumed_from: u64,
        part: &mut File,
        meta: &mut PartialMeta,
        meta_path: &Path,
        hash_state: &mut Option<(StreamHasher, u64)>,
        observer: &mut dyn DownloadObserver,
    ) -> Result<Attempt, DownloadError> {
        let size = req.expected_size;
        let mut offset = part.metadata()?.len();
        if offset > size {
            part.set_len(0)?;
            offset = 0;
            *hash_state = None;
        }
        if offset == size {
            return self.finish(req, part, hash_state);
        }

        let if_range = if meta.url == url {
            meta.validator.as_deref()
        } else {
            None
        };
        let resp = match self.transport.get(url, offset, if_range) {
            Ok(r) => r,
            Err(TransportError::Forbidden(e)) => {
                return Ok(Attempt::Abandon(SourceFailure::Forbidden(e)));
            }
            Err(e) => return Ok(Attempt::Retry(e.to_string())),
        };
        match resp.status {
            206 if resp.range_start == offset => {}
            206 => {
                // Server returned a different range than requested; start over.
                part.set_len(0)?;
                *hash_state = None;
                return Ok(Attempt::Retry("unexpected range in response".into()));
            }
            200 => {
                if offset > 0 {
                    // Range ignored or content changed (If-Range mismatch).
                    observer.note("server sent the full file; restarting download");
                    part.set_len(0)?;
                    offset = 0;
                    *hash_state = None;
                }
            }
            416 => {
                part.set_len(0)?;
                *hash_state = None;
                return Ok(Attempt::Retry("range not satisfiable".into()));
            }
            408 | 425 | 429 | 500..=599 => {
                return Ok(Attempt::Retry(format!("HTTP {}", resp.status)));
            }
            other => return Ok(Attempt::Abandon(SourceFailure::Status(other))),
        }
        if let Some(total) = resp.total_len
            && total != size
            && !(resp.status == 200 && total == 0)
        {
            return Ok(Attempt::Abandon(SourceFailure::SizeMismatch {
                expected: size,
                reported: total,
            }));
        }

        // Record source + validator before writing body bytes.
        if meta.url != url || meta.validator != resp.validator {
            meta.url = url.to_owned();
            meta.validator = resp.validator.clone();
            inst_fsx::write_atomic(meta_path, &meta.encode(), false)?;
        }

        // Bring the hasher up to `offset` (re-hash the prefix only when
        // resuming a download from a previous process).
        let (mut hasher, hashed) = match hash_state.take() {
            Some((h, len)) if len == offset => (h, len),
            _ => (req.expected_hash.hasher(), 0),
        };
        let mut hashed = hashed;
        if hashed < offset {
            part.seek(SeekFrom::Start(hashed))?;
            let mut prefix = (&mut *part).take(offset - hashed);
            loop {
                let n = prefix.read(&mut self.buffer)?;
                if n == 0 {
                    break;
                }
                hasher.update(&self.buffer[..n]);
                hashed += n as u64;
            }
            if hashed != offset {
                part.set_len(0)?;
                return Ok(Attempt::Retry("partial file changed".into()));
            }
        }
        part.seek(SeekFrom::Start(offset))?;

        let mut body = resp.body;
        let mut received = 0u64;
        let result = loop {
            if observer.cancelled() {
                part.flush()?;
                *hash_state = Some((hasher, offset + received));
                return Err(DownloadError::Cancelled);
            }
            let n = match body.read(&mut self.buffer) {
                Ok(0) => break Ok(()),
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => break Err(e),
            };
            let allowed = (size - offset - received).min(n as u64) as usize;
            if allowed < n {
                break Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "server sent more data than expected",
                ));
            }
            part.write_all(&self.buffer[..n])?;
            hasher.update(&self.buffer[..n]);
            received += n as u64;
            observer.progress(Progress {
                downloaded: offset + received,
                total: size,
                resumed_from,
                source_index,
            });
        };
        part.flush()?;
        *hash_state = Some((hasher, offset + received));
        let now = offset + received;
        match result {
            Ok(()) if now == size => self.finish(req, part, hash_state),
            Ok(()) => Ok(if received > 0 {
                Attempt::Progressed
            } else {
                Attempt::Retry("connection closed early".into())
            }),
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                part.set_len(0)?;
                *hash_state = None;
                Ok(Attempt::Abandon(SourceFailure::SizeMismatch {
                    expected: size,
                    reported: now + 1,
                }))
            }
            Err(e) => Ok(if received > 0 {
                Attempt::Progressed
            } else {
                Attempt::Retry(e.to_string())
            }),
        }
    }

    fn finish(
        &mut self,
        req: &DownloadRequest<'_>,
        part: &mut File,
        hash_state: &mut Option<(StreamHasher, u64)>,
    ) -> Result<Attempt, DownloadError> {
        let size = req.expected_size;
        let actual = match hash_state.take() {
            Some((h, len)) if len == size => h.finalize(),
            _ => {
                part.seek(SeekFrom::Start(0))?;
                let mut h = req.expected_hash.hasher();
                let mut total = 0u64;
                loop {
                    let n = part.read(&mut self.buffer)?;
                    if n == 0 {
                        break;
                    }
                    h.update(&self.buffer[..n]);
                    total += n as u64;
                }
                if total != size {
                    part.set_len(0)?;
                    return Ok(Attempt::Retry("size changed during verification".into()));
                }
                h.finalize()
            }
        };
        if actual == req.expected_hash {
            part.sync_all()?;
            Ok(Attempt::Complete)
        } else {
            Ok(Attempt::Abandon(SourceFailure::Integrity { actual }))
        }
    }
}

fn sleep_cancellable(
    total: Duration,
    observer: &mut dyn DownloadObserver,
) -> Result<(), DownloadError> {
    let step = Duration::from_millis(50);
    let mut left = total;
    while !left.is_zero() {
        if observer.cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let d = left.min(step);
        std::thread::sleep(d);
        left -= d;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_roundtrip_and_rejects_garbage() {
        let m = PartialMeta {
            hash_key: "sha256-00".into(),
            size: 42,
            url: "https://x".into(),
            validator: Some("\"etag\"".into()),
        };
        assert_eq!(PartialMeta::decode(&m.encode()), Some(m));
        assert_eq!(PartialMeta::decode(b"garbage"), None);
    }
}
