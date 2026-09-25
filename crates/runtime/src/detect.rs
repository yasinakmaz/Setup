//! System detection: commands on PATH, versions, prerequisites.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Up to four numeric version parts. Missing parts compare as zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Ver(pub [u32; 4]);

impl Ver {
    /// Parses the first version-like token (`v8.0.11`, `git version 2.47.0.windows.1`).
    pub fn find_in(text: &str) -> Option<Ver> {
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_digit()
                && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'v')
            {
                let start = i;
                let mut end = i;
                let mut dots = 0;
                while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b'.') {
                    if bytes[end] == b'.' {
                        dots += 1;
                    }
                    end += 1;
                }
                if dots >= 1
                    && let Some(v) = Ver::parse(text[start..end].trim_end_matches('.'))
                {
                    return Some(v);
                }
                i = end;
            }
            i += 1;
        }
        None
    }

    pub fn parse(s: &str) -> Option<Ver> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        let core = s.split(['-', '+', ' ']).next()?;
        let mut parts = [0u32; 4];
        let mut n = 0;
        for piece in core.split('.') {
            if n == 4 {
                break;
            }
            parts[n] = piece.parse().ok()?;
            n += 1;
        }
        (n > 0).then_some(Ver(parts))
    }

    pub fn major(&self) -> u32 {
        self.0[0]
    }
}

/// Whether `found` satisfies a minimum version. With `same_major` the major
/// versions must match (e.g. .NET roll-forward within a major version).
pub fn satisfies(found: Ver, min: Option<Ver>, same_major: bool) -> bool {
    match min {
        None => true,
        Some(min) => found >= min && (!same_major || found.major() == min.major()),
    }
}

/// Finds an executable on `PATH`.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    if name.contains(['/', '\\']) {
        let p = PathBuf::from(name);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .map(|e| e.to_ascii_lowercase())
            .chain(std::iter::once(String::new()))
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Runs `program args` with a timeout and returns its (stdout, stderr).
pub fn run_capture(program: &Path, args: &[&str], timeout: Duration) -> Option<(String, String)> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let out_thread = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = stdout.read_to_end(&mut s);
        s
    });
    let err_thread = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = stderr.read_to_end(&mut s);
        s
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return None,
        }
    }
    let out = out_thread.join().ok()?;
    let err = err_thread.join().ok()?;
    Some((
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&err).into_owned(),
    ))
}

/// Windows build number (e.g. 22631), `None` elsewhere.
pub fn windows_build() -> Option<u32> {
    #[cfg(windows)]
    {
        let key = crate::platform::open_key(
            crate::platform::HKLM,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            false,
        )
        .ok()??;
        key.get_string("CurrentBuildNumber")
            .ok()??
            .trim()
            .parse()
            .ok()
    }
    #[cfg(not(windows))]
    None
}

/// How a prerequisite is detected (resolved from the catalog at build time).
#[derive(Clone, Copy, Debug)]
pub enum Detection {
    Registry {
        key: &'static str,
        value: &'static str,
        version_value: Option<&'static str>,
        per_user_too: bool,
    },
    DotnetSharedFramework {
        framework: &'static str,
    },
    Command {
        program: &'static str,
        args: &'static [&'static str],
        stderr: bool,
    },
    Service {
        name: &'static str,
    },
}

/// Result of detecting a prerequisite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detected {
    Missing,
    /// Present; version when known.
    Present(Option<Ver>),
}

pub fn detect(d: &Detection) -> Detected {
    match d {
        Detection::Command {
            program,
            args,
            stderr,
        } => {
            let Some(path) = find_on_path(program) else {
                return Detected::Missing;
            };
            match run_capture(&path, args, Duration::from_secs(15)) {
                Some((out, err)) => {
                    let text = if *stderr { &err } else { &out };
                    Detected::Present(Ver::find_in(text).or_else(|| Ver::find_in(&err)))
                }
                None => Detected::Present(None),
            }
        }
        Detection::DotnetSharedFramework { framework } => {
            let mut best: Option<Ver> = None;
            for root in dotnet_roots() {
                let dir = root.join("shared").join(framework);
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for e in entries.flatten() {
                        if let Some(v) = e.file_name().to_str().and_then(Ver::parse)
                            && best.is_none_or(|b| v > b)
                        {
                            best = Some(v);
                        }
                    }
                }
            }
            best.map_or(Detected::Missing, |v| Detected::Present(Some(v)))
        }
        Detection::Registry {
            key,
            value,
            version_value,
            per_user_too,
        } => detect_registry(key, value, *version_value, *per_user_too),
        Detection::Service { name } => detect_service(name),
    }
}

/// Detects all installed framework versions (for multi-version checks).
pub fn dotnet_framework_versions(framework: &str) -> Vec<Ver> {
    let mut out = Vec::new();
    for root in dotnet_roots() {
        if let Ok(entries) = std::fs::read_dir(root.join("shared").join(framework)) {
            out.extend(
                entries
                    .flatten()
                    .filter_map(|e| e.file_name().to_str().and_then(Ver::parse)),
            );
        }
    }
    out.sort();
    out.dedup();
    out
}

fn dotnet_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(r) = std::env::var_os("DOTNET_ROOT") {
        roots.push(PathBuf::from(r));
    }
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramW6432"] {
            if let Some(pf) = std::env::var_os(var) {
                roots.push(PathBuf::from(pf).join("dotnet"));
            }
        }
    } else {
        roots.push(crate::platform::home_dir().join(".dotnet"));
        for p in [
            "/usr/share/dotnet",
            "/usr/lib/dotnet",
            "/usr/lib64/dotnet",
            "/opt/dotnet",
        ] {
            roots.push(PathBuf::from(p));
        }
    }
    roots.dedup();
    roots
}

#[cfg(windows)]
fn detect_registry(
    key: &str,
    value: &str,
    version_value: Option<&str>,
    per_user_too: bool,
) -> Detected {
    use crate::platform::{HKCU, HKLM, open_key};
    let hives: &[u8] = if per_user_too { &[HKLM, HKCU] } else { &[HKLM] };
    for &hive in hives {
        let Ok(Some(k)) = open_key(hive, key, false) else {
            continue;
        };
        let present = match k.get_raw(value) {
            Ok(Some((4, data))) => data.first().is_some_and(|b| *b != 0), // REG_DWORD
            Ok(Some((_, data))) => !data.is_empty(),
            _ => value.is_empty(),
        };
        if present {
            let v = version_value
                .and_then(|n| k.get_string(n).ok().flatten())
                .and_then(|s| Ver::parse(&s));
            return Detected::Present(v);
        }
    }
    Detected::Missing
}

#[cfg(not(windows))]
fn detect_registry(_: &str, _: &str, _: Option<&str>, _: bool) -> Detected {
    Detected::Missing
}

fn detect_service(name: &str) -> Detected {
    #[cfg(windows)]
    {
        let sc = std::env::var_os("SystemRoot")
            .map(|r| PathBuf::from(r).join("System32").join("sc.exe"))
            .unwrap_or_else(|| PathBuf::from("sc.exe"));
        match run_capture(&sc, &["query", name], Duration::from_secs(10)) {
            Some((out, _)) if out.contains("SERVICE_NAME") => Detected::Present(None),
            _ => Detected::Missing,
        }
    }
    #[cfg(not(windows))]
    {
        let unit = format!("{name}.service");
        for user in [false, true] {
            let mut args = vec!["list-unit-files", unit.as_str(), "--no-legend"];
            if user {
                args.insert(0, "--user");
            }
            if let Some((out, _)) =
                run_capture(Path::new("systemctl"), &args, Duration::from_secs(10))
                && out.contains(&unit)
            {
                return Detected::Present(None);
            }
        }
        Detected::Missing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_versions_in_tool_output() {
        assert_eq!(Ver::find_in("v22.11.0\n"), Some(Ver([22, 11, 0, 0])));
        assert_eq!(
            Ver::find_in("git version 2.47.0.windows.1"),
            Some(Ver([2, 47, 0, 0]))
        );
        assert_eq!(
            Ver::find_in("psql (PostgreSQL) 16.4"),
            Some(Ver([16, 4, 0, 0]))
        );
        assert_eq!(
            Ver::find_in("openjdk version \"21.0.4\" 2024-07-16"),
            Some(Ver([21, 0, 4, 0]))
        );
        assert_eq!(Ver::find_in("Python 3.12.7"), Some(Ver([3, 12, 7, 0])));
        assert_eq!(Ver::find_in("no version here 7"), None);
        assert_eq!(Ver::parse("v14.40.33810.00"), Some(Ver([14, 40, 33810, 0])));
    }

    #[test]
    fn version_requirements() {
        let v = |s| Ver::parse(s).expect("v");
        assert!(satisfies(v("8.0.11"), Some(v("8.0")), true));
        assert!(!satisfies(v("9.0.0"), Some(v("8.0")), true));
        assert!(satisfies(v("9.0.0"), Some(v("8.0")), false));
        assert!(!satisfies(v("7.0.20"), Some(v("8.0")), false));
        assert!(satisfies(v("1.0"), None, true));
    }

    #[test]
    fn detects_commands() {
        assert!(find_on_path("sh").is_some() || cfg!(windows));
        assert_eq!(
            detect(&Detection::Command {
                program: "definitely-not-a-real-program-xyz",
                args: &[],
                stderr: false
            }),
            Detected::Missing
        );
    }
}
