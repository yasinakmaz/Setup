//! Directory creation that refuses to traverse symbolic links.
//!
//! When an installer writes `root/a/b/file`, an attacker who can plant a
//! symlink at `root/a` must not be able to redirect the write elsewhere. Each
//! component below `root` is created (or checked) with `symlink_metadata`, and
//! any symlink or non-directory is an error.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::relpath::RelPath;

/// Creates `root` itself (with parents) if missing, and returns the
/// directories that were newly created, outermost first.
pub fn create_root(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut missing = Vec::new();
    let mut cur = Some(root);
    while let Some(p) = cur {
        match fs::symlink_metadata(p) {
            Ok(meta) => {
                if !meta.is_dir() {
                    // Root may legitimately be reached through a symlink
                    // chosen by the user (e.g. /opt -> /data/opt); only the
                    // final target must be a directory.
                    if !fs::metadata(p).map(|m| m.is_dir()).unwrap_or(false) {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            format!("{} exists and is not a directory", p.display()),
                        ));
                    }
                }
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                missing.push(p.to_path_buf());
                cur = p.parent();
            }
            Err(e) => return Err(e),
        }
    }
    missing.reverse();
    let mut created = Vec::with_capacity(missing.len());
    for dir in missing {
        match fs::create_dir(&dir) {
            Ok(()) => created.push(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Ok(created)
}

/// Ensures `root/rel` exists as a real directory without following symlinks
/// below `root`. Newly created directories are appended to `created`.
pub fn ensure_dir_under(
    root: &Path,
    rel: RelPath<'_>,
    created: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let mut cur = root.to_path_buf();
    for component in rel.components() {
        cur.push(component);
        match fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("refusing to follow symbolic link {}", cur.display()),
                ));
            }
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} exists and is not a directory", cur.display()),
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => match fs::create_dir(&cur) {
                Ok(()) => created.push(cur.clone()),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    // Lost a race: re-check that what appeared is a directory.
                    let meta = fs::symlink_metadata(&cur)?;
                    if !meta.is_dir() || meta.file_type().is_symlink() {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            format!("{} was replaced concurrently", cur.display()),
                        ));
                    }
                }
                Err(e) => return Err(e),
            },
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Checks that an existing entry at `path` is not a symlink. Returns whether
/// the entry exists.
pub fn check_not_symlink(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("refusing to replace symbolic link {}", path.display()),
        )),
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_nested_dirs_and_reports_them() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut created = Vec::new();
        let rel = RelPath::new("a/b/c").expect("valid");
        ensure_dir_under(tmp.path(), rel, &mut created).expect("create");
        assert_eq!(created.len(), 3);
        created.clear();
        ensure_dir_under(tmp.path(), rel, &mut created).expect("idempotent");
        assert!(created.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_component() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir");
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("a")).expect("symlink");
        let mut created = Vec::new();
        let err = ensure_dir_under(
            tmp.path(),
            RelPath::new("a/b").expect("valid"),
            &mut created,
        )
        .expect_err("must refuse");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(!outside.path().join("b").exists());
    }

    #[test]
    fn create_root_reports_new_dirs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("x/y");
        let created = create_root(&root).expect("create");
        assert_eq!(created, vec![tmp.path().join("x"), root.clone()]);
        assert!(create_root(&root).expect("again").is_empty());
    }
}
