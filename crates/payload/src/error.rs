use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::format::IndexError;

/// What failed an integrity check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntegrityTarget {
    Index,
    Block(u32),
    File(String),
}

#[derive(Debug)]
pub enum PayloadError {
    Io(io::Error),
    /// Reading a source file failed while building.
    Source(PathBuf, io::Error),
    /// A source file changed size between scanning and compression.
    SourceChanged(PathBuf),
    /// The executable carries no payload.
    NotFound,
    Index(IndexError),
    /// Hash verification failed: the payload is corrupt or was tampered with.
    Integrity(IntegrityTarget),
    /// Decompressed data ended early or ran long.
    Truncated(IntegrityTarget),
    UnsafePath(String, inst_fsx::PathError),
    DuplicatePath(String),
    Plan(&'static str),
    Cancelled,
}

impl PayloadError {
    #[cfg(feature = "write")]
    pub(crate) fn source(path: &std::path::Path, e: io::Error) -> Self {
        PayloadError::Source(path.to_path_buf(), e)
    }

    /// Whether the error means the setup file itself is damaged, as opposed
    /// to an environmental problem (disk full, permissions).
    pub fn is_corruption(&self) -> bool {
        matches!(
            self,
            PayloadError::Index(_)
                | PayloadError::Integrity(_)
                | PayloadError::Truncated(_)
                | PayloadError::NotFound
        )
    }
}

impl From<io::Error> for PayloadError {
    fn from(e: io::Error) -> Self {
        PayloadError::Io(e)
    }
}

impl From<IndexError> for PayloadError {
    fn from(e: IndexError) -> Self {
        PayloadError::Index(e)
    }
}

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PayloadError::Io(e) => write!(f, "I/O error: {e}"),
            PayloadError::Source(p, e) => write!(f, "cannot read {}: {e}", p.display()),
            PayloadError::SourceChanged(p) => {
                write!(
                    f,
                    "{} changed while the installer was being built",
                    p.display()
                )
            }
            PayloadError::NotFound => f.write_str("no payload found"),
            PayloadError::Index(e) => write!(f, "{e}"),
            PayloadError::Integrity(t) => match t {
                IntegrityTarget::Index => f.write_str("payload index checksum mismatch"),
                IntegrityTarget::Block(i) => write!(f, "checksum mismatch in block {i}"),
                IntegrityTarget::File(p) => write!(f, "checksum mismatch for {p}"),
            },
            PayloadError::Truncated(t) => match t {
                IntegrityTarget::Index => f.write_str("payload index truncated"),
                IntegrityTarget::Block(i) => write!(f, "block {i} is truncated"),
                IntegrityTarget::File(p) => write!(f, "data for {p} is truncated"),
            },
            PayloadError::UnsafePath(p, e) => write!(f, "unsafe path {p:?}: {e}"),
            PayloadError::DuplicatePath(p) => {
                write!(
                    f,
                    "duplicate path {p:?} (paths are compared case-insensitively)"
                )
            }
            PayloadError::Plan(what) => write!(f, "invalid payload plan: {what}"),
            PayloadError::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for PayloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PayloadError::Io(e) | PayloadError::Source(_, e) => Some(e),
            PayloadError::Index(e) => Some(e),
            _ => None,
        }
    }
}
