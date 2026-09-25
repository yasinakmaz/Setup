//! Streaming payload extraction (installer side).
//!
//! Memory use is bounded by the codec window plus two reused buffers,
//! independent of payload size. Every file is written to an exclusive
//! temporary sibling, hashed while written, verified, and only then renamed
//! into place.

use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use inst_fsx::{AtomicFile, RelPath, dirs};

use crate::codec;
use crate::error::{IntegrityTarget, PayloadError};
use crate::format::{BlockEntry, FOOTER_LEN, FileEntry, Footer, HEADER_LEN, Header, Index};
use crate::locate;

/// A located and verified payload index.
#[derive(Clone, Debug)]
pub struct Payload {
    pub index: Index,
    /// Absolute offset of the payload start in the containing file.
    pub base: u64,
    pub footer: Footer,
}

impl Payload {
    /// Finds the payload at the logical end of `r` (typically the running
    /// executable), skipping an Authenticode certificate table if present.
    pub fn locate<R: Read + Seek>(r: &mut R) -> Result<Payload, PayloadError> {
        let file_len = r.seek(SeekFrom::End(0))?;
        let end = locate::logical_end(r, file_len)?;
        // Payloads are aligned so the footer normally ends exactly at `end`;
        // tolerate zero padding added by third-party signing tools.
        for pad in 0..crate::format::ALIGNMENT.min(end) {
            match Payload::read_ending_at(r, end - pad) {
                Err(PayloadError::NotFound) => continue,
                other => return other,
            }
        }
        Err(PayloadError::NotFound)
    }

    /// Reads a payload whose footer ends exactly at `end`.
    pub fn read_ending_at<R: Read + Seek>(r: &mut R, end: u64) -> Result<Payload, PayloadError> {
        if end < HEADER_LEN + FOOTER_LEN {
            return Err(PayloadError::NotFound);
        }
        let mut footer_bytes = [0u8; FOOTER_LEN as usize];
        r.seek(SeekFrom::Start(end - FOOTER_LEN))?;
        r.read_exact(&mut footer_bytes)?;
        if footer_bytes[..8] != crate::format::FOOTER_MAGIC {
            return Err(PayloadError::NotFound);
        }
        let footer = Footer::decode(&footer_bytes)?;
        if footer.payload_len > end {
            return Err(PayloadError::Truncated(IntegrityTarget::Index));
        }
        let base = end - footer.payload_len;

        let mut header_bytes = [0u8; HEADER_LEN as usize];
        r.seek(SeekFrom::Start(base))?;
        r.read_exact(&mut header_bytes)?;
        Header::decode(&header_bytes)
            .map_err(|e| PayloadError::Index(crate::format::IndexError::Wire(e)))?;

        let mut index_bytes = vec![0u8; footer.index_len as usize];
        r.seek(SeekFrom::Start(base + footer.index_offset))?;
        r.read_exact(&mut index_bytes)?;
        if blake3::hash(&index_bytes).as_bytes() != &footer.index_hash {
            return Err(PayloadError::Integrity(IntegrityTarget::Index));
        }
        let index = Index::decode(&index_bytes, footer.index_offset)?;
        Ok(Payload {
            index,
            base,
            footer,
        })
    }

    /// Absolute file offset and length of block `i`.
    #[inline]
    pub fn block_range(&self, i: usize) -> (u64, u64) {
        let b = &self.index.blocks[i];
        (self.base + b.offset, b.compressed_len)
    }

    /// Positions `r` at block `i` and limits reading to its bytes.
    pub fn open_block<'r, R: Read + Seek>(
        &self,
        r: &'r mut R,
        i: usize,
    ) -> io::Result<io::Take<&'r mut R>> {
        let (offset, len) = self.block_range(i);
        r.seek(SeekFrom::Start(offset))?;
        Ok(r.take(len))
    }
}

/// Decision for one file during extraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileAction {
    Extract,
    /// Decode but discard (e.g. a component that was not selected).
    Skip,
}

/// Hooks that let the runtime journal every change for rollback.
pub trait ExtractObserver {
    /// Called before a file is decoded. `target` is where it will be written.
    fn begin_file(&mut self, _path: RelPath<'_>, _entry: &FileEntry, _target: &Path) -> io::Result<FileAction> {
        Ok(FileAction::Extract)
    }
    /// Directories that extraction created, outermost first.
    fn dirs_created(&mut self, _dirs: &[PathBuf]) {}
    /// Called after the new content was verified and right before it
    /// replaces `target`. `exists` tells whether `target` exists; the runtime
    /// uses this to move the old file to its rollback backup.
    fn before_commit(&mut self, _path: RelPath<'_>, _target: &Path, _exists: bool) -> io::Result<()> {
        Ok(())
    }
    /// Called after the file was renamed into place.
    fn committed(&mut self, _path: RelPath<'_>, _entry: &FileEntry, _target: &Path) -> io::Result<()> {
        Ok(())
    }
    /// Uncompressed bytes processed since the last call.
    fn progress(&mut self, _bytes: u64) {}
    fn cancelled(&self) -> bool {
        false
    }
}

/// An observer that accepts everything and journals nothing.
pub struct PlainExtract;
impl ExtractObserver for PlainExtract {}

#[derive(Clone, Copy, Debug, Default)]
pub struct ExtractOptions {
    /// fsync every file before renaming it. Slow for many small files.
    pub durable: bool,
}

/// Reusable buffers for extraction. Keep one per worker thread.
pub struct ExtractBuffers {
    copy: Vec<u8>,
    created_dirs: Vec<PathBuf>,
}

impl Default for ExtractBuffers {
    fn default() -> Self {
        ExtractBuffers {
            copy: vec![0u8; 256 << 10],
            created_dirs: Vec::new(),
        }
    }
}

/// Counts and hashes compressed bytes as the decoder pulls them.
struct HashingReader<R> {
    inner: R,
    hasher: blake3::Hasher,
    read: u64,
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.read += n as u64;
        Ok(n)
    }
}

/// Creates the payload's explicit (possibly empty) directories under `root`.
pub fn extract_dirs(
    payload: &Payload,
    root: &Path,
    observer: &mut dyn ExtractObserver,
    bufs: &mut ExtractBuffers,
) -> Result<(), PayloadError> {
    for span in &payload.index.dirs {
        let rel = RelPath::new(payload.index.str(*span))
            .map_err(|e| PayloadError::UnsafePath(payload.index.str(*span).to_owned(), e))?;
        bufs.created_dirs.clear();
        dirs::ensure_dir_under(root, rel, &mut bufs.created_dirs)?;
        if !bufs.created_dirs.is_empty() {
            observer.dirs_created(&bufs.created_dirs);
        }
    }
    Ok(())
}

/// Extracts every file of block `block_index` from `raw`, which must yield
/// exactly the block's compressed bytes (see [`Payload::open_block`]).
pub fn extract_block<R: Read>(
    payload: &Payload,
    block_index: usize,
    raw: R,
    root: &Path,
    observer: &mut dyn ExtractObserver,
    bufs: &mut ExtractBuffers,
    opts: ExtractOptions,
) -> Result<(), PayloadError> {
    let index = &payload.index;
    let block: &BlockEntry = &index.blocks[block_index];
    let block_id = block_index as u32;
    if !codec::decoder_available(block.codec) {
        return Err(PayloadError::Io(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("codec {} not available", block.codec.name()),
        )));
    }

    let mut hashing = HashingReader {
        inner: raw,
        hasher: blake3::Hasher::new(),
        read: 0,
    };
    {
        let buffered = BufReader::with_capacity(128 << 10, &mut hashing);
        let mut decoder = codec::decoder(block.codec, buffered)?;

        for entry in index.block_files(block) {
            if observer.cancelled() {
                return Err(PayloadError::Cancelled);
            }
            let path_str = index.file_path(entry);
            let rel = RelPath::new(path_str)
                .map_err(|e| PayloadError::UnsafePath(path_str.to_owned(), e))?;
            let target = rel.to_path_under(root);
            match observer.begin_file(rel, entry, &target)? {
                FileAction::Skip => {
                    let copied =
                        io::copy(&mut (&mut decoder).take(entry.size), &mut io::sink())?;
                    if copied != entry.size {
                        return Err(PayloadError::Truncated(IntegrityTarget::File(path_str.to_owned())));
                    }
                    observer.progress(entry.size);
                }
                FileAction::Extract => {
                    extract_file(&mut decoder, rel, entry, &target, root, observer, bufs, opts)?;
                }
            }
        }

        // The decoded stream must end exactly after the last file.
        let mut probe = [0u8; 1];
        match decoder.read(&mut probe) {
            Ok(0) => {}
            Ok(_) => return Err(PayloadError::Integrity(IntegrityTarget::Block(block_id))),
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                return Err(PayloadError::Integrity(IntegrityTarget::Block(block_id)));
            }
            Err(e) => return Err(e.into()),
        }
    }
    // Drain anything the decoder did not need (normally nothing) so the
    // block hash covers every byte.
    io::copy(&mut hashing, &mut io::sink())?;
    if hashing.read != block.compressed_len {
        return Err(PayloadError::Truncated(IntegrityTarget::Block(block_id)));
    }
    if hashing.hasher.finalize().as_bytes() != &block.hash {
        return Err(PayloadError::Integrity(IntegrityTarget::Block(block_id)));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn extract_file(
    decoder: &mut dyn Read,
    rel: RelPath<'_>,
    entry: &FileEntry,
    target: &Path,
    root: &Path,
    observer: &mut dyn ExtractObserver,
    bufs: &mut ExtractBuffers,
    opts: ExtractOptions,
) -> Result<(), PayloadError> {
    if let Some(parent) = rel.parent() {
        bufs.created_dirs.clear();
        dirs::ensure_dir_under(root, parent, &mut bufs.created_dirs)?;
        if !bufs.created_dirs.is_empty() {
            observer.dirs_created(&bufs.created_dirs);
        }
    }
    let exists = dirs::check_not_symlink(target)?;
    if exists && target.is_dir() {
        return Err(PayloadError::Io(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} is a directory", target.display()),
        )));
    }

    let mut out = AtomicFile::create(target)?;
    out.file().set_len(entry.size).ok(); // pre-allocation hint only
    let mut hasher = blake3::Hasher::new();
    let mut remaining = entry.size;
    let corrupt = |e: io::Error| -> PayloadError {
        if e.kind() == io::ErrorKind::InvalidData || e.kind() == io::ErrorKind::UnexpectedEof {
            PayloadError::Integrity(IntegrityTarget::File(rel.as_str().to_owned()))
        } else {
            PayloadError::Io(e)
        }
    };
    while remaining > 0 {
        if observer.cancelled() {
            return Err(PayloadError::Cancelled);
        }
        let want = bufs.copy.len().min(remaining as usize);
        let n = match decoder.read(&mut bufs.copy[..want]) {
            Ok(0) => {
                return Err(PayloadError::Truncated(IntegrityTarget::File(rel.as_str().to_owned())));
            }
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(corrupt(e)),
        };
        hasher.update(&bufs.copy[..n]);
        out.write_all(&bufs.copy[..n])?;
        remaining -= n as u64;
        observer.progress(n as u64);
    }
    if hasher.finalize().as_bytes() != &entry.hash {
        return Err(PayloadError::Integrity(IntegrityTarget::File(rel.as_str().to_owned())));
    }
    set_mode(out.temp_path(), entry.flags.executable())?;
    observer.before_commit(rel, target, exists)?;
    out.commit(opts.durable)?;
    observer.committed(rel, entry, target)?;
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, executable: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if executable { 0o755 } else { 0o644 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _executable: bool) -> io::Result<()> {
    Ok(())
}

/// Verifies the compressed hash of every block without decompressing.
/// `progress` receives compressed bytes read.
pub fn verify_blocks<R: Read + Seek>(
    payload: &Payload,
    r: &mut R,
    mut progress: impl FnMut(u64),
) -> Result<(), PayloadError> {
    let mut buf = vec![0u8; 1 << 20];
    for i in 0..payload.index.blocks.len() {
        let mut block = payload.open_block(r, i)?;
        let mut hasher = blake3::Hasher::new();
        let mut total = 0u64;
        loop {
            let n = match block.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            };
            hasher.update(&buf[..n]);
            total += n as u64;
            progress(n as u64);
        }
        let expected = &payload.index.blocks[i];
        if total != expected.compressed_len {
            return Err(PayloadError::Truncated(IntegrityTarget::Block(i as u32)));
        }
        if hasher.finalize().as_bytes() != &expected.hash {
            return Err(PayloadError::Integrity(IntegrityTarget::Block(i as u32)));
        }
    }
    Ok(())
}
