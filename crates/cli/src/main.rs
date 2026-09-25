//! Headless command line interface of the Studio: create, analyze, check and
//! build projects in scripts and CI.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use inst_builder::{BuildObserver, BuildRequest, Stage};
use inst_catalog::Catalog;
use inst_model::Target;

const USAGE: &str = "\
Usage:
  {cli} new <app-dir> [--out <project-file>]
  {cli} analyze <app-dir-or-executable>
  {cli} doctor <project-file>
  {cli} build <project-file> [--target windows-x64|linux-x64|windows-arm64|linux-arm64]...
                             [--out <dir>] [--no-gui] [--no-appimage]
                             [--opt smallest|small|fast] [--verbose]
  {cli} catalog [search]
  {cli} cache [stats|clear-unused|clear-all]
";

fn usage() -> String {
    USAGE.replace("{cli}", inst_brand::CLI_NAME)
}

fn load_project(path: &Path) -> Result<inst_model::Project, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    inst_model::io::from_toml(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn parse_target(s: &str) -> Result<Target, String> {
    Ok(match s {
        "windows-x64" => Target::WINDOWS_X64,
        "linux-x64" => Target::LINUX_X64,
        "windows-arm64" => Target::WINDOWS_ARM64,
        "linux-arm64" => Target::LINUX_ARM64,
        other => return Err(format!("unknown target {other:?}")),
    })
}

struct Console {
    verbose: bool,
}

impl BuildObserver for Console {
    fn stage(&self, target: Option<Target>, stage: Stage) {
        match target {
            Some(t) => eprintln!("» [{t}] {}", stage.label()),
            None => eprintln!("» {}", stage.label()),
        }
    }
    fn log(&self, line: &str) {
        if self.verbose
            || line.starts_with("error")
            || line.starts_with("warning[")
            || line.starts_with("  ")
            || line.contains(':') && !line.starts_with("   Compiling")
        {
            eprintln!("  {line}");
        }
    }
}

fn cmd_build(args: &[String]) -> Result<(), String> {
    let mut project_file = None;
    let mut targets = Vec::new();
    let mut out = None;
    let mut gui = true;
    let mut appimage = true;
    let mut opt = inst_codegen::OptProfile::Smallest;
    let mut verbose = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--target" => targets.push(parse_target(it.next().ok_or("--target needs a value")?)?),
            "--out" => out = Some(PathBuf::from(it.next().ok_or("--out needs a value")?)),
            "--no-gui" => gui = false,
            "--no-appimage" => appimage = false,
            "--verbose" | "-v" => verbose = true,
            "--opt" => {
                opt = match it.next().map(String::as_str) {
                    Some("smallest") => inst_codegen::OptProfile::Smallest,
                    Some("small") => inst_codegen::OptProfile::Small,
                    Some("fast") => inst_codegen::OptProfile::Fast,
                    other => return Err(format!("invalid --opt {other:?}")),
                }
            }
            p if project_file.is_none() && !p.starts_with('-') => {
                project_file = Some(PathBuf::from(p))
            }
            other => return Err(format!("unexpected argument {other:?}")),
        }
    }
    let project_file = project_file.ok_or("missing project file")?;
    let project_file = project_file
        .canonicalize()
        .map_err(|e| format!("{}: {e}", project_file.display()))?;
    let project = load_project(&project_file)?;
    let mut req = BuildRequest::new(project_file, project);
    req.targets = targets;
    req.gui = gui;
    req.appimage = appimage;
    req.opt_profile = opt;
    if let Some(out) = out {
        req.output_dir = out;
    }
    let catalog = Catalog::with_user_dir(&user_catalog_dir()).map_err(|e| e.to_string())?;
    let reports =
        inst_builder::build(&req, &catalog, &Console { verbose }).map_err(|e| e.to_string())?;
    for r in reports {
        println!("\n{r}");
    }
    Ok(())
}

fn user_catalog_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".config")))
    };
    base.unwrap_or_default()
        .join(inst_brand::STUDIO_DIR_NAME)
        .join("catalog")
}

fn cmd_catalog(args: &[String]) -> Result<(), String> {
    let catalog = Catalog::with_user_dir(&user_catalog_dir()).map_err(|e| e.to_string())?;
    let query = args.first().map_or("", String::as_str);
    for item in catalog.search(query) {
        println!(
            "{:<24} {} — {} [{}]",
            item.id,
            item.name,
            item.vendor,
            item.channels.join(", ")
        );
        for p in &item.packages {
            let red = match inst_catalog::redundancy(p) {
                inst_catalog::Redundancy::Full { routes } => {
                    format!("{routes} independent sources")
                }
                inst_catalog::Redundancy::Degraded { routes } => {
                    format!("source redundancy degraded ({routes})")
                }
                inst_catalog::Redundancy::Unavailable => "embed or custom source only".into(),
            };
            println!("    {} {:?}: {red}", p.os.name(), p.arch);
        }
    }
    Ok(())
}

fn cmd_cache(args: &[String]) -> Result<(), String> {
    let cache = inst_fetch::Cache::open(inst_builder::prereqs::studio_cache_dir().join("cache"))
        .map_err(|e| e.to_string())?;
    match args.first().map_or("stats", String::as_str) {
        "stats" => {
            let s = cache.stats().map_err(|e| e.to_string())?;
            println!("{}", cache.root().display());
            println!(
                "{} entries, {}, partial downloads {}",
                s.entries,
                inst_fsx::format_bytes(s.bytes),
                inst_fsx::format_bytes(s.partial_bytes)
            );
            for e in cache.entries(true).map_err(|e| e.to_string())? {
                println!(
                    "  {} {} {} {:?} {}",
                    e.info.product,
                    e.info.version,
                    inst_fsx::format_bytes(e.size),
                    e.integrity,
                    e.hash
                );
            }
        }
        "clear-unused" => {
            let freed = cache
                .clear_unused(Duration::from_secs(30 * 86_400))
                .map_err(|e| e.to_string())?;
            println!("freed {}", inst_fsx::format_bytes(freed));
        }
        "clear-all" => {
            let freed = cache.clear_all().map_err(|e| e.to_string())?;
            println!("freed {}", inst_fsx::format_bytes(freed));
        }
        other => return Err(format!("unknown cache command {other:?}")),
    }
    Ok(())
}

fn cmd_doctor(args: &[String]) -> Result<(), String> {
    let path = PathBuf::from(args.first().ok_or("missing project file")?);
    let project = load_project(&path)?;
    let catalog = Catalog::with_user_dir(&user_catalog_dir()).map_err(|e| e.to_string())?;
    let report = inst_doctor::examine(&project, &path, &catalog);
    for f in &report.findings {
        println!("{f}");
    }
    println!(
        "\n{} error(s), {} warning(s), {} other finding(s)",
        report.errors(),
        report.warnings(),
        report.findings.len() - report.errors() - report.warnings()
    );
    if report.errors() > 0 {
        Err("the project has errors".into())
    } else {
        Ok(())
    }
}

fn cmd_analyze(args: &[String]) -> Result<(), String> {
    let path = PathBuf::from(args.first().ok_or("missing path")?);
    let a = inst_analyzer::analyze(&path).map_err(|e| e.to_string())?;
    print!("{a}");
    Ok(())
}

fn cmd_new(args: &[String]) -> Result<(), String> {
    let app = PathBuf::from(args.first().ok_or("missing application directory")?);
    let out = match args.get(1).map(String::as_str) {
        Some("--out") => PathBuf::from(args.get(2).ok_or("--out needs a value")?),
        _ => PathBuf::from(format!("project.{}", inst_brand::PROJECT_EXTENSION)),
    };
    let analysis = inst_analyzer::analyze(&app).map_err(|e| e.to_string())?;
    let project_dir = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let project = inst_analyzer::to_project(&analysis, project_dir);
    let text = inst_model::io::to_toml(&project).map_err(|e| e.to_string())?;
    inst_fsx::write_atomic(&out, text.as_bytes(), true).map_err(|e| e.to_string())?;
    print!("{analysis}");
    println!("\nProject written to {}", out.display());
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        eprint!("{}", usage());
        return ExitCode::from(2);
    };
    let rest = &args[1..];
    let result = match cmd.as_str() {
        "new" => cmd_new(rest),
        "analyze" => cmd_analyze(rest),
        "doctor" => cmd_doctor(rest),
        "build" => cmd_build(rest),
        "catalog" => cmd_catalog(rest),
        "cache" => cmd_cache(rest),
        "-h" | "--help" | "help" => {
            print!("{}", usage());
            Ok(())
        }
        other => Err(format!("unknown command {other:?}\n\n{}", usage())),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
