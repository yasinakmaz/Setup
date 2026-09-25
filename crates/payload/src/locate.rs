//! Finding the logical end of an executable that carries a payload.
//!
//! On Linux (ELF, AppImage) the payload footer is the last thing in the file.
//! On Windows, Authenticode signing appends a certificate table after the
//! payload; the PE security data directory tells us where it starts.

use std::io::{self, Read, Seek, SeekFrom};

const IMAGE_DIRECTORY_ENTRY_SECURITY: u64 = 4;

/// Returns the offset where the payload footer must end.
pub fn logical_end<R: Read + Seek>(r: &mut R, file_len: u64) -> io::Result<u64> {
    Ok(authenticode_table_start(r, file_len)?.unwrap_or(file_len))
}

/// If `r` is a PE image whose certificate table occupies the end of the file,
/// returns the table's start offset.
pub fn authenticode_table_start<R: Read + Seek>(r: &mut R, file_len: u64) -> io::Result<Option<u64>> {
    if file_len < 0x40 {
        return Ok(None);
    }
    let mut dos = [0u8; 0x40];
    r.seek(SeekFrom::Start(0))?;
    r.read_exact(&mut dos)?;
    if &dos[..2] != b"MZ" {
        return Ok(None);
    }
    let e_lfanew = u64::from(u32::from_le_bytes([dos[0x3c], dos[0x3d], dos[0x3e], dos[0x3f]]));
    // "PE\0\0" + COFF header (20 bytes) + optional header magic (2 bytes).
    if e_lfanew.saturating_add(26) > file_len {
        return Ok(None);
    }
    let mut pe = [0u8; 26];
    r.seek(SeekFrom::Start(e_lfanew))?;
    r.read_exact(&mut pe)?;
    if &pe[..4] != b"PE\0\0" {
        return Ok(None);
    }
    let size_of_optional = u64::from(u16::from_le_bytes([pe[20], pe[21]]));
    let opt_start = e_lfanew + 24;
    let magic = u16::from_le_bytes([pe[24], pe[25]]);
    let (count_off, dirs_off) = match magic {
        0x10b => (92u64, 96u64),  // PE32
        0x20b => (108u64, 112u64), // PE32+
        _ => return Ok(None),
    };
    let dir_entry = dirs_off + IMAGE_DIRECTORY_ENTRY_SECURITY * 8;
    if dir_entry + 8 > size_of_optional || opt_start + dir_entry + 8 > file_len {
        return Ok(None);
    }
    let mut count = [0u8; 4];
    r.seek(SeekFrom::Start(opt_start + count_off))?;
    r.read_exact(&mut count)?;
    if u64::from(u32::from_le_bytes(count)) <= IMAGE_DIRECTORY_ENTRY_SECURITY {
        return Ok(None);
    }
    let mut dir = [0u8; 8];
    r.seek(SeekFrom::Start(opt_start + dir_entry))?;
    r.read_exact(&mut dir)?;
    // For the security directory, "VirtualAddress" is a file offset.
    let offset = u64::from(u32::from_le_bytes([dir[0], dir[1], dir[2], dir[3]]));
    let size = u64::from(u32::from_le_bytes([dir[4], dir[5], dir[6], dir[7]]));
    if size == 0 || offset == 0 || offset.checked_add(size) != Some(file_len) {
        return Ok(None);
    }
    Ok(Some(offset))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Builds a minimal PE32+ header with a security directory entry.
    fn fake_pe(total_len: usize, cert_offset: u32, cert_size: u32) -> Vec<u8> {
        let mut v = vec![0u8; total_len];
        v[..2].copy_from_slice(b"MZ");
        let e_lfanew = 0x80u32;
        v[0x3c..0x40].copy_from_slice(&e_lfanew.to_le_bytes());
        let pe = e_lfanew as usize;
        v[pe..pe + 4].copy_from_slice(b"PE\0\0");
        v[pe + 20..pe + 22].copy_from_slice(&240u16.to_le_bytes()); // SizeOfOptionalHeader
        let opt = pe + 24;
        v[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        v[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes());
        let sec = opt + 112 + 4 * 8;
        v[sec..sec + 4].copy_from_slice(&cert_offset.to_le_bytes());
        v[sec + 4..sec + 8].copy_from_slice(&cert_size.to_le_bytes());
        v
    }

    #[test]
    fn finds_certificate_table() {
        let pe = fake_pe(4096, 4000, 96);
        let mut c = Cursor::new(&pe);
        assert_eq!(logical_end(&mut c, 4096).expect("io"), 4000);
    }

    #[test]
    fn ignores_unsigned_and_non_pe() {
        let pe = fake_pe(4096, 0, 0);
        assert_eq!(logical_end(&mut Cursor::new(&pe), 4096).expect("io"), 4096);
        let elf = b"\x7fELF".repeat(100);
        assert_eq!(logical_end(&mut Cursor::new(&elf), 400).expect("io"), 400);
        // Table not at end of file: not trusted.
        let pe = fake_pe(4096, 1000, 96);
        assert_eq!(logical_end(&mut Cursor::new(&pe), 4096).expect("io"), 4096);
    }
}
