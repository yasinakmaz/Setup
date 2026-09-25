//! Payload creation (Studio side).
//!
//! Blocks are compressed in parallel into temporary files, then concatenated
//! in order. Every source file is streamed once: it is hashed and compressed
//! in the same pass through a reused buffer. The whole payload is never held
//! in memory.

use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::codec::{CodecParams, encoder};
use crate::error::PayloadError;
use crate::format::{
    ALIGNMENT, BlockEntry, FORMAT_VERSION, FileEntry, FileFlags, Footer, HEADER_LEN, Hash, Header,
    Index,
};

/// A file to put into the payload.
#[derive(Clone, Debug)]
pub struct InputFile {
    /// Destination path, `/`-separated, validated with `inst_fsx::relpath`.
    pub rel_path: String,
    /// Where to read the content from.
    pub source: PathBuf,
    /// Expected size (from the scan). A mismatch at read time aborts the build.
    pub size: u64,
    pub executable: bool,
    pub component: u16,
}

/// One block of the payload: the files it contains (indices into the input
/// list, in storage order) and how to compress them.
#[derive(Clone, Debug)]
pub struct BlockPlan {
    pub params: CodecParams,
    pub files: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct WriteOptions {
    /// Worker threads compressing blocks concurrently.
    pub threads: usize,
    /// Directory for per-block temporary files (same volume as the output
    /// is not required).
    pub temp_dir: PathBuf,
    /// Size of the reused I/O buffer per worker.
    pub buffer_size: usize,
}

impl Default for WriteOptions {
    fn default() -> Self {
        WriteOptions {
            threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
            temp_dir: std::env::temp_dir(),
            buffer_size: 1 << 20,
        }
    }
}

/// Result of a payload build.
#[derive(Clone, Debug)]
pub struct WriteSummary {
    pub index: Index,
    /// Bytes written including header, index, footer and alignment padding.
    pub payload_len: u64,
    pub index_len: u64,
    pub padding: u64,
    pub elapsed: Duration,
    pub block_stats: Vec<BlockStats>,
}

#[derive(Clone, Debug)]
pub struct BlockStats {
    pub compressed_len: u64,
    pub uncompressed_len: u64,
    pub elapsed: Duration,
}

/// Cooperative progress/cancellation hooks.
pub trait WriteObserver: Sync {
    fn progress(&self, _uncompressed_bytes: u64) {}
    fn cancelled(&self) -> bool {
        false
    }
}

/// Observer that does nothing.
pub struct NoObserver;
impl WriteObserver for NoObserver {}

struct CompressedBlock {
    temp: tempfile::NamedTempFile,
    compressed_len: u64,
    hash: Hash,
    file_hashes: Vec<Hash>,
    stats: BlockStats,
}

/// Counts and hashes everything written through it.
struct HashingWriter<W> {
    inner: W,
    hasher: blake3::Hasher,
    written: u64,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.written += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn compress_block(
    inputs: &[InputFile],
    plan: &BlockPlan,
    opts: &WriteOptions,
    buf: &mut Vec<u8>,
    observer: &dyn WriteObserver,
) -> Result<CompressedBlock, PayloadError> {
    let started = Instant::now();
    let temp = tempfile::Builder::new()
        .prefix(".block-")
        .tempfile_in(&opts.temp_dir)?;
    let mut sink = HashingWriter {
        inner: BufWriter::with_capacity(opts.buffer_size, temp.as_file()),
        hasher: blake3::Hasher::new(),
        written: 0,
    };
    let mut file_hashes = Vec::with_capacity(plan.files.len());
    let mut uncompressed = 0u64;
    {
        let mut enc = encoder(&plan.params, &mut sink)?;
        buf.resize(opts.buffer_size.max(4096), 0);
        for &i in &plan.files {
            let input = &inputs[i];
            let mut file = File::open(&input.source)
                .map_err(|e| PayloadError::source(&input.source, e))?;
            let mut hasher = blake3::Hasher::new();
            let mut read_total = 0u64;
            loop {
                if observer.cancelled() {
                    return Err(PayloadError::Cancelled);
                }
                let n = match file.read(buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(PayloadError::source(&input.source, e)),
                };
                hasher.update(&buf[..n]);
                enc.write_all(&buf[..n])?;
                read_total += n as u64;
                observer.progress(n as u64);
            }
            if read_total != input.size {
                return Err(PayloadError::SourceChanged(input.source.clone()));
            }
            uncompressed += read_total;
            file_hashes.push(*hasher.finalize().as_bytes());
        }
        enc.finish()?;
    }
    sink.flush()?;
    let compressed_len = sink.written;
    let hash = *sink.hasher.finalize().as_bytes();
    drop(sink);
    Ok(CompressedBlock {
        temp,
        compressed_len,
        hash,
        file_hashes,
        stats: BlockStats {
            compressed_len,
            uncompressed_len: uncompressed,
            elapsed: started.elapsed(),
        },
    })
}

/// Validates the plan: every input used exactly once, paths safe and unique
/// (case-insensitively, because Windows file systems are).
fn validate_plan(inputs: &[InputFile], blocks: &[BlockPlan], dirs: &[String]) -> Result<(), PayloadError> {
    let mut used = vec![false; inputs.len()];
    for plan in blocks {
        for &i in &plan.files {
            let slot = used
                .get_mut(i)
                .ok_or(PayloadError::Plan("block references unknown input"))?;
            if std::mem::replace(slot, true) {
                return Err(PayloadError::Plan("input used by more than one block"));
            }
        }
    }
    if used.iter().any(|u| !u) {
        return Err(PayloadError::Plan("input not assigned to any block"));
    }
    let mut seen = std::collections::HashSet::with_capacity(inputs.len());
    for input in inputs {
        inst_fsx::relpath::validate(&input.rel_path)
            .map_err(|e| PayloadError::UnsafePath(input.rel_path.clone(), e))?;
        if !seen.insert(input.rel_path.to_lowercase()) {
            return Err(PayloadError::DuplicatePath(input.rel_path.clone()));
        }
    }
    for dir in dirs {
        inst_fsx::relpath::validate(dir).map_err(|e| PayloadError::UnsafePath(dir.clone(), e))?;
    }
    Ok(())
}

/// Writes a complete payload to `out`, which must be positioned where the
/// payload starts. `prefix_len` is the number of bytes already in the output
/// file before the payload (the runtime executable); it is used to pad the
/// final file to [`ALIGNMENT`].
pub fn write_payload<W: Write>(
    out: &mut W,
    prefix_len: u64,
    inputs: &[InputFile],
    blocks: &[BlockPlan],
    empty_dirs: &[String],
    opts: &WriteOptions,
    observer: &dyn WriteObserver,
) -> Result<WriteSummary, PayloadError> {
    let started = Instant::now();
    validate_plan(inputs, blocks, empty_dirs)?;

    // Compress blocks in parallel. Workers pull the next block index from a
    // shared counter so large and small blocks balance across threads.
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let results: Vec<Mutex<Option<Result<CompressedBlock, PayloadError>>>> =
        blocks.iter().map(|_| Mutex::new(None)).collect();
    let threads = opts.threads.clamp(1, blocks.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                let mut buf = Vec::new();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= blocks.len() || failed.load(Ordering::Relaxed) {
                        break;
                    }
                    let result = compress_block(inputs, &blocks[i], opts, &mut buf, observer);
                    if result.is_err() {
                        failed.store(true, Ordering::Relaxed);
                    }
                    *results[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
                }
            });
        }
    });

    let mut index = Index::default();
    // Directory strings first: the same order the decoder uses, so a decoded
    // index compares equal to the one built here.
    for dir in empty_dirs {
        let span = index.push_str(dir);
        index.dirs.push(span);
    }
    let mut compressed = Vec::with_capacity(blocks.len());
    for slot in results {
        match slot.into_inner().unwrap_or_else(|e| e.into_inner()) {
            Some(Ok(block)) => compressed.push(block),
            Some(Err(e)) => return Err(e),
            None => return Err(PayloadError::Cancelled),
        }
    }

    // Header.
    let header = Header {
        version: FORMAT_VERSION,
        flags: 0,
    };
    out.write_all(&header.encode())?;
    let mut offset = HEADER_LEN;

    // Blocks, concatenated in plan order.
    let mut block_stats = Vec::with_capacity(compressed.len());
    let mut copy_buf = vec![0u8; opts.buffer_size.max(4096)];
    for (bi, (plan, mut block)) in blocks.iter().zip(compressed).enumerate() {
        let first_file = index.files.len() as u32;
        let mut uncompressed = 0u64;
        for (&input_idx, hash) in plan.files.iter().zip(&block.file_hashes) {
            let input = &inputs[input_idx];
            let path = index.push_str(&input.rel_path);
            uncompressed += input.size;
            index.files.push(FileEntry {
                path,
                size: input.size,
                block: bi as u32,
                flags: FileFlags(if input.executable {
                    FileFlags::EXECUTABLE
                } else {
                    0
                }),
                component: input.component,
                hash: *hash,
            });
        }
        index.blocks.push(BlockEntry {
            codec: plan.params.codec,
            filter: plan.params.filter,
            offset,
            compressed_len: block.compressed_len,
            uncompressed_len: uncompressed,
            hash: block.hash,
            first_file,
            file_count: plan.files.len() as u32,
        });
        index.total_size += uncompressed;
        let file = block.temp.as_file_mut();
        file.seek(SeekFrom::Start(0))?;
        let copied = copy_exact(file, out, &mut copy_buf)?;
        if copied != block.compressed_len {
            return Err(PayloadError::Plan("temporary block file changed"));
        }
        offset += block.compressed_len;
        block_stats.push(block.stats);
    }
    // Index.
    let mut index_bytes = Vec::with_capacity(64 + index.files.len() * 64 + index.strings.len());
    index.encode(&mut index_bytes);
    let index_offset = offset;
    out.write_all(&index_bytes)?;
    offset += index_bytes.len() as u64;

    // Alignment padding goes between the index and the footer, accounted
    // for as (empty) signature bytes, so the footer stays at the very end.
    let unpadded_end = prefix_len + offset + crate::format::FOOTER_LEN;
    let padding = (ALIGNMENT - unpadded_end % ALIGNMENT) % ALIGNMENT;
    out.write_all(&[0u8; ALIGNMENT as usize][..padding as usize])?;
    offset += padding;

    let payload_len = offset + crate::format::FOOTER_LEN;
    let footer = Footer {
        version: FORMAT_VERSION,
        flags: 0,
        signature_len: padding as u32,
        payload_len,
        index_offset,
        index_len: index_bytes.len() as u64,
        index_hash: *blake3::hash(&index_bytes).as_bytes(),
    };
    out.write_all(&footer.encode())?;
    out.flush()?;

    debug_assert!(index.validate(index_offset).is_ok());
    Ok(WriteSummary {
        index,
        payload_len,
        index_len: index_bytes.len() as u64,
        padding,
        elapsed: started.elapsed(),
        block_stats,
    })
}

fn copy_exact<R: Read, W: Write>(src: &mut R, dst: &mut W, buf: &mut [u8]) -> io::Result<u64> {
    let mut total = 0u64;
    loop {
        let n = match src.read(buf) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        dst.write_all(&buf[..n])?;
        total += n as u64;
    }
}

/// Appends a payload to an existing executable at `exe_path`.
pub fn append_to_executable(
    exe_path: &Path,
    inputs: &[InputFile],
    blocks: &[BlockPlan],
    empty_dirs: &[String],
    opts: &WriteOptions,
    observer: &dyn WriteObserver,
) -> Result<WriteSummary, PayloadError> {
    let mut file = std::fs::OpenOptions::new().append(true).open(exe_path)?;
    let prefix_len = file.metadata()?.len();
    let mut out = BufWriter::with_capacity(1 << 20, &mut file);
    let summary = write_payload(&mut out, prefix_len, inputs, blocks, empty_dirs, opts, observer)?;
    out.flush()?;
    drop(out);
    file.sync_all()?;
    Ok(summary)
}
