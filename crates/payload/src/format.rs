//! On-disk payload format, version 1.
//!
//! ```text
//! ┌──────────────────────────────┐ ← payload start (appended to the runtime executable)
//! │ Header (32 bytes)            │   magic, version, flags
//! ├──────────────────────────────┤
//! │ Block 0 (compressed)         │   solid or grouped-solid; each block is
//! │ Block 1                      │   independently decodable, so blocks can
//! │ …                            │   be extracted in parallel
//! ├──────────────────────────────┤
//! │ Index (wire-encoded)         │   block table, directory table, file
//! │                              │   table, BLAKE3 hashes, metadata
//! ├──────────────────────────────┤
//! │ Signature metadata (opt.)    │   detached signature over the index hash
//! ├──────────────────────────────┤
//! │ Footer (72 bytes)            │   magic, lengths, offsets, index hash
//! └──────────────────────────────┘ ← logical end (before an Authenticode
//!                                     certificate table, if any)
//! ```
//!
//! Integrity model:
//! * the footer carries the BLAKE3 hash of the index; the index is rejected
//!   if it does not match;
//! * each block carries the BLAKE3 hash of its compressed bytes;
//! * each file carries the BLAKE3 hash of its content, checked before the
//!   file is committed (renamed into place). Corrupt data is never committed.

use inst_fsx::relpath;
use inst_wire::{Reader, WireError, Writer};

pub const HEADER_MAGIC: [u8; 8] = *b"INSTPL\x00\x01";
pub const FOOTER_MAGIC: [u8; 8] = *b"INSTPEND";
pub const FORMAT_VERSION: u16 = 1;
pub const HEADER_LEN: u64 = 32;
pub const FOOTER_LEN: u64 = 72;
/// Payloads are padded so the executable ends on this alignment, which keeps
/// the footer at a deterministic position after Authenticode signing.
pub const ALIGNMENT: u64 = 8;

/// Hard limits applied when decoding untrusted indexes.
pub const MAX_INDEX_LEN: u64 = 256 << 20;
pub const MAX_FILES: usize = 4_000_000;
pub const MAX_BLOCKS: usize = 1_000_000;

pub type Hash = [u8; 32];

/// Compression codec of a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Codec {
    /// Stored without compression.
    None = 0,
    /// Zstandard.
    Zstd = 1,
    /// XZ container with LZMA2 (optionally preceded by a BCJ filter).
    Xz = 2,
}

impl Codec {
    pub fn from_u8(v: u8) -> Result<Codec, WireError> {
        Ok(match v {
            0 => Codec::None,
            1 => Codec::Zstd,
            2 => Codec::Xz,
            tag => {
                return Err(WireError::InvalidTag {
                    what: "codec",
                    tag: u64::from(tag),
                });
            }
        })
    }

    pub const fn name(self) -> &'static str {
        match self {
            Codec::None => "none",
            Codec::Zstd => "zstd",
            Codec::Xz => "xz",
        }
    }
}

/// Executable preprocessing filter applied before compression.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Filter {
    None = 0,
    /// x86/x86-64 branch-call-jump converter.
    X86 = 1,
    /// AArch64 branch converter.
    Arm64 = 2,
}

impl Filter {
    pub fn from_u8(v: u8) -> Result<Filter, WireError> {
        Ok(match v {
            0 => Filter::None,
            1 => Filter::X86,
            2 => Filter::Arm64,
            tag => {
                return Err(WireError::InvalidTag {
                    what: "filter",
                    tag: u64::from(tag),
                });
            }
        })
    }
}

/// Per-file flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FileFlags(pub u8);

impl FileFlags {
    pub const EXECUTABLE: u8 = 1;

    #[inline]
    pub const fn executable(self) -> bool {
        self.0 & Self::EXECUTABLE != 0
    }
}

/// Byte range inside [`Index::strings`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: u32,
    pub len: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockEntry {
    pub codec: Codec,
    pub filter: Filter,
    /// Offset of the compressed bytes relative to the payload start.
    pub offset: u64,
    pub compressed_len: u64,
    pub uncompressed_len: u64,
    /// BLAKE3 of the compressed bytes.
    pub hash: Hash,
    /// Files of this block are `files[first_file .. first_file + file_count]`,
    /// stored back to back in the decompressed stream.
    pub first_file: u32,
    pub file_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub path: Span,
    pub size: u64,
    pub block: u32,
    pub flags: FileFlags,
    /// Optional component the file belongs to (0 = core application).
    pub component: u16,
    /// BLAKE3 of the file content.
    pub hash: Hash,
}

/// Parsed payload index. Paths live in one string buffer (`strings`) to
/// avoid one allocation per file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    pub blocks: Vec<BlockEntry>,
    /// Directories that must exist even if empty.
    pub dirs: Vec<Span>,
    pub files: Vec<FileEntry>,
    pub strings: String,
    /// Sum of all file sizes.
    pub total_size: u64,
}

impl Index {
    #[inline]
    pub fn str(&self, span: Span) -> &str {
        let start = span.start as usize;
        &self.strings[start..start + span.len as usize]
    }

    #[inline]
    pub fn file_path(&self, file: &FileEntry) -> &str {
        self.str(file.path)
    }

    pub fn block_files(&self, block: &BlockEntry) -> &[FileEntry] {
        let start = block.first_file as usize;
        &self.files[start..start + block.file_count as usize]
    }

    /// Total compressed size of all blocks.
    pub fn compressed_size(&self) -> u64 {
        self.blocks.iter().map(|b| b.compressed_len).sum()
    }

    pub(crate) fn push_str(&mut self, s: &str) -> Span {
        let start = self.strings.len() as u32;
        self.strings.push_str(s);
        Span {
            start,
            len: s.len() as u32,
        }
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        let mut w = Writer::new(out);
        w.u16(FORMAT_VERSION);
        w.varint(self.total_size);
        w.varint(self.blocks.len() as u64);
        for b in &self.blocks {
            w.u8(b.codec as u8)
                .u8(b.filter as u8)
                .varint(b.offset)
                .varint(b.compressed_len)
                .varint(b.uncompressed_len)
                .hash(&b.hash)
                .varint(u64::from(b.first_file))
                .varint(u64::from(b.file_count));
        }
        w.varint(self.dirs.len() as u64);
        for d in &self.dirs {
            w.str(self.str(*d));
        }
        w.varint(self.files.len() as u64);
        for f in &self.files {
            w.str(self.str(f.path))
                .varint(f.size)
                .varint(u64::from(f.block))
                .u8(f.flags.0)
                .u16(f.component)
                .hash(&f.hash);
        }
    }

    /// Decodes and fully validates an index read from an untrusted payload.
    /// `payload_data_end` is the offset (relative to the payload start) where
    /// block data must end.
    pub fn decode(bytes: &[u8], payload_data_end: u64) -> Result<Index, IndexError> {
        let mut r = Reader::new(bytes);
        let version = r.u16()?;
        if version != FORMAT_VERSION {
            return Err(IndexError::UnsupportedVersion(version));
        }
        let total_size = r.varint()?;
        // 1 + 1 + 1 + 1 + 1 + 32 + 1 + 1 bytes minimum per block.
        let block_count = r.count(39)?;
        if block_count > MAX_BLOCKS {
            return Err(IndexError::Invalid("too many blocks"));
        }
        let mut index = Index {
            blocks: Vec::with_capacity(block_count),
            total_size,
            ..Index::default()
        };
        for _ in 0..block_count {
            let codec = Codec::from_u8(r.u8()?)?;
            let filter = Filter::from_u8(r.u8()?)?;
            index.blocks.push(BlockEntry {
                codec,
                filter,
                offset: r.varint()?,
                compressed_len: r.varint()?,
                uncompressed_len: r.varint()?,
                hash: r.hash()?,
                first_file: u32::try_from(r.varint()?).map_err(|_| IndexError::Invalid("file range"))?,
                file_count: u32::try_from(r.varint()?).map_err(|_| IndexError::Invalid("file range"))?,
            });
        }
        let dir_count = r.count(2)?;
        index.dirs.reserve_exact(dir_count);
        for _ in 0..dir_count {
            let path = r.str()?;
            relpath::validate(path).map_err(IndexError::Path)?;
            let span = index.push_str(path);
            index.dirs.push(span);
        }
        // path (>=2) + size + block + flags + component(2) + hash(32)
        let file_count = r.count(38)?;
        if file_count > MAX_FILES {
            return Err(IndexError::Invalid("too many files"));
        }
        index.files.reserve_exact(file_count);
        for _ in 0..file_count {
            let path = r.str()?;
            relpath::validate(path).map_err(IndexError::Path)?;
            let span = index.push_str(path);
            index.files.push(FileEntry {
                path: span,
                size: r.varint()?,
                block: u32::try_from(r.varint()?).map_err(|_| IndexError::Invalid("block ref"))?,
                flags: FileFlags(r.u8()?),
                component: r.u16()?,
                hash: r.hash()?,
            });
        }
        r.finish()?;
        index.validate(payload_data_end)?;
        Ok(index)
    }

    /// Structural validation: block bounds, file ranges, sizes, uniqueness.
    pub fn validate(&self, payload_data_end: u64) -> Result<(), IndexError> {
        let mut next_file = 0u64;
        let mut prev_end = HEADER_LEN;
        let mut total = 0u64;
        for (i, b) in self.blocks.iter().enumerate() {
            if b.offset < prev_end {
                return Err(IndexError::Invalid("blocks overlap or are out of order"));
            }
            let end = b
                .offset
                .checked_add(b.compressed_len)
                .ok_or(IndexError::Invalid("block length overflow"))?;
            if end > payload_data_end {
                return Err(IndexError::Invalid("block extends past payload"));
            }
            prev_end = end;
            if u64::from(b.first_file) != next_file {
                return Err(IndexError::Invalid("block file ranges are not contiguous"));
            }
            next_file += u64::from(b.file_count);
            if next_file > self.files.len() as u64 {
                return Err(IndexError::Invalid("block file range out of bounds"));
            }
            let mut sum = 0u64;
            for f in self.block_files(b) {
                if f.block as usize != i {
                    return Err(IndexError::Invalid("file block reference mismatch"));
                }
                sum = sum
                    .checked_add(f.size)
                    .ok_or(IndexError::Invalid("size overflow"))?;
            }
            if sum != b.uncompressed_len {
                return Err(IndexError::Invalid("block size does not match its files"));
            }
            if b.codec == Codec::None && b.compressed_len != b.uncompressed_len {
                return Err(IndexError::Invalid("stored block length mismatch"));
            }
            total = total
                .checked_add(sum)
                .ok_or(IndexError::Invalid("size overflow"))?;
        }
        if next_file != self.files.len() as u64 {
            return Err(IndexError::Invalid("files not covered by blocks"));
        }
        if total != self.total_size {
            return Err(IndexError::Invalid("total size mismatch"));
        }
        let mut seen = std::collections::HashSet::with_capacity(self.files.len());
        for f in &self.files {
            if !seen.insert(self.file_path(f)) {
                return Err(IndexError::Invalid("duplicate file path"));
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum IndexError {
    Wire(WireError),
    Path(relpath::PathError),
    UnsupportedVersion(u16),
    Invalid(&'static str),
}

impl From<WireError> for IndexError {
    fn from(e: WireError) -> Self {
        IndexError::Wire(e)
    }
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexError::Wire(e) => write!(f, "malformed index: {e}"),
            IndexError::Path(e) => write!(f, "unsafe path in index: {e}"),
            IndexError::UnsupportedVersion(v) => write!(f, "unsupported payload version {v}"),
            IndexError::Invalid(what) => write!(f, "invalid index: {what}"),
        }
    }
}

impl std::error::Error for IndexError {}

/// Fixed-size header at the start of the payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: u16,
    pub flags: u16,
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER_LEN as usize] {
        let mut out = [0u8; HEADER_LEN as usize];
        out[..8].copy_from_slice(&HEADER_MAGIC);
        out[8..10].copy_from_slice(&self.version.to_le_bytes());
        out[10..12].copy_from_slice(&self.flags.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8; HEADER_LEN as usize]) -> Result<Header, WireError> {
        if bytes[..8] != HEADER_MAGIC {
            return Err(WireError::BadMagic);
        }
        let version = u16::from_le_bytes([bytes[8], bytes[9]]);
        if version != FORMAT_VERSION {
            return Err(WireError::BadMagic);
        }
        Ok(Header {
            version,
            flags: u16::from_le_bytes([bytes[10], bytes[11]]),
        })
    }
}

/// Fixed-size footer at the logical end of the payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Footer {
    pub version: u16,
    pub flags: u16,
    /// Length of the signature section immediately before the footer.
    pub signature_len: u32,
    /// Total payload length including header and footer.
    pub payload_len: u64,
    /// Index offset relative to the payload start.
    pub index_offset: u64,
    pub index_len: u64,
    pub index_hash: Hash,
}

impl Footer {
    pub fn encode(&self) -> [u8; FOOTER_LEN as usize] {
        let mut out = [0u8; FOOTER_LEN as usize];
        out[..8].copy_from_slice(&FOOTER_MAGIC);
        out[8..10].copy_from_slice(&self.version.to_le_bytes());
        out[10..12].copy_from_slice(&self.flags.to_le_bytes());
        out[12..16].copy_from_slice(&self.signature_len.to_le_bytes());
        out[16..24].copy_from_slice(&self.payload_len.to_le_bytes());
        out[24..32].copy_from_slice(&self.index_offset.to_le_bytes());
        out[32..40].copy_from_slice(&self.index_len.to_le_bytes());
        out[40..72].copy_from_slice(&self.index_hash);
        out
    }

    pub fn decode(bytes: &[u8; FOOTER_LEN as usize]) -> Result<Footer, IndexError> {
        if bytes[..8] != FOOTER_MAGIC {
            return Err(IndexError::Wire(WireError::BadMagic));
        }
        let mut r = Reader::new(&bytes[8..]);
        let footer = Footer {
            version: r.u16()?,
            flags: r.u16()?,
            signature_len: r.u32()?,
            payload_len: r.u64()?,
            index_offset: r.u64()?,
            index_len: r.u64()?,
            index_hash: r.hash()?,
        };
        if footer.version != FORMAT_VERSION {
            return Err(IndexError::UnsupportedVersion(footer.version));
        }
        let index_end = footer
            .index_offset
            .checked_add(footer.index_len)
            .and_then(|e| e.checked_add(u64::from(footer.signature_len)))
            .and_then(|e| e.checked_add(FOOTER_LEN))
            .ok_or(IndexError::Invalid("footer overflow"))?;
        if footer.index_offset < HEADER_LEN
            || index_end != footer.payload_len
            || footer.index_len > MAX_INDEX_LEN
        {
            return Err(IndexError::Invalid("footer offsets"));
        }
        Ok(footer)
    }
}
