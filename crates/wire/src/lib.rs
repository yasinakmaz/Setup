//! Compact little-endian binary encoding.
//!
//! Used for the payload index and the installation manifest. The format is
//! deliberately small and hand-written: generated installers must not pay for
//! a general serialization framework, and every decoder here must be safe on
//! untrusted input (bounded lengths, no panics, zero-copy borrows).
//!
//! Integers use fixed-width little-endian or unsigned LEB128 (`varint`).
//! Byte strings and UTF-8 strings are prefixed by a varint length.

#![forbid(unsafe_code)]

use core::fmt;

/// Decoding error. Carries no allocation so it is cheap to return from hot
/// paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    /// The input ended before the value was complete.
    UnexpectedEnd,
    /// A varint used more than 10 bytes or overflowed `u64`.
    VarintOverflow,
    /// A length prefix exceeded the configured limit or the remaining input.
    LengthOutOfBounds,
    /// A string was not valid UTF-8.
    InvalidUtf8,
    /// An enum tag was not recognised.
    InvalidTag { what: &'static str, tag: u64 },
    /// A magic number or version did not match.
    BadMagic,
    /// Input remained after the value was fully decoded.
    TrailingBytes,
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WireError::UnexpectedEnd => f.write_str("unexpected end of data"),
            WireError::VarintOverflow => f.write_str("varint overflow"),
            WireError::LengthOutOfBounds => f.write_str("length out of bounds"),
            WireError::InvalidUtf8 => f.write_str("invalid UTF-8"),
            WireError::InvalidTag { what, tag } => write!(f, "invalid {what} tag {tag}"),
            WireError::BadMagic => f.write_str("bad magic or unsupported version"),
            WireError::TrailingBytes => f.write_str("trailing bytes"),
        }
    }
}

impl std::error::Error for WireError {}

pub type Result<T> = core::result::Result<T, WireError>;

/// Appends encoded values to a caller-owned buffer (so it can be reused).
pub struct Writer<'a> {
    buf: &'a mut Vec<u8>,
}

impl<'a> Writer<'a> {
    #[inline]
    pub fn new(buf: &'a mut Vec<u8>) -> Self {
        Writer { buf }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    #[inline]
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    #[inline]
    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(u8::from(v))
    }

    #[inline]
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    #[inline]
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    #[inline]
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Unsigned LEB128.
    #[inline]
    pub fn varint(&mut self, mut v: u64) -> &mut Self {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(byte);
                return self;
            }
            self.buf.push(byte | 0x80);
        }
    }

    #[inline]
    pub fn raw(&mut self, bytes: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(bytes);
        self
    }

    #[inline]
    pub fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.varint(bytes.len() as u64);
        self.raw(bytes)
    }

    #[inline]
    pub fn str(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }

    #[inline]
    pub fn opt_str(&mut self, s: Option<&str>) -> &mut Self {
        match s {
            Some(s) => self.u8(1).str(s),
            None => self.u8(0),
        }
    }

    #[inline]
    pub fn hash(&mut self, h: &[u8; 32]) -> &mut Self {
        self.raw(h)
    }
}

/// Zero-copy decoder over a byte slice.
#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    /// Upper bound for any single length prefix, protecting against huge
    /// allocations or scans driven by hostile input.
    max_len: usize,
}

impl<'a> Reader<'a> {
    /// Default limit for a single length-prefixed value (64 MiB).
    pub const DEFAULT_MAX_LEN: usize = 64 << 20;

    #[inline]
    pub fn new(data: &'a [u8]) -> Self {
        Reader {
            data,
            pos: 0,
            max_len: Self::DEFAULT_MAX_LEN,
        }
    }

    #[inline]
    pub fn with_max_len(mut self, max_len: usize) -> Self {
        self.max_len = max_len;
        self
    }

    #[inline]
    pub fn position(&self) -> usize {
        self.pos
    }

    #[inline]
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Fails with [`WireError::TrailingBytes`] unless all input was consumed.
    pub fn finish(&self) -> Result<()> {
        if self.is_empty() {
            Ok(())
        } else {
            Err(WireError::TrailingBytes)
        }
    }

    #[inline]
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(WireError::UnexpectedEnd)?;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or(WireError::UnexpectedEnd)?;
        self.pos = end;
        Ok(slice)
    }

    #[inline]
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    #[inline]
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    #[inline]
    pub fn bool(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            tag => Err(WireError::InvalidTag {
                what: "bool",
                tag: u64::from(tag),
            }),
        }
    }

    #[inline]
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    #[inline]
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    #[inline]
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    #[inline]
    pub fn varint(&mut self) -> Result<u64> {
        let mut value = 0u64;
        for i in 0..10 {
            let byte = self.u8()?;
            let low = u64::from(byte & 0x7f);
            if i == 9 && byte > 1 {
                return Err(WireError::VarintOverflow);
            }
            value |= low << (7 * i);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(WireError::VarintOverflow)
    }

    /// Reads a varint that must fit in `usize` and not exceed `max_len`.
    #[inline]
    pub fn len(&mut self) -> Result<usize> {
        let len = usize::try_from(self.varint()?).map_err(|_| WireError::LengthOutOfBounds)?;
        if len > self.max_len || len > self.remaining() {
            return Err(WireError::LengthOutOfBounds);
        }
        Ok(len)
    }

    /// Reads a count of items that each occupy at least `min_item_size`
    /// bytes; rejects counts that cannot possibly fit in the input. This lets
    /// callers pre-allocate `Vec::with_capacity(count)` safely.
    pub fn count(&mut self, min_item_size: usize) -> Result<usize> {
        let count = usize::try_from(self.varint()?).map_err(|_| WireError::LengthOutOfBounds)?;
        let needed = count
            .checked_mul(min_item_size.max(1))
            .ok_or(WireError::LengthOutOfBounds)?;
        if needed > self.remaining() {
            return Err(WireError::LengthOutOfBounds);
        }
        Ok(count)
    }

    #[inline]
    pub fn bytes(&mut self) -> Result<&'a [u8]> {
        let len = self.len()?;
        self.take(len)
    }

    #[inline]
    pub fn str(&mut self) -> Result<&'a str> {
        core::str::from_utf8(self.bytes()?).map_err(|_| WireError::InvalidUtf8)
    }

    #[inline]
    pub fn opt_str(&mut self) -> Result<Option<&'a str>> {
        if self.bool()? {
            self.str().map(Some)
        } else {
            Ok(None)
        }
    }

    #[inline]
    pub fn hash(&mut self) -> Result<[u8; 32]> {
        self.array()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut buf = Vec::new();
        Writer::new(&mut buf)
            .u8(7)
            .bool(true)
            .u16(0xbeef)
            .u32(0xdead_beef)
            .u64(u64::MAX)
            .varint(0)
            .varint(300)
            .varint(u64::MAX)
            .str("Kurulum ✓ تثبيت")
            .opt_str(None)
            .opt_str(Some("x"))
            .hash(&[9; 32]);
        let mut r = Reader::new(&buf);
        assert_eq!(r.u8(), Ok(7));
        assert_eq!(r.bool(), Ok(true));
        assert_eq!(r.u16(), Ok(0xbeef));
        assert_eq!(r.u32(), Ok(0xdead_beef));
        assert_eq!(r.u64(), Ok(u64::MAX));
        assert_eq!(r.varint(), Ok(0));
        assert_eq!(r.varint(), Ok(300));
        assert_eq!(r.varint(), Ok(u64::MAX));
        assert_eq!(r.str(), Ok("Kurulum ✓ تثبيت"));
        assert_eq!(r.opt_str(), Ok(None));
        assert_eq!(r.opt_str(), Ok(Some("x")));
        assert_eq!(r.hash(), Ok([9; 32]));
        assert_eq!(r.finish(), Ok(()));
    }

    #[test]
    fn rejects_hostile_lengths() {
        let mut buf = Vec::new();
        Writer::new(&mut buf).varint(u64::MAX);
        assert_eq!(Reader::new(&buf).bytes(), Err(WireError::LengthOutOfBounds));

        let mut buf = Vec::new();
        Writer::new(&mut buf).varint(1_000_000);
        assert_eq!(
            Reader::new(&buf).count(8),
            Err(WireError::LengthOutOfBounds)
        );

        // 11-byte varint.
        let buf = [0xffu8; 11];
        assert_eq!(Reader::new(&buf).varint(), Err(WireError::VarintOverflow));

        assert_eq!(
            Reader::new(&[2]).bool(),
            Err(WireError::InvalidTag {
                what: "bool",
                tag: 2
            })
        );
        assert_eq!(Reader::new(&[1, 0xff]).str(), Err(WireError::InvalidUtf8));
        assert_eq!(Reader::new(&[1, 2]).u32(), Err(WireError::UnexpectedEnd));
    }

    #[test]
    fn never_panics_on_arbitrary_input() {
        // Cheap deterministic fuzz: every prefix of an encoded stream, plus
        // byte-flipped variants, must decode or fail cleanly.
        let mut buf = Vec::new();
        Writer::new(&mut buf)
            .str("hello")
            .varint(12345)
            .u64(1)
            .bytes(&[1, 2, 3]);
        for cut in 0..=buf.len() {
            let mut r = Reader::new(&buf[..cut]);
            let _ = r.str().and_then(|_| r.varint()).and_then(|_| r.u64());
        }
        for i in 0..buf.len() {
            let mut copy = buf.clone();
            copy[i] ^= 0xa5;
            let mut r = Reader::new(&copy);
            let _ = r.str().and_then(|_| r.varint()).and_then(|_| r.bytes());
        }
    }
}
