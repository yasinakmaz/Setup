//! Project Analyzer.
//!
//! Given an application directory or executable, extracts what can be
//! known (name, version, publisher, architecture, main executable, icon,
//! size, file count, runtime dependencies) and suggests prerequisites,
//! database/network usage, services and configuration files.
//!
//! Every suggestion carries a **reason** and a **confidence**. Nothing is
//! presented as certain unless it was read from the binaries themselves.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use inst_model::platform::{Arch, Os};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl fmt::Display for Confidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Confidence::Low => "Low",
            Confidence::Medium => "Medium",
            Confidence::High => "High",
        })
    }
}

/// A value with the evidence behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggested<T> {
    pub value: T,
    pub reason: String,
    pub confidence: Confidence,
}

impl<T> Suggested<T> {
    fn new(value: T, reason: impl Into<String>, confidence: Confidence) -> Self {
        Suggested {
            value,
            reason: reason.into(),
            confidence,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HintKind {
    /// A catalog prerequisite, with a version requirement.
    Prerequisite {
        id: String,
        version: String,
    },
    Database {
        provider: &'static str,
    },
    Network,
    ServiceExecutable {
        path: String,
    },
    ConfigFile {
        path: String,
    },
    /// A dependency is shipped with the application (informational).
    Bundled {
        what: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub kind: HintKind,
    pub reason: String,
    pub confidence: Confidence,
}

#[derive(Clone, Debug)]
pub struct Analysis {
    /// Application directory.
    pub root: PathBuf,
    pub platform: Suggested<Os>,
    pub arch: Suggested<Arch>,
    pub name: Suggested<String>,
    pub version: Suggested<String>,
    pub publisher: Suggested<String>,
    pub description: Option<String>,
    pub main_executable: Option<Suggested<String>>,
    /// `.ico` extracted from the main executable, if any.
    pub icon_ico: Option<Vec<u8>>,
    pub file_count: usize,
    pub total_size: u64,
    /// DLL imports / ELF `NEEDED` entries of the main executable.
    pub runtime_dependencies: Vec<String>,
    pub hints: Vec<Hint>,
}

#[derive(Debug)]
pub enum AnalyzeError {
    Io(PathBuf, std::io::Error),
    Empty(PathBuf),
}

impl fmt::Display for AnalyzeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnalyzeError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            AnalyzeError::Empty(p) => write!(f, "{} contains no files", p.display()),
        }
    }
}

impl std::error::Error for AnalyzeError {}

struct Entry {
    rel: String,
    lower: String,
    size: u64,
    executable_bit: bool,
}

/// Maximum size of a binary we parse (larger binaries are still listed).
const MAX_PARSE: u64 = 512 << 20;

fn walk(root: &Path) -> Result<Vec<Entry>, AnalyzeError> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        let rd = fs::read_dir(&dir).map_err(|e| AnalyzeError::Io(dir.clone(), e))?;
        for entry in rd.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push((entry.path(), rel));
            } else if let Ok(meta) = entry.metadata()
                && meta.is_file()
            {
                #[cfg(unix)]
                let executable_bit = {
                    use std::os::unix::fs::PermissionsExt;
                    meta.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let executable_bit = false;
                out.push(Entry {
                    lower: rel.to_lowercase(),
                    rel,
                    size: meta.len(),
                    executable_bit,
                });
            }
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

fn read_head(path: &Path, n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    let len = fs::File::open(path)
        .and_then(|mut f| f.read(&mut buf))
        .unwrap_or(0);
    buf.truncate(len);
    buf
}

#[derive(Default)]
struct BinaryInfo {
    os: Option<Os>,
    arch: Option<Arch>,
    product_name: Option<String>,
    file_description: Option<String>,
    company: Option<String>,
    version: Option<String>,
    imports: Vec<String>,
    icon: Option<Vec<u8>>,
}

fn inspect_binary(path: &Path) -> Option<BinaryInfo> {
    let meta = fs::metadata(path).ok()?;
    if meta.len() > MAX_PARSE {
        return None;
    }
    let head = read_head(path, 4);
    if head.starts_with(b"MZ") {
        let bytes = fs::read(path).ok()?;
        inspect_pe(&bytes)
    } else if head.starts_with(b"\x7fELF") {
        let bytes = fs::read(path).ok()?;
        let elf = goblin::elf::Elf::parse(&bytes).ok()?;
        let arch = match elf.header.e_machine {
            goblin::elf::header::EM_X86_64 => Some(Arch::X64),
            goblin::elf::header::EM_AARCH64 => Some(Arch::Arm64),
            _ => None,
        };
        Some(BinaryInfo {
            os: Some(Os::Linux),
            arch,
            imports: elf.libraries.iter().map(|s| (*s).to_owned()).collect(),
            ..BinaryInfo::default()
        })
    } else {
        None
    }
}

fn inspect_pe(bytes: &[u8]) -> Option<BinaryInfo> {
    use pelite::{PeFile, Wrap};
    let pe = PeFile::from_bytes(bytes).ok()?;
    let arch = match pe.file_header().Machine {
        0x8664 => Some(Arch::X64),
        0xAA64 => Some(Arch::Arm64),
        _ => None,
    };
    let mut info = BinaryInfo {
        os: Some(Os::Windows),
        arch,
        ..BinaryInfo::default()
    };
    if let Ok(imports) = pe.imports() {
        match imports {
            Wrap::T32(i) => {
                for d in i.iter() {
                    if let Ok(n) = d.dll_name() {
                        info.imports.push(n.to_string());
                    }
                }
            }
            Wrap::T64(i) => {
                for d in i.iter() {
                    if let Ok(n) = d.dll_name() {
                        info.imports.push(n.to_string());
                    }
                }
            }
        }
    }
    if let Ok(res) = pe.resources() {
        if let Ok(vi) = res.version_info() {
            if let Some(lang) = vi.translation().first().copied() {
                let get = |k: &str| {
                    vi.value(lang, k)
                        .map(|s| s.trim_end_matches('\0').trim().to_owned())
                        .filter(|s| !s.is_empty())
                };
                info.product_name = get("ProductName");
                info.file_description = get("FileDescription");
                info.company = get("CompanyName");
                info.version = get("ProductVersion").or_else(|| get("FileVersion"));
            }
            if info.version.is_none()
                && let Some(f) = vi.fixed()
            {
                let v = f.dwProductVersion;
                info.version = Some(format!("{}.{}.{}.{}", v.Major, v.Minor, v.Patch, v.Build));
            }
        }
        if let Some(Ok((_, group))) = res.icons().next() {
            let mut ico = Vec::new();
            if group.write(&mut ico).is_ok() {
                info.icon = Some(ico);
            }
        }
    }
    Some(info)
}

/// Keeps the numeric core of a version string (`1.4.0.0 (x64)` → `1.4.0`).
fn clean_version(v: &str) -> Option<String> {
    let normalized = v.replace(", ", ".").replace(',', ".");
    let token = normalized.split([' ', '+', '(']).next()?.trim().to_owned();
    let mut parts: Vec<&str> = token.split('.').collect();
    while parts.len() > 3 && parts.last() == Some(&"0") {
        parts.pop();
    }
    let joined = parts.join(".");
    inst_model::Version::parse(&joined)
        .ok()
        .map(|v| v.to_string())
}

fn is_binary_candidate(e: &Entry) -> bool {
    e.lower.ends_with(".exe")
        || (e.executable_bit && !e.lower.contains('.'))
        || e.executable_bit && e.lower.ends_with(".appimage")
}

fn helper_name(lower: &str) -> bool {
    let name = lower.rsplit('/').next().unwrap_or(lower);
    [
        "unins",
        "uninstall",
        "crashpad",
        "crash",
        "update",
        "updater",
        "helper",
        "elevate",
        "createdump",
        "notification",
    ]
    .iter()
    .any(|h| name.contains(h))
}

pub fn analyze(input: &Path) -> Result<Analysis, AnalyzeError> {
    let meta = fs::metadata(input).map_err(|e| AnalyzeError::Io(input.to_path_buf(), e))?;
    let (root, explicit_main) = if meta.is_file() {
        let parent = input.parent().unwrap_or(Path::new("."));
        let name = input
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned);
        // `app/bin/tool` → the application folder is `app`.
        let in_bin = parent
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("bin"));
        match (in_bin, parent.parent()) {
            (true, Some(grand)) => (grand.to_path_buf(), name.map(|n| format!("bin/{n}"))),
            _ => (parent.to_path_buf(), name),
        }
    } else {
        (input.to_path_buf(), None)
    };
    let entries = walk(&root)?;
    if entries.is_empty() {
        return Err(AnalyzeError::Empty(root));
    }
    let total_size = entries.iter().map(|e| e.size).sum();
    let dir_name = root
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "Application".into());

    // Main executable.
    let main = if let Some(m) = explicit_main {
        Some(Suggested::new(
            m,
            "selected by the developer",
            Confidence::High,
        ))
    } else {
        let dir_lower = dir_name.to_lowercase();
        let mut candidates: Vec<(&Entry, i64)> = entries
            .iter()
            .filter(|e| is_binary_candidate(e) && !helper_name(&e.lower))
            .map(|e| {
                let depth = e.rel.matches('/').count() as i64;
                let stem = e
                    .lower
                    .rsplit('/')
                    .next()
                    .unwrap_or(&e.lower)
                    .trim_end_matches(".exe")
                    .to_owned();
                let mut score = -depth * 1000 + (e.size as i64 >> 16).min(500);
                if dir_lower.contains(&stem) || stem.contains(&dir_lower) {
                    score += 800;
                }
                (e, score)
            })
            .collect();
        candidates.sort_by_key(|(_, s)| std::cmp::Reverse(*s));
        match candidates.as_slice() {
            [] => None,
            [(only, _)] => Some(Suggested::new(
                only.rel.clone(),
                "only executable in the folder",
                Confidence::High,
            )),
            [(best, _), rest @ ..] => Some(Suggested::new(
                best.rel.clone(),
                format!(
                    "top-level executable matching the folder name; {} other candidate(s)",
                    rest.len()
                ),
                Confidence::Medium,
            )),
        }
    };
    let bin = main
        .as_ref()
        .and_then(|m| inspect_binary(&root.join(&m.value)))
        .unwrap_or_default();

    let platform = match bin.os {
        Some(os) => Suggested::new(
            os,
            "read from the main executable's header",
            Confidence::High,
        ),
        None if entries
            .iter()
            .any(|e| e.lower.ends_with(".exe") || e.lower.ends_with(".dll")) =>
        {
            Suggested::new(Os::Windows, ".exe/.dll files present", Confidence::Medium)
        }
        None => Suggested::new(Os::Linux, "no Windows binaries found", Confidence::Low),
    };
    let arch = match bin.arch {
        Some(a) => Suggested::new(
            a,
            "read from the main executable's header",
            Confidence::High,
        ),
        None => Suggested::new(Arch::X64, "default", Confidence::Low),
    };
    let name = match (&bin.product_name, &bin.file_description) {
        (Some(n), _) => Suggested::new(n.clone(), "ProductName version resource", Confidence::High),
        (None, Some(d)) => Suggested::new(
            d.clone(),
            "FileDescription version resource",
            Confidence::Medium,
        ),
        _ => Suggested::new(dir_name, "folder name", Confidence::Low),
    };
    let version = match bin.version.as_deref().and_then(clean_version) {
        Some(v) => Suggested::new(
            v,
            "version resource of the main executable",
            Confidence::High,
        ),
        None => Suggested::new(
            "1.0.0".to_owned(),
            "no version information found; please set it",
            Confidence::Low,
        ),
    };
    let publisher = match &bin.company {
        Some(c) => Suggested::new(c.clone(), "CompanyName version resource", Confidence::High),
        None => Suggested::new(
            String::new(),
            "no publisher information found",
            Confidence::Low,
        ),
    };

    let mut hints = Vec::new();
    let has = |needle: &str| {
        entries
            .iter()
            .any(|e| e.lower.rsplit('/').next() == Some(needle))
    };
    let has_prefix = |prefix: &str| {
        entries.iter().any(|e| {
            e.lower
                .rsplit('/')
                .next()
                .is_some_and(|n| n.starts_with(prefix))
        })
    };
    let file_named = |needle: &str| {
        entries
            .iter()
            .find(|e| e.lower.rsplit('/').next() == Some(needle))
            .map(|e| e.rel.clone())
    };

    // .NET: runtimeconfig.json tells the framework and version exactly.
    for e in entries
        .iter()
        .filter(|e| e.lower.ends_with(".runtimeconfig.json"))
    {
        let Ok(text) = fs::read_to_string(root.join(&e.rel)) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let opts = &json["runtimeOptions"];
        let mut frameworks: Vec<&serde_json::Value> = Vec::new();
        if opts["framework"].is_object() {
            frameworks.push(&opts["framework"]);
        }
        if let Some(list) = opts["frameworks"].as_array() {
            frameworks.extend(list.iter());
        }
        if frameworks.is_empty() && (has("coreclr.dll") || has("libcoreclr.so")) {
            hints.push(Hint {
                kind: HintKind::Bundled {
                    what: ".NET runtime (self-contained)".into(),
                },
                reason: format!(
                    "{} has no framework reference and the runtime is shipped",
                    e.rel
                ),
                confidence: Confidence::High,
            });
        }
        for fw in frameworks {
            let (Some(name), Some(ver)) = (fw["name"].as_str(), fw["version"].as_str()) else {
                continue;
            };
            let id = match name {
                "Microsoft.WindowsDesktop.App" => "dotnet-desktop-runtime",
                "Microsoft.AspNetCore.App" => "aspnet-runtime",
                "Microsoft.NETCore.App" => "dotnet-runtime",
                _ => continue,
            };
            let major_minor: String = ver.split('.').take(2).collect::<Vec<_>>().join(".");
            hints.push(Hint {
                kind: HintKind::Prerequisite {
                    id: id.into(),
                    version: format!("^{major_minor}"),
                },
                reason: format!("{} requires {name} {ver}", e.rel),
                confidence: Confidence::High,
            });
        }
    }

    // Visual C++ runtime imported but not shipped app-local.
    let imports_lower: BTreeSet<String> = bin.imports.iter().map(|i| i.to_lowercase()).collect();
    for dll in ["vcruntime140.dll", "msvcp140.dll", "vcruntime140_1.dll"] {
        if imports_lower.contains(dll) {
            if has(dll) {
                hints.push(Hint {
                    kind: HintKind::Bundled {
                        what: "Visual C++ runtime (app-local)".into(),
                    },
                    reason: format!("{dll} is shipped with the application"),
                    confidence: Confidence::High,
                });
            } else {
                hints.push(Hint {
                    kind: HintKind::Prerequisite {
                        id: "vc-redist".into(),
                        version: "*".into(),
                    },
                    reason: format!(
                        "the main executable imports {} and it is not in the application folder",
                        dll.to_uppercase()
                    ),
                    confidence: Confidence::High,
                });
            }
            break;
        }
    }
    if has("webview2loader.dll") || has("microsoft.web.webview2.core.dll") {
        hints.push(Hint {
            kind: HintKind::Prerequisite {
                id: "webview2".into(),
                version: "*".into(),
            },
            reason: "WebView2 loader found".into(),
            confidence: Confidence::High,
        });
    }

    // Databases.
    let db_rules: &[(&[&str], &'static str, Confidence)] = &[
        (
            &["libpq.dll", "libpq.so", "libpq.so.5"],
            "PostgreSQL",
            Confidence::High,
        ),
        (&["npgsql.dll"], "PostgreSQL", Confidence::Medium),
        (
            &[
                "libmysql.dll",
                "libmysqlclient.so",
                "libmariadb.dll",
                "libmariadb.so.3",
            ],
            "MySQL / MariaDB",
            Confidence::High,
        ),
        (
            &["mysql.data.dll", "mysqlconnector.dll"],
            "MySQL / MariaDB",
            Confidence::Medium,
        ),
        (
            &[
                "sqlite3.dll",
                "e_sqlite3.dll",
                "libsqlite3.so",
                "system.data.sqlite.dll",
                "microsoft.data.sqlite.dll",
            ],
            "SQLite",
            Confidence::High,
        ),
        (
            &["microsoft.data.sqlclient.dll", "system.data.sqlclient.dll"],
            "SQL Server",
            Confidence::Medium,
        ),
    ];
    for (files, provider, confidence) in db_rules {
        let found: Vec<&str> = files
            .iter()
            .copied()
            .filter(|f| has(f) || imports_lower.contains(*f))
            .collect();
        if let Some(first) = found.first() {
            if hints
                .iter()
                .any(|h| h.kind == HintKind::Database { provider })
            {
                continue;
            }
            hints.push(Hint {
                kind: HintKind::Database { provider },
                reason: format!("{first} found (client library; the server may be remote)"),
                confidence: *confidence,
            });
        }
    }

    // Other runtimes.
    let has_ext = |ext: &str| entries.iter().any(|e| e.lower.ends_with(ext));
    if has_ext(".jar")
        && !entries
            .iter()
            .any(|e| e.lower.ends_with("bin/java") || e.lower.ends_with("bin/java.exe"))
    {
        hints.push(Hint {
            kind: HintKind::Prerequisite {
                id: "java-runtime".into(),
                version: "*".into(),
            },
            reason: ".jar files found and no bundled Java runtime".into(),
            confidence: Confidence::Medium,
        });
    }
    if has("package.json") && !has("node.exe") && !has("node") {
        hints.push(Hint {
            kind: HintKind::Prerequisite {
                id: "nodejs".into(),
                version: "*".into(),
            },
            reason: "package.json found and no bundled Node.js".into(),
            confidence: Confidence::Medium,
        });
    }
    if has_ext(".py") && !has_prefix("python3") && !has("python.exe") {
        hints.push(Hint {
            kind: HintKind::Prerequisite {
                id: "python".into(),
                version: "*".into(),
            },
            reason: ".py files found and no bundled Python".into(),
            confidence: Confidence::Low,
        });
    }

    // Network.
    let net_imports = [
        "winhttp.dll",
        "wininet.dll",
        "ws2_32.dll",
        "libcurl.so.4",
        "libssl.so.3",
    ];
    if let Some(i) = net_imports.iter().find(|i| imports_lower.contains(**i)) {
        hints.push(Hint {
            kind: HintKind::Network,
            reason: format!("the main executable imports {i}"),
            confidence: Confidence::Medium,
        });
    } else if has("system.net.http.dll") || has("libcurl.dll") {
        hints.push(Hint {
            kind: HintKind::Network,
            reason: "HTTP client library found".into(),
            confidence: Confidence::Low,
        });
    }

    // Services.
    if has("microsoft.extensions.hosting.windowsservices.dll") {
        hints.push(Hint {
            kind: HintKind::ServiceExecutable {
                path: main.as_ref().map(|m| m.value.clone()).unwrap_or_default(),
            },
            reason: "Microsoft.Extensions.Hosting.WindowsServices is referenced".into(),
            confidence: Confidence::High,
        });
    }
    for e in entries.iter().filter(|e| is_binary_candidate(e)) {
        let n = e.lower.rsplit('/').next().unwrap_or(&e.lower);
        if ["service", "svc", "daemon", "worker", "agent"]
            .iter()
            .any(|k| n.contains(k))
        {
            hints.push(Hint {
                kind: HintKind::ServiceExecutable {
                    path: e.rel.clone(),
                },
                reason: format!("executable name '{n}' suggests a background service"),
                confidence: Confidence::Low,
            });
        }
    }

    // Configuration files (top level).
    for e in entries.iter().filter(|e| !e.rel.contains('/')) {
        if [
            ".json", ".config", ".ini", ".yaml", ".yml", ".toml", ".conf", ".env",
        ]
        .iter()
        .any(|x| e.lower.ends_with(x))
            && !e.lower.ends_with(".deps.json")
            && !e.lower.ends_with(".runtimeconfig.json")
        {
            hints.push(Hint {
                kind: HintKind::ConfigFile {
                    path: e.rel.clone(),
                },
                reason: "configuration file at the application root".into(),
                confidence: Confidence::Medium,
            });
        }
    }
    let _ = file_named;

    Ok(Analysis {
        root,
        platform,
        arch,
        name,
        version,
        publisher,
        description: bin.file_description.clone(),
        main_executable: main,
        icon_ico: bin.icon,
        file_count: entries.len(),
        total_size,
        runtime_dependencies: bin.imports,
        hints,
    })
}

/// Builds a new project from an analysis. `project_dir` is where the project
/// file will live (the source path is stored relative to it).
pub fn to_project(a: &Analysis, project_dir: &Path) -> inst_model::Project {
    let version = inst_model::Version::parse(&a.version.value)
        .unwrap_or_else(|_| inst_model::Version::parse("1.0.0").expect("valid"));
    let source = relative_path(project_dir, &a.root);
    let mut p = inst_model::Project::new(&a.name.value, &a.publisher.value, version, source);
    p.application.main_executable = a.main_executable.as_ref().map(|m| m.value.clone());
    if let Some(d) = &a.description {
        p.product.description = inst_model::LocalizedText::plain(d.clone());
    }
    p.targets.windows_x64 = a.platform.value == Os::Windows && a.arch.value == Arch::X64;
    p.targets.windows_arm64 = a.platform.value == Os::Windows && a.arch.value == Arch::Arm64;
    p.targets.linux_x64 = a.platform.value == Os::Linux && a.arch.value == Arch::X64;
    p.targets.linux_arm64 = a.platform.value == Os::Linux && a.arch.value == Arch::Arm64;
    for h in &a.hints {
        if let HintKind::Prerequisite { id, version } = &h.kind
            && h.confidence >= Confidence::Medium
            && !p.prerequisites.iter().any(|x| &x.id == id)
        {
            p.prerequisites.push(inst_model::project::PrerequisiteRef {
                id: id.clone(),
                version: inst_model::VersionReq::parse(version).unwrap_or_default(),
                acquisition: inst_model::project::Acquisition::Automatic,
                custom_sources: Vec::new(),
                condition: None,
                suggested_reason: Some(format!("{} (confidence: {})", h.reason, h.confidence)),
            });
        }
    }
    p
}

/// `target` relative to `base` when possible (`../dist`), else absolute.
pub fn relative_path(base: &Path, target: &Path) -> PathBuf {
    let (Ok(base), Ok(target)) = (base.canonicalize(), target.canonicalize()) else {
        return target.to_path_buf();
    };
    let b: Vec<_> = base.components().collect();
    let t: Vec<_> = target.components().collect();
    let common = b.iter().zip(&t).take_while(|(x, y)| x == y).count();
    if common == 0 {
        return target;
    }
    let mut out = PathBuf::new();
    for _ in common..b.len() {
        out.push("..");
    }
    for c in &t[common..] {
        out.push(c);
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

impl fmt::Display for Analysis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let line =
            |f: &mut fmt::Formatter<'_>, label: &str, value: &str, reason: &str, c: Confidence| {
                writeln!(
                    f,
                    "{label:<18} {value}\n{:<18} reason: {reason} · confidence: {c}",
                    ""
                )
            };
        writeln!(f, "Application folder {}", self.root.display())?;
        line(
            f,
            "Name",
            &self.name.value,
            &self.name.reason,
            self.name.confidence,
        )?;
        line(
            f,
            "Version",
            &self.version.value,
            &self.version.reason,
            self.version.confidence,
        )?;
        line(
            f,
            "Publisher",
            &self.publisher.value,
            &self.publisher.reason,
            self.publisher.confidence,
        )?;
        line(
            f,
            "Platform",
            self.platform.value.name(),
            &self.platform.reason,
            self.platform.confidence,
        )?;
        line(
            f,
            "Architecture",
            &format!("{:?}", self.arch.value),
            &self.arch.reason,
            self.arch.confidence,
        )?;
        match &self.main_executable {
            Some(m) => line(f, "Main executable", &m.value, &m.reason, m.confidence)?,
            None => writeln!(f, "{:<18} none found", "Main executable")?,
        }
        writeln!(
            f,
            "{:<18} {}",
            "Icon",
            if self.icon_ico.is_some() {
                "extracted from the executable"
            } else {
                "not found"
            }
        )?;
        writeln!(
            f,
            "{:<18} {} files, {}",
            "Size",
            self.file_count,
            inst_fsx::format_bytes(self.total_size)
        )?;
        if !self.runtime_dependencies.is_empty() {
            writeln!(
                f,
                "{:<18} {}",
                "Dependencies",
                self.runtime_dependencies.join(", ")
            )?;
        }
        if !self.hints.is_empty() {
            writeln!(f, "\nSuggestions")?;
            for h in &self.hints {
                let what = match &h.kind {
                    HintKind::Prerequisite { id, version } => {
                        format!("Prerequisite {id} {version}")
                    }
                    HintKind::Database { provider } => format!("{provider} usage detected"),
                    HintKind::Network => "Network usage".into(),
                    HintKind::ServiceExecutable { path } => {
                        format!("Possible service executable {path}")
                    }
                    HintKind::ConfigFile { path } => format!("Configuration file {path}"),
                    HintKind::Bundled { what } => format!("Bundled: {what}"),
                };
                writeln!(
                    f,
                    "  {what}\n    Reason: {}\n    Confidence: {}",
                    h.reason, h.confidence
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, content: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        fs::write(p, content).expect("write");
    }

    #[test]
    fn analyzes_a_dotnet_style_app_folder() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().join("Acme Orders");
        write(&root, "AcmeOrders.exe", b"MZ not really a pe");
        write(&root, "AcmeOrders.runtimeconfig.json", br#"{"runtimeOptions":{"framework":{"name":"Microsoft.WindowsDesktop.App","version":"8.0.0"}}}"#);
        write(&root, "libpq.dll", b"x");
        write(&root, "appsettings.json", b"{}");
        write(&root, "tools/unins000.exe", b"MZ");
        write(&root, "OrdersWorkerService.exe", b"MZ");
        let a = analyze(&root).expect("analyze");
        assert_eq!(
            a.main_executable.as_ref().map(|m| m.value.as_str()),
            Some("AcmeOrders.exe")
        );
        assert_eq!(a.platform.value, Os::Windows);
        assert_eq!(a.file_count, 6);
        let has = |pred: &dyn Fn(&Hint) -> bool| a.hints.iter().any(pred);
        assert!(has(&|h| h.kind
            == HintKind::Prerequisite {
                id: "dotnet-desktop-runtime".into(),
                version: "^8.0".into()
            }
            && h.confidence == Confidence::High));
        assert!(has(&|h| h.kind
            == HintKind::Database {
                provider: "PostgreSQL"
            }
            && h.reason.contains("libpq.dll")));
        assert!(has(
            &|h| matches!(&h.kind, HintKind::ConfigFile { path } if path == "appsettings.json")
        ));
        assert!(has(
            &|h| matches!(&h.kind, HintKind::ServiceExecutable { path } if path == "OrdersWorkerService.exe")
        ));
        let text = a.to_string();
        assert!(text.contains("Reason:") && text.contains("Confidence: High"));

        let p = to_project(&a, tmp.path());
        assert_eq!(p.application.source, PathBuf::from("Acme Orders"));
        assert_eq!(p.prerequisites[0].id, "dotnet-desktop-runtime");
        assert!(p.targets.windows_x64 && !p.targets.linux_x64);
    }

    #[test]
    fn reads_real_elf_headers() {
        // The test binary itself is an ELF (on Linux) with NEEDED entries.
        let exe = std::env::current_exe().expect("exe");
        if cfg!(target_os = "linux") {
            let info = inspect_binary(&exe).expect("elf");
            assert_eq!(info.os, Some(Os::Linux));
            assert!(
                info.imports.iter().any(|l| l.starts_with("libc.so")),
                "{:?}",
                info.imports
            );
        }
    }

    #[test]
    fn versions_are_cleaned() {
        assert_eq!(clean_version("1.4.0.0").as_deref(), Some("1.4.0"));
        assert_eq!(clean_version("2, 0, 1, 7").as_deref(), Some("2.0.1.7"));
        assert_eq!(clean_version("3.1.0 (x64)").as_deref(), Some("3.1.0"));
    }
}
