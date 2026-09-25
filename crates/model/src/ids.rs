//! Identifiers and versions.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;

/// Stable product identifier, e.g. `com.acme.orders`.
///
/// It keys the Windows uninstall registry entry, the Linux installed-product
/// registry and upgrade detection, so it must never change between versions.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProductId(String);

impl ProductId {
    pub fn new(id: impl Into<String>) -> Result<Self, IdError> {
        let id = id.into();
        validate_product_id(&id)?;
        Ok(ProductId(id))
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Derives a reasonable id from a product and publisher name
    /// (`Acme Ltd.` + `Orders` → `acme-ltd.orders`).
    pub fn suggest(publisher: &str, product: &str) -> ProductId {
        let mut id = String::with_capacity(publisher.len() + product.len() + 1);
        let publisher = slug(publisher);
        if !publisher.is_empty() {
            id.push_str(&publisher);
            id.push('.');
        }
        let product = slug(product);
        id.push_str(if product.is_empty() { "app" } else { &product });
        ProductId(id)
    }
}

impl fmt::Display for ProductId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for ProductId {
    type Error = IdError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        ProductId::new(value)
    }
}

impl From<ProductId> for String {
    fn from(value: ProductId) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdError {
    Length,
    InvalidChar(char),
    Edge,
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdError::Length => f.write_str("identifier must be 1-128 characters"),
            IdError::InvalidChar(c) => write!(f, "identifier contains invalid character {c:?}"),
            IdError::Edge => f.write_str("identifier must start and end with a letter or digit"),
        }
    }
}

impl std::error::Error for IdError {}

fn validate_product_id(id: &str) -> Result<(), IdError> {
    if id.is_empty() || id.len() > 128 {
        return Err(IdError::Length);
    }
    if let Some(c) = id
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
    {
        return Err(IdError::InvalidChar(c));
    }
    let first = id.chars().next().unwrap_or('.');
    let last = id.chars().last().unwrap_or('.');
    if !first.is_ascii_alphanumeric() || !last.is_ascii_alphanumeric() {
        return Err(IdError::Edge);
    }
    Ok(())
}

/// Validates a short local identifier (node, action, field, component ids):
/// ASCII letters, digits, `-` and `_`.
pub fn validate_local_id(id: &str) -> Result<(), IdError> {
    if id.is_empty() || id.len() > 64 {
        return Err(IdError::Length);
    }
    if let Some(c) = id
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_')))
    {
        return Err(IdError::InvalidChar(c));
    }
    Ok(())
}

/// Lower-case ASCII slug: letters and digits separated by single `-`.
pub fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut dash = false;
    for c in s.chars() {
        let c = fold_ascii(c);
        if c.is_ascii_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c.to_ascii_lowercase());
        } else {
            dash = true;
        }
    }
    out
}

/// Maps common accented Latin and Turkish letters to ASCII for slugs.
fn fold_ascii(c: char) -> char {
    match c {
        'ç' | 'Ç' => 'c',
        'ğ' | 'Ğ' => 'g',
        'ı' | 'İ' => 'i',
        'ö' | 'Ö' | 'ó' | 'ò' | 'ô' | 'õ' => 'o',
        'ş' | 'Ş' => 's',
        'ü' | 'Ü' | 'ú' | 'ù' | 'û' => 'u',
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ß' => 's',
        other => other,
    }
}

/// Product version: up to four numeric parts plus an optional pre-release
/// tag (`1.4.0`, `2.0.0.17`, `3.1.0-beta.2`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Version {
    parts: [u32; 4],
    len: u8,
    pre: Option<String>,
}

impl Version {
    pub fn parse(s: &str) -> Result<Version, VersionError> {
        let s = s.trim();
        let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
        let (core, pre) = match s.split_once(['-', '+']) {
            Some((core, pre)) if !pre.is_empty() => (core, Some(pre.to_owned())),
            Some(_) => return Err(VersionError),
            None => (s, None),
        };
        let mut parts = [0u32; 4];
        let mut len = 0u8;
        for piece in core.split('.') {
            if len == 4 || piece.is_empty() || !piece.bytes().all(|b| b.is_ascii_digit()) {
                return Err(VersionError);
            }
            parts[len as usize] = piece.parse().map_err(|_| VersionError)?;
            len += 1;
        }
        if len == 0 {
            return Err(VersionError);
        }
        Ok(Version { parts, len, pre })
    }

    #[inline]
    pub fn parts(&self) -> &[u32] {
        &self.parts[..self.len as usize]
    }

    #[inline]
    pub fn pre(&self) -> Option<&str> {
        self.pre.as_deref()
    }

    pub fn major(&self) -> u32 {
        self.parts[0]
    }

    /// Windows `VS_FIXEDFILEINFO` requires every part to fit in 16 bits.
    pub fn fits_windows_file_version(&self) -> bool {
        self.parts().iter().all(|p| *p <= u32::from(u16::MAX))
    }

    /// Four 16-bit parts for PE version resources (missing parts are 0).
    pub fn windows_quad(&self) -> [u16; 4] {
        self.parts.map(|p| u16::try_from(p).unwrap_or(u16::MAX))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.parts.cmp(&other.parts) {
            Ordering::Equal => match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => cmp_prerelease(a, b),
            },
            other => other,
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// SemVer-style pre-release comparison: numeric identifiers compare
/// numerically and sort before alphanumeric ones.
fn cmp_prerelease(a: &str, b: &str) -> Ordering {
    let mut ai = a.split('.');
    let mut bi = b.split('.');
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, p) in self.parts().iter().enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            write!(f, "{p}")?;
        }
        if let Some(pre) = &self.pre {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

impl TryFrom<String> for Version {
    type Error = VersionError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Version::parse(&value)
    }
}

impl From<Version> for String {
    fn from(value: Version) -> Self {
        value.to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersionError;

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid version (expected e.g. 1.4.0 or 2.0.0-beta.1)")
    }
}

impl std::error::Error for VersionError {}

/// A simple version requirement: `*`, `>=8.0`, `>8`, `=1.2.3`, `^8.0`
/// (same major), `~8.0` (same major.minor).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VersionReq {
    op: ReqOp,
    version: Option<Version>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ReqOp {
    Any,
    Ge,
    Gt,
    Eq,
    Caret,
    Tilde,
}

impl VersionReq {
    pub const ANY: VersionReq = VersionReq {
        op: ReqOp::Any,
        version: None,
    };

    pub fn parse(s: &str) -> Result<VersionReq, VersionError> {
        let s = s.trim();
        if s.is_empty() || s == "*" {
            return Ok(VersionReq::ANY);
        }
        let (op, rest) = if let Some(r) = s.strip_prefix(">=") {
            (ReqOp::Ge, r)
        } else if let Some(r) = s.strip_prefix('>') {
            (ReqOp::Gt, r)
        } else if let Some(r) = s.strip_prefix('=') {
            (ReqOp::Eq, r)
        } else if let Some(r) = s.strip_prefix('^') {
            (ReqOp::Caret, r)
        } else if let Some(r) = s.strip_prefix('~') {
            (ReqOp::Tilde, r)
        } else {
            (ReqOp::Ge, s)
        };
        Ok(VersionReq {
            op,
            version: Some(Version::parse(rest)?),
        })
    }

    pub fn matches(&self, v: &Version) -> bool {
        let Some(req) = &self.version else {
            return true;
        };
        match self.op {
            ReqOp::Any => true,
            ReqOp::Ge => v >= req,
            ReqOp::Gt => v > req,
            ReqOp::Eq => v == req,
            ReqOp::Caret => v >= req && v.parts[0] == req.parts[0],
            ReqOp::Tilde => v >= req && v.parts[0] == req.parts[0] && v.parts[1] == req.parts[1],
        }
    }

    /// Minimum version this requirement accepts, if bounded.
    pub fn minimum(&self) -> Option<&Version> {
        self.version.as_ref()
    }
}

impl Default for VersionReq {
    fn default() -> Self {
        VersionReq::ANY
    }
}

impl fmt::Display for VersionReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let op = match self.op {
            ReqOp::Any => return f.write_str("*"),
            ReqOp::Ge => ">=",
            ReqOp::Gt => ">",
            ReqOp::Eq => "=",
            ReqOp::Caret => "^",
            ReqOp::Tilde => "~",
        };
        match &self.version {
            Some(v) => write!(f, "{op}{v}"),
            None => f.write_str("*"),
        }
    }
}

impl TryFrom<String> for VersionReq {
    type Error = VersionError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        VersionReq::parse(&value)
    }
}

impl From<VersionReq> for String {
    fn from(value: VersionReq) -> Self {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_ids() {
        assert!(ProductId::new("com.acme.orders").is_ok());
        assert!(ProductId::new("acme_orders-2").is_ok());
        assert_eq!(ProductId::new(""), Err(IdError::Length));
        assert_eq!(ProductId::new("a b"), Err(IdError::InvalidChar(' ')));
        assert_eq!(ProductId::new(".x"), Err(IdError::Edge));
        assert_eq!(
            ProductId::suggest("Açık Yazılım A.Ş.", "Sipariş Yönetimi").as_str(),
            "acik-yazilim-a-s.siparis-yonetimi"
        );
    }

    #[test]
    fn versions_order() {
        let v = |s| Version::parse(s).expect(s);
        assert!(v("1.4.0") > v("1.3.9"));
        assert!(v("1.4") == v("1.4.0.0") || v("1.4").parts() != v("1.4.0.0").parts());
        assert!(v("2.0.0-beta.2") < v("2.0.0"));
        assert!(v("2.0.0-beta.2") > v("2.0.0-beta.1"));
        assert!(v("2.0.0-alpha") < v("2.0.0-beta"));
        assert!(v("2.0.0-1") < v("2.0.0-alpha"));
        assert_eq!(v("v1.2.3").to_string(), "1.2.3");
        assert!(Version::parse("1..2").is_err());
        assert!(Version::parse("1.2.3.4.5").is_err());
        assert!(Version::parse("x").is_err());
        assert!(!v("70000.1").fits_windows_file_version());
    }

    #[test]
    fn version_requirements() {
        let v = |s| Version::parse(s).expect(s);
        let r = |s| VersionReq::parse(s).expect(s);
        assert!(r(">=8.0").matches(&v("8.0.11")));
        assert!(!r(">=8.0").matches(&v("7.0.20")));
        assert!(r("^8.0").matches(&v("8.9")));
        assert!(!r("^8.0").matches(&v("9.0")));
        assert!(r("~8.1").matches(&v("8.1.5")));
        assert!(!r("~8.1").matches(&v("8.2.0")));
        assert!(r("*").matches(&v("0.1")));
        assert!(r("8").matches(&v("8.0")));
        assert_eq!(r("^8.0").to_string(), "^8.0");
    }
}
