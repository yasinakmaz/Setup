//! Free disk space queries.

use std::io;
use std::path::Path;

/// Bytes available to the current user on the volume containing `path`.
///
/// `path` does not need to exist; the nearest existing ancestor is queried.
pub fn available_space(path: &Path) -> io::Result<u64> {
    let mut probe = path;
    loop {
        if probe.exists() {
            return sys::available_space(probe);
        }
        probe = probe
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no existing ancestor"))?;
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
mod sys {
    use std::ffi::CString;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    pub fn available_space(path: &Path) -> io::Result<u64> {
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: `c_path` is a valid NUL-terminated string and `stat` points
        // to writable memory of the correct type; statvfs fully initializes it
        // on success.
        let rc = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: statvfs returned 0, so the struct is initialized.
        let stat = unsafe { stat.assume_init() };
        #[allow(clippy::unnecessary_cast)]
        Ok((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod sys {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    pub fn available_space(path: &Path) -> io::Result<u64> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut available = 0u64;
        // SAFETY: `wide` is NUL-terminated; the out-pointer is valid; the
        // other out-parameters are optional and passed as null.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut available,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(available)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reports_some_space_for_temp_dir() {
        let tmp = std::env::temp_dir();
        let space = super::available_space(&tmp.join("does/not/exist")).expect("space");
        assert!(space > 0);
    }
}
