//! Central resource coordinator.
//!
//! Hashing, compression, scanning and cargo must not each spawn "one thread
//! per core": they share one budget derived from logical CPUs and available
//! memory, and one core stays free for the Studio UI.

/// Machine resources relevant to build scheduling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resources {
    pub logical_cpus: usize,
    /// Available (not total) physical memory in bytes, if known.
    pub available_memory: Option<u64>,
}

impl Resources {
    pub fn detect() -> Resources {
        Resources {
            logical_cpus: std::thread::available_parallelism().map_or(1, |n| n.get()),
            available_memory: available_memory(),
        }
    }

    /// Worker threads for CPU-bound work, keeping one core for the UI.
    pub fn cpu_workers(&self, reserve_ui: bool) -> usize {
        let cpus = if reserve_ui {
            self.logical_cpus.saturating_sub(1)
        } else {
            self.logical_cpus
        };
        cpus.max(1)
    }

    /// Parallel block encoders that fit in memory: each encoder needs
    /// `per_encoder` bytes; at most half of available memory is used.
    pub fn compression_workers(&self, blocks: usize, per_encoder: u64, reserve_ui: bool) -> usize {
        let by_cpu = self.cpu_workers(reserve_ui);
        let by_mem = match self.available_memory {
            Some(mem) if per_encoder > 0 => ((mem / 2) / per_encoder).max(1) as usize,
            _ => by_cpu,
        };
        by_cpu.min(by_mem).min(blocks.max(1))
    }
}

#[cfg(target_os = "linux")]
fn available_memory() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("MemAvailable:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        .map(|kb| kb * 1024)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn available_memory() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: zero-initialized struct with its size set, as the API requires.
    unsafe {
        let mut status: MEMORYSTATUSEX = std::mem::zeroed();
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        (GlobalMemoryStatusEx(&mut status) != 0).then_some(status.ullAvailPhys)
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
fn available_memory() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_are_bounded() {
        let r = Resources {
            logical_cpus: 16,
            available_memory: Some(4 << 30),
        };
        assert_eq!(r.cpu_workers(true), 15);
        // 700 MiB per xz-9 encoder: only 2 fit in half of 4 GiB.
        assert_eq!(r.compression_workers(100, 700 << 20, true), 2);
        assert_eq!(r.compression_workers(3, 1 << 20, true), 3);
        let tiny = Resources {
            logical_cpus: 1,
            available_memory: None,
        };
        assert_eq!(tiny.cpu_workers(true), 1);
    }
}
