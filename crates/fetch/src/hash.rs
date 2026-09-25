//! Content hashes accepted for downloads.
//!
//! Vendors publish SHA-256 or SHA-512; our own artifacts use BLAKE3. The
//! algorithm is part of the identity of a download: `sha256:<hex>`.

use std::fmt;

use sha2::Digest;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ContentHash {
    Sha256([u8; 32]),
    Sha512([u8; 64]),
    Blake3([u8; 32]),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HashParseError;

impl fmt::Display for HashParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected `sha256:<64 hex>`, `sha512:<128 hex>` or `blake3:<64 hex>`")
    }
}

impl std::error::Error for HashParseError {}

fn decode_hex<const N: usize>(s: &str) -> Result<[u8; N], HashParseError> {
    let bytes = s.as_bytes();
    if bytes.len() != N * 2 {
        return Err(HashParseError);
    }
    let mut out = [0u8; N];
    for (i, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        let hi = (pair[0] as char).to_digit(16).ok_or(HashParseError)?;
        let lo = (pair[1] as char).to_digit(16).ok_or(HashParseError)?;
        out[i] = (hi * 16 + lo) as u8;
    }
    Ok(out)
}

impl ContentHash {
    pub fn parse(s: &str) -> Result<ContentHash, HashParseError> {
        let (algo, hex) = s.split_once(':').ok_or(HashParseError)?;
        match algo.to_ascii_lowercase().as_str() {
            "sha256" => Ok(ContentHash::Sha256(decode_hex(hex)?)),
            "sha512" => Ok(ContentHash::Sha512(decode_hex(hex)?)),
            "blake3" => Ok(ContentHash::Blake3(decode_hex(hex)?)),
            _ => Err(HashParseError),
        }
    }

    pub const fn algorithm(&self) -> &'static str {
        match self {
            ContentHash::Sha256(_) => "sha256",
            ContentHash::Sha512(_) => "sha512",
            ContentHash::Blake3(_) => "blake3",
        }
    }

    pub fn digest(&self) -> &[u8] {
        match self {
            ContentHash::Sha256(d) | ContentHash::Blake3(d) => d,
            ContentHash::Sha512(d) => d,
        }
    }

    /// Lower-case hex of the digest.
    pub fn hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let digest = self.digest();
        let mut s = String::with_capacity(digest.len() * 2);
        for b in digest {
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 15) as usize] as char);
        }
        s
    }

    /// File-name-safe key: `sha256-<hex>`.
    pub fn key(&self) -> String {
        format!("{}-{}", self.algorithm(), self.hex())
    }

    pub fn hasher(&self) -> StreamHasher {
        match self {
            ContentHash::Sha256(_) => StreamHasher::Sha256(sha2::Sha256::new()),
            ContentHash::Sha512(_) => StreamHasher::Sha512(sha2::Sha512::new()),
            ContentHash::Blake3(_) => StreamHasher::Blake3(Box::new(blake3::Hasher::new())),
        }
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.algorithm(), self.hex())
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Streaming hasher matching a [`ContentHash`] algorithm.
#[derive(Clone)]
pub enum StreamHasher {
    Sha256(sha2::Sha256),
    Sha512(sha2::Sha512),
    Blake3(Box<blake3::Hasher>),
}

impl StreamHasher {
    #[inline]
    pub fn update(&mut self, data: &[u8]) {
        match self {
            StreamHasher::Sha256(h) => h.update(data),
            StreamHasher::Sha512(h) => h.update(data),
            StreamHasher::Blake3(h) => {
                h.update(data);
            }
        }
    }

    pub fn finalize(self) -> ContentHash {
        match self {
            StreamHasher::Sha256(h) => ContentHash::Sha256(h.finalize().into()),
            StreamHasher::Sha512(h) => ContentHash::Sha512(h.finalize().into()),
            StreamHasher::Blake3(h) => ContentHash::Blake3(*h.finalize().as_bytes()),
        }
    }
}

/// Hashes a file with the algorithm of `like`.
pub fn hash_file(path: &std::path::Path, like: &ContentHash) -> std::io::Result<(ContentHash, u64)> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = like.hasher();
    let mut buf = vec![0u8; 256 << 10];
    let mut total = 0u64;
    loop {
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok((hasher.finalize(), total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats() {
        let s = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let h = ContentHash::parse(s).expect("parse");
        assert_eq!(h.to_string(), s);
        let mut hasher = h.hasher();
        hasher.update(b"");
        assert_eq!(hasher.finalize(), h);
        assert!(ContentHash::parse("md5:abcd").is_err());
        assert!(ContentHash::parse("sha256:zz").is_err());
        assert_eq!(h.key(), format!("sha256-{}", h.hex()));
    }
}
