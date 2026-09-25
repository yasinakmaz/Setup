//! Validated relative paths.
//!
//! Every path that comes from a payload, a manifest or a project file is
//! untrusted until it passes [`RelPath::new`]. A valid `RelPath`:
//!
//! * uses `/` as the only separator and is not absolute;
//! * has no empty, `.` or `..` components (no traversal);
//! * contains no NUL, control characters, `\`, `:` (drive letters and NTFS
//!   alternate data streams) or other characters invalid on Windows;
//! * has no Windows reserved device names (`CON`, `NUL`, `COM1`, …) and no
//!   component ending in a dot or space;
//! * respects component and total length limits.
//!
//! The rules are the intersection of Windows and Linux so that a payload built
//! on one platform can never be interpreted differently on the other.

use std::fmt;
use std::path::{Path, PathBuf};

/// Maximum length of a single component, in bytes.
pub const MAX_COMPONENT_LEN: usize = 255;
/// Maximum length of the whole relative path, in bytes.
pub const MAX_PATH_LEN: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathError {
    Empty,
    TooLong,
    Absolute,
    EmptyComponent,
    DotComponent,
    InvalidChar(char),
    ReservedName,
    TrailingDotOrSpace,
    ComponentTooLong,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::Empty => f.write_str("path is empty"),
            PathError::TooLong => f.write_str("path is too long"),
            PathError::Absolute => f.write_str("path is absolute"),
            PathError::EmptyComponent => f.write_str("path has an empty component"),
            PathError::DotComponent => f.write_str("path contains '.' or '..'"),
            PathError::InvalidChar(c) => write!(f, "path contains invalid character {c:?}"),
            PathError::ReservedName => f.write_str("path uses a reserved device name"),
            PathError::TrailingDotOrSpace => {
                f.write_str("path component ends with a dot or space")
            }
            PathError::ComponentTooLong => f.write_str("path component is too long"),
        }
    }
}

impl std::error::Error for PathError {}

/// A borrowed, validated relative path. See the module documentation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RelPath<'a>(&'a str);

impl<'a> RelPath<'a> {
    pub fn new(path: &'a str) -> Result<Self, PathError> {
        validate(path)?;
        Ok(RelPath(path))
    }

    #[inline]
    pub fn as_str(self) -> &'a str {
        self.0
    }

    #[inline]
    pub fn components(self) -> impl DoubleEndedIterator<Item = &'a str> {
        self.0.split('/')
    }

    /// File name (last component).
    #[inline]
    pub fn file_name(self) -> &'a str {
        self.0.rsplit('/').next().unwrap_or(self.0)
    }

    /// Parent path, or `None` for a single-component path.
    pub fn parent(self) -> Option<RelPath<'a>> {
        self.0.rfind('/').map(|i| RelPath(&self.0[..i]))
    }

    /// Joins onto `root` using the platform separator. The result is always
    /// inside `root` because the path was validated.
    pub fn to_path_under(self, root: &Path) -> PathBuf {
        let mut out = PathBuf::with_capacity(root.as_os_str().len() + self.0.len() + 1);
        out.push(root);
        for c in self.components() {
            out.push(c);
        }
        out
    }
}

impl fmt::Debug for RelPath<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.0, f)
    }
}

impl fmt::Display for RelPath<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// Validates `path` according to the module rules.
pub fn validate(path: &str) -> Result<(), PathError> {
    if path.is_empty() {
        return Err(PathError::Empty);
    }
    if path.len() > MAX_PATH_LEN {
        return Err(PathError::TooLong);
    }
    if path.starts_with('/') {
        return Err(PathError::Absolute);
    }
    for component in path.split('/') {
        validate_component(component)?;
    }
    Ok(())
}

/// Validates a single path component (a file or directory name).
pub fn validate_component(component: &str) -> Result<(), PathError> {
    if component.is_empty() {
        return Err(PathError::EmptyComponent);
    }
    if component == "." || component == ".." {
        return Err(PathError::DotComponent);
    }
    if component.len() > MAX_COMPONENT_LEN {
        return Err(PathError::ComponentTooLong);
    }
    for c in component.chars() {
        if c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '/') {
            return Err(PathError::InvalidChar(c));
        }
    }
    if component.ends_with('.') || component.ends_with(' ') {
        return Err(PathError::TrailingDotOrSpace);
    }
    if is_reserved_device_name(component) {
        return Err(PathError::ReservedName);
    }
    Ok(())
}

/// Windows reserves device names regardless of extension (`nul.txt` opens the
/// NUL device on many Windows versions).
fn is_reserved_device_name(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component).trim_end();
    let bytes = stem.as_bytes();
    let eq = |name: &[u8]| bytes.eq_ignore_ascii_case(name);
    if eq(b"CON") || eq(b"PRN") || eq(b"AUX") || eq(b"NUL") || eq(b"CONIN$") || eq(b"CONOUT$") {
        return true;
    }
    if bytes.len() == 4 {
        let prefix = &bytes[..3];
        let digit = bytes[3];
        if (prefix.eq_ignore_ascii_case(b"COM") || prefix.eq_ignore_ascii_case(b"LPT"))
            && digit.is_ascii_digit()
            && digit != b'0'
        {
            return true;
        }
    }
    // COM¹ ² ³ and LPT¹ ² ³ are also reserved.
    if let Some(rest) = stem
        .get(..3)
        .filter(|p| p.eq_ignore_ascii_case("COM") || p.eq_ignore_ascii_case("LPT"))
        .and_then(|_| stem.get(3..))
    {
        return matches!(rest, "¹" | "²" | "³");
    }
    false
}

/// Converts an OS path relative to `root` into a `/`-separated string and
/// validates it. Used when scanning application directories.
pub fn relative_to(root: &Path, path: &Path) -> Result<String, PathError> {
    let rel = path.strip_prefix(root).map_err(|_| PathError::Absolute)?;
    let mut out = String::with_capacity(rel.as_os_str().len());
    for (i, component) in rel.components().enumerate() {
        let std::path::Component::Normal(name) = component else {
            return Err(PathError::DotComponent);
        };
        let name = name.to_str().ok_or(PathError::InvalidChar('\u{fffd}'))?;
        if i > 0 {
            out.push('/');
        }
        out.push_str(name);
    }
    validate(&out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_paths() {
        for p in [
            "app.exe",
            "bin/app",
            "lib/x86_64/libfoo.so.1",
            "şablonlar/ملف.txt",
            ".config/settings.json",
            "a b/c d.txt",
        ] {
            assert_eq!(validate(p), Ok(()), "{p}");
        }
    }

    #[test]
    fn rejects_traversal_and_absolute() {
        assert_eq!(validate("../etc/passwd"), Err(PathError::DotComponent));
        assert_eq!(validate("a/../../b"), Err(PathError::DotComponent));
        assert_eq!(validate("a/./b"), Err(PathError::DotComponent));
        assert_eq!(validate("/etc/passwd"), Err(PathError::Absolute));
        assert_eq!(validate("a//b"), Err(PathError::EmptyComponent));
        assert_eq!(validate("a/"), Err(PathError::EmptyComponent));
        assert_eq!(validate(""), Err(PathError::Empty));
    }

    #[test]
    fn rejects_windows_specials() {
        assert_eq!(validate("C:/Windows"), Err(PathError::InvalidChar(':')));
        assert_eq!(validate("file.txt:stream"), Err(PathError::InvalidChar(':')));
        assert_eq!(validate("a\\..\\b"), Err(PathError::InvalidChar('\\')));
        assert_eq!(validate("dir/NUL"), Err(PathError::ReservedName));
        assert_eq!(validate("com1.txt"), Err(PathError::ReservedName));
        assert_eq!(validate("LPT9"), Err(PathError::ReservedName));
        assert_eq!(validate("COM¹"), Err(PathError::ReservedName));
        assert_eq!(validate("com0"), Ok(()));
        assert_eq!(validate("console.log"), Ok(()));
        assert_eq!(validate("name."), Err(PathError::TrailingDotOrSpace));
        assert_eq!(validate("name "), Err(PathError::TrailingDotOrSpace));
        assert_eq!(validate("a\u{0}b"), Err(PathError::InvalidChar('\0')));
    }

    #[test]
    fn joins_under_root() {
        let root = Path::new("/opt/app");
        let rel = RelPath::new("bin/tool").expect("valid");
        assert_eq!(rel.to_path_under(root), Path::new("/opt/app/bin/tool"));
        assert_eq!(rel.file_name(), "tool");
        assert_eq!(rel.parent().map(RelPath::as_str), Some("bin"));
    }

    #[test]
    fn relative_to_rejects_escape() {
        let root = Path::new("/r");
        assert_eq!(relative_to(root, Path::new("/r/a/b")).as_deref(), Ok("a/b"));
        assert!(relative_to(root, Path::new("/other/a")).is_err());
    }
}
