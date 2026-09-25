//! Application directory scanning.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// Kind of binary detected from file magic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinaryKind {
    Elf,
    Pe,
}

#[derive(Clone, Debug)]
pub struct ScannedFile {
    /// `/`-separated path relative to the application directory.
    pub rel: String,
    pub abs: PathBuf,
    pub size: u64,
    pub binary: Option<BinaryKind>,
    /// Should be installed with the executable bit (Unix).
    pub executable: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Scan {
    pub files: Vec<ScannedFile>,
    /// Directories without any included file (kept as explicit entries).
    pub empty_dirs: Vec<String>,
    pub excluded: usize,
    pub skipped_symlinks: Vec<String>,
    pub total_size: u64,
}

/// Matches `text` against a glob: `*` (within a component), `?`, `**`
/// (across components).
pub fn glob_match(pattern: &str, text: &str) -> bool {
    fn rec(p: &[u8], t: &[u8]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some(b'*') if p.get(1) == Some(&b'*') => {
                let rest = p[2..].strip_prefix(b"/").unwrap_or(&p[2..]);
                rest.is_empty()
                    || (0..=t.len()).any(|i| (i == 0 || t[i - 1] == b'/') && rec(rest, &t[i..]))
                    || rec(rest, t)
            }
            Some(b'*') => (0..=t.len())
                .take_while(|&i| i == 0 || t[i - 1] != b'/')
                .any(|i| rec(&p[1..], &t[i..])),
            Some(b'?') => t.first().is_some_and(|c| *c != b'/') && rec(&p[1..], &t[1..]),
            Some(c) => {
                t.first().is_some_and(|x| x.eq_ignore_ascii_case(c)) && rec(&p[1..], &t[1..])
            }
        }
    }
    rec(pattern.as_bytes(), text.as_bytes())
}

/// Exclusion rules: `*.pdb` matches a file name anywhere, `logs/` a
/// directory anywhere, patterns containing `/` match the whole path.
pub struct Excludes<'a>(pub &'a [String]);

impl Excludes<'_> {
    pub fn excludes(&self, rel: &str, is_dir: bool) -> bool {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        self.0.iter().any(|p| {
            if let Some(dir) = p.strip_suffix('/') {
                is_dir && (glob_match(dir, name) || glob_match(dir, rel))
            } else if p.contains('/') {
                glob_match(p, rel)
            } else {
                glob_match(p, name)
            }
        })
    }
}

fn detect_binary(path: &Path) -> Option<BinaryKind> {
    let mut magic = [0u8; 4];
    let mut f = fs::File::open(path).ok()?;
    f.read_exact(&mut magic).ok()?;
    if &magic == b"\x7fELF" {
        Some(BinaryKind::Elf)
    } else if &magic[..2] == b"MZ" {
        Some(BinaryKind::Pe)
    } else {
        None
    }
}

#[cfg(unix)]
fn has_exec_bit(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn has_exec_bit(_: &fs::Metadata) -> bool {
    false
}

pub fn scan(root: &Path, excludes: &[String]) -> io::Result<Scan> {
    let ex = Excludes(excludes);
    let mut out = Scan::default();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, rel_dir)) = stack.pop() {
        let mut entries: Vec<fs::DirEntry> = fs::read_dir(&dir)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(fs::DirEntry::file_name);
        let mut included_children = 0usize;
        for entry in entries {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} has a non UTF-8 name", entry.path().display()),
                ));
            };
            let rel = if rel_dir.is_empty() {
                name.to_owned()
            } else {
                format!("{rel_dir}/{name}")
            };
            let ft = entry.file_type()?;
            let path = entry.path();
            if ft.is_symlink() {
                match fs::metadata(&path) {
                    Ok(m) if m.is_file() => {} // follow file links
                    _ => {
                        out.skipped_symlinks.push(rel);
                        continue;
                    }
                }
            }
            let meta = fs::metadata(&path)?;
            if ex.excludes(&rel, meta.is_dir()) {
                out.excluded += 1;
                continue;
            }
            included_children += 1;
            if meta.is_dir() {
                stack.push((path, rel));
            } else if meta.is_file() {
                inst_fsx::relpath::validate(&rel).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("{rel}: {e}"))
                })?;
                let binary = detect_binary(&path);
                let executable = has_exec_bit(&meta) || binary == Some(BinaryKind::Elf);
                out.total_size += meta.len();
                out.files.push(ScannedFile {
                    rel,
                    abs: path,
                    size: meta.len(),
                    binary,
                    executable,
                });
            }
        }
        if included_children == 0 && !rel_dir.is_empty() {
            out.empty_dirs.push(rel_dir);
        }
    }
    out.files.sort_by(|a, b| a.rel.cmp(&b.rel));
    out.empty_dirs.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("*.pdb", "app.pdb"));
        assert!(!glob_match("*.pdb", "dir/app.pdb"));
        assert!(glob_match("**/*.pdb", "dir/sub/app.pdb"));
        assert!(glob_match("**/*.pdb", "app.pdb"));
        assert!(glob_match("docs/**", "docs/a/b.md"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "a/c"));
        assert!(glob_match("THUMBS.db", "Thumbs.db"));
    }

    #[test]
    fn scans_with_excludes() {
        let tmp = tempfile::tempdir().expect("tmp");
        let r = tmp.path();
        for (p, c) in [
            ("app", b"\x7fELF...".as_slice()),
            ("app.pdb", b"x"),
            ("lib/x.dll", b"MZ.."),
            ("logs/today.log", b"x"),
            ("data/config.json", b"{}"),
        ] {
            fs::create_dir_all(r.join(p).parent().expect("parent")).expect("mkdir");
            fs::write(r.join(p), c).expect("write");
        }
        fs::create_dir_all(r.join("plugins")).expect("mkdir");
        let s = scan(r, &["*.pdb".into(), "logs/".into()]).expect("scan");
        let names: Vec<&str> = s.files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(names, ["app", "data/config.json", "lib/x.dll"]);
        assert_eq!(s.excluded, 2);
        assert_eq!(s.empty_dirs, ["plugins"]);
        assert_eq!(s.files[0].binary, Some(BinaryKind::Elf));
        assert!(s.files[0].executable);
        assert_eq!(s.files[2].binary, Some(BinaryKind::Pe));
    }
}
