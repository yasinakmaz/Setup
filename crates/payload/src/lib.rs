//! Installer payload: format, creation and streaming extraction.
//!
//! See [`format`] for the binary layout and integrity model.

pub mod codec;
mod error;
pub mod format;
pub mod locate;
#[cfg(feature = "read")]
pub mod read;
#[cfg(feature = "write")]
pub mod write;

pub use codec::CodecParams;
pub use error::{IntegrityTarget, PayloadError};
pub use format::{Codec, FileEntry, Filter, Index};
#[cfg(feature = "read")]
pub use read::{ExtractBuffers, ExtractObserver, ExtractOptions, FileAction, Payload};
#[cfg(feature = "write")]
pub use write::{BlockPlan, InputFile, WriteOptions, WriteSummary};
