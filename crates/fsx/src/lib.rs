//! Security-sensitive file-system primitives used by the Studio and by
//! generated installers.
//!
//! * [`relpath`] — validation of untrusted relative paths (no traversal).
//! * [`atomic`] — write-to-temp + rename with exclusive creation.
//! * [`dirs`] — directory creation that refuses symlinked components.
//! * [`space`] — free disk space.

pub mod atomic;
pub mod dirs;
pub mod relpath;
pub mod space;

pub use atomic::{AtomicFile, write_atomic};
pub use relpath::{PathError, RelPath};
pub use space::available_space;

/// Formats a byte count using binary units (`1.5 MiB`).
pub fn format_bytes(bytes: u64) -> ByteSize {
    ByteSize(bytes)
}

/// Display adapter returned by [`format_bytes`].
#[derive(Clone, Copy, Debug)]
pub struct ByteSize(pub u64);

impl std::fmt::Display for ByteSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
        let mut value = self.0 as f64;
        let mut unit = 0;
        while value >= 1024.0 && unit + 1 < UNITS.len() {
            value /= 1024.0;
            unit += 1;
        }
        if unit == 0 {
            write!(f, "{} B", self.0)
        } else {
            write!(f, "{value:.2} {}", UNITS[unit])
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_bytes() {
        assert_eq!(super::format_bytes(512).to_string(), "512 B");
        assert_eq!(super::format_bytes(174_281_632).to_string(), "166.21 MiB");
    }
}
