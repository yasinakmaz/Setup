//! Resumable, verified downloads and a content-addressed cache.
//!
//! Used by generated installers (prerequisite downloads) and by the Studio
//! (catalog packages, AppImage runtime). Guarantees:
//!
//! * interrupted downloads resume via HTTP `Range` + `If-Range`;
//! * a partial file is bound to the expected content hash and size;
//! * retries use exponential backoff; sources are tried in priority order;
//! * nothing is returned before size and hash verification succeed;
//! * completion is an atomic rename.

pub mod cache;
pub mod download;
pub mod hash;
pub mod transport;

pub use cache::{Cache, CacheEntry, ItemInfo};
pub use download::{DownloadError, DownloadObserver, DownloadRequest, Downloader, Progress, RetryPolicy};
pub use hash::ContentHash;
pub use transport::{Transport, TransportConfig};
