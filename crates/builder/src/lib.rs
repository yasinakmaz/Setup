//! Installer compiler.
//!
//! ```text
//! project ─▶ validate ─▶ scan ─▶ compression analysis ─▶ resolve prerequisites
//!         ─▶ generate Rust ─▶ cargo build (shared target dir) ─▶ append payload
//!         ─▶ package (AppImage) ─▶ sign ─▶ self-verify ─▶ report
//! ```

pub mod appimage;
pub mod compress;
pub mod prereqs;
pub mod report;
pub mod resources;
pub mod scan;
pub mod sign;
pub mod toolchain;

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use inst_catalog::Catalog;
use inst_codegen::{CodegenInput, OptProfile, RuntimeSources};
use inst_fetch::Cache;
use inst_model::platform::{Os, Target};
use inst_model::project::Project;
use inst_model::validate::{self, Severity};
use inst_payload::CodecParams;
use inst_payload::format::Codec;
use inst_payload::write::{BlockPlan, InputFile, WriteObserver, WriteOptions};

pub use report::ArtifactReport;
pub use resources::Resources;

/// Pipeline stages, for progress display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Validate,
    Scan,
    Compression,
    Prerequisites,
    Generate,
    Compile,
    Payload,
    Package,
    Sign,
    Verify,
}

impl Stage {
    pub const fn label(self) -> &'static str {
        match self {
            Stage::Validate => "Validating project",
            Stage::Scan => "Scanning application",
            Stage::Compression => "Analyzing compression",
            Stage::Prerequisites => "Resolving prerequisites",
            Stage::Generate => "Generating installer code",
            Stage::Compile => "Compiling runtime",
            Stage::Payload => "Compressing payload",
            Stage::Package => "Packaging",
            Stage::Sign => "Signing",
            Stage::Verify => "Verifying output",
        }
    }
}

/// Progress, log and cancellation hooks. Called from worker threads.
pub trait BuildObserver: Sync {
    fn stage(&self, _target: Option<Target>, _stage: Stage) {}
    fn progress(&self, _done: u64, _total: u64) {}
    fn log(&self, _line: &str) {}
    fn cancelled(&self) -> bool {
        false
    }
}

pub struct NoopBuildObserver;
impl BuildObserver for NoopBuildObserver {}

#[derive(Debug)]
pub enum BuildError {
    Invalid(Vec<validate::Diagnostic>),
    Toolchain(String),
    Io(String, std::io::Error),
    Prerequisites(String),
    Codegen(inst_codegen::CodegenError),
    Compile(String),
    Payload(inst_payload::PayloadError),
    Package(String),
    Verify(String),
    Cancelled,
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::Invalid(d) => {
                writeln!(f, "the project has errors:")?;
                for diag in d.iter().filter(|d| d.severity == Severity::Error) {
                    writeln!(f, "  {diag}")?;
                }
                Ok(())
            }
            BuildError::Toolchain(e) => write!(f, "toolchain: {e}"),
            BuildError::Io(ctx, e) => write!(f, "{ctx}: {e}"),
            BuildError::Prerequisites(e) => write!(f, "prerequisites: {e}"),
            BuildError::Codegen(e) => write!(f, "code generation: {e}"),
            BuildError::Compile(e) => write!(f, "compilation failed:\n{e}"),
            BuildError::Payload(e) => write!(f, "payload: {e}"),
            BuildError::Package(e) => write!(f, "packaging: {e}"),
            BuildError::Verify(e) => write!(f, "output verification failed: {e}"),
            BuildError::Cancelled => f.write_str("build cancelled"),
        }
    }
}

impl std::error::Error for BuildError {}

fn io(ctx: impl Into<String>) -> impl FnOnce(std::io::Error) -> BuildError {
    let ctx = ctx.into();
    move |e| BuildError::Io(ctx, e)
}

#[derive(Clone, Debug)]
pub struct BuildRequest {
    pub project_file: PathBuf,
    pub project: Project,
    /// Empty = all enabled targets.
    pub targets: Vec<Target>,
    pub output_dir: PathBuf,
    /// Generated crates and the shared cargo target directory.
    pub work_dir: PathBuf,
    /// Include the graphical installer UI.
    pub gui: bool,
    pub opt_profile: OptProfile,
    /// Package Linux output as AppImage (otherwise a plain executable).
    pub appimage: bool,
    /// Leave one CPU for an interactive UI.
    pub reserve_ui: bool,
}

impl BuildRequest {
    pub fn new(project_file: PathBuf, project: Project) -> BuildRequest {
        let base = project_file
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        BuildRequest {
            output_dir: base.join("out"),
            work_dir: prereqs::studio_cache_dir().join("builds"),
            project_file,
            project,
            targets: Vec::new(),
            gui: true,
            opt_profile: OptProfile::Smallest,
            appimage: true,
            reserve_ui: false,
        }
    }
}

struct PayloadObserver<'a>(&'a dyn BuildObserver, std::sync::atomic::AtomicU64, u64);

impl WriteObserver for PayloadObserver<'_> {
    fn progress(&self, bytes: u64) {
        let done = self
            .1
            .fetch_add(bytes, std::sync::atomic::Ordering::Relaxed)
            + bytes;
        self.0.progress(done, self.2);
    }
    fn cancelled(&self) -> bool {
        self.0.cancelled()
    }
}

/// Writes `contents` only when it differs, so cargo's freshness checks
/// skip unchanged crates.
fn write_if_changed(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if fs::read(path).is_ok_and(|old| old == contents) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    inst_fsx::write_atomic(path, contents, false)
}

fn codec_feature(codec: Codec) -> Option<&'static str> {
    match codec {
        Codec::None => None,
        Codec::Zstd => Some("zstd"),
        Codec::Xz => Some("xz"),
    }
}

/// Builds installers for every requested target.
pub fn build(
    req: &BuildRequest,
    catalog: &Catalog,
    observer: &dyn BuildObserver,
) -> Result<Vec<ArtifactReport>, BuildError> {
    let project = &req.project;
    let res = Resources::detect();

    observer.stage(None, Stage::Validate);
    let diagnostics = validate::validate(project, catalog);
    for d in &diagnostics {
        observer.log(&d.to_string());
    }
    if validate::has_errors(&diagnostics) {
        return Err(BuildError::Invalid(diagnostics));
    }
    let targets = if req.targets.is_empty() {
        project.enabled_targets()
    } else {
        req.targets.clone()
    };
    let runtime = toolchain::RuntimeLocation::discover().ok_or_else(|| {
        BuildError::Toolchain("runtime sources not found (set INST_RUNTIME_SRC)".into())
    })?;
    fs::create_dir_all(&req.work_dir).map_err(io("creating the work directory"))?;
    let toolchain = toolchain::Toolchain::detect(&runtime.root).map_err(BuildError::Toolchain)?;

    // Scan once for all targets.
    let t0 = Instant::now();
    observer.stage(None, Stage::Scan);
    let source = project.source_dir(&req.project_file);
    let scan = scan::scan(&source, &project.application.exclude)
        .map_err(io(format!("scanning {}", source.display())))?;
    observer.log(&format!(
        "{} files, {} ({} excluded)",
        scan.files.len(),
        inst_fsx::format_bytes(scan.total_size),
        scan.excluded
    ));
    let scan_time = t0.elapsed();
    if let Some(main) = &project.application.main_executable
        && !scan.files.iter().any(|f| &f.rel == main)
    {
        return Err(BuildError::Invalid(vec![validate::Diagnostic {
            severity: Severity::Error,
            code: "B0001",
            location: "application.main-executable".into(),
            message: format!("{main} is not in the application folder (or is excluded)"),
        }]));
    }

    let cache = Cache::open(prereqs::studio_cache_dir().join("cache"))
        .map_err(io("opening the Studio cache"))?;
    let mut reports = Vec::with_capacity(targets.len());
    for target in targets {
        if observer.cancelled() {
            return Err(BuildError::Cancelled);
        }
        reports.push(build_target(
            req, catalog, observer, &res, &runtime, &toolchain, &scan, scan_time, &cache, target,
        )?);
    }
    Ok(reports)
}

#[allow(clippy::too_many_arguments)]
fn build_target(
    req: &BuildRequest,
    catalog: &Catalog,
    observer: &dyn BuildObserver,
    res: &Resources,
    runtime: &toolchain::RuntimeLocation,
    toolchain: &toolchain::Toolchain,
    scan: &scan::Scan,
    scan_time: Duration,
    cache: &Cache,
    target: Target,
) -> Result<ArtifactReport, BuildError> {
    let project = &req.project;
    let mut timings = vec![("Scan", scan_time)];
    let mut warnings = Vec::new();
    let triple = toolchain
        .triple_for(target)
        .map_err(BuildError::Toolchain)?;

    // Compression.
    let t = Instant::now();
    observer.stage(Some(target), Stage::Compression);
    let decision = compress::decide(
        project.compression.profile,
        &scan.files,
        target.arch,
        project.compression.block_size_mib,
    )
    .map_err(io("analyzing compression"))?;
    for c in &decision.candidates {
        observer.log(&format!(
            "  {:<12} ratio {:>5.1}%  encode {:>7.1} MiB/s  decode {:>7.1} MiB/s  est. {}",
            c.name,
            c.ratio() * 100.0,
            c.encode_mib_s,
            c.decode_mib_s,
            inst_fsx::format_bytes(c.estimated_total)
        ));
    }
    observer.log(&format!("compression: {}", decision.reason));
    timings.push(("Compression analysis", t.elapsed()));

    // Prerequisites.
    let t = Instant::now();
    observer.stage(Some(target), Stage::Prerequisites);
    let resolved = if project.prerequisites.is_empty() {
        Vec::new()
    } else {
        let fetcher = prereqs::NetFetcher::new(req.work_dir.join("tmp"))
            .map_err(BuildError::Prerequisites)?;
        prereqs::resolve_all(project, target, catalog, &fetcher, cache)
            .map_err(BuildError::Prerequisites)?
    };
    for r in &resolved {
        observer.log(&format!("prerequisite: {}", r.report));
    }
    timings.push(("Prerequisites", t.elapsed()));

    // Payload inputs.
    let mut inputs: Vec<InputFile> = scan
        .files
        .iter()
        .map(|f| InputFile {
            rel_path: f.rel.clone(),
            source: f.abs.clone(),
            size: f.size,
            executable: f.executable,
            component: 0,
        })
        .collect();
    let mut plans = compress::plan_blocks(&scan.files, &decision, res.cpu_workers(false).min(4));
    let embedded: Vec<usize> = resolved
        .iter()
        .filter_map(|r| r.embedded_file.as_ref())
        .map(|(rel, path, size)| {
            inputs.push(InputFile {
                rel_path: rel.clone(),
                source: path.clone(),
                size: *size,
                executable: false,
                component: inst_runtime_component::PREREQ_COMPONENT,
            });
            inputs.len() - 1
        })
        .collect();
    if !embedded.is_empty() {
        // Vendor installers are already compressed: store them.
        plans.push(BlockPlan {
            params: CodecParams::STORED,
            files: embedded,
        });
    }
    let codecs: Vec<Codec> = {
        let mut c: Vec<Codec> = plans.iter().map(|p| p.params.codec).collect();
        c.sort_by_key(|c| *c as u8);
        c.dedup();
        c
    };
    let codec_features: Vec<&'static str> =
        codecs.iter().filter_map(|c| codec_feature(*c)).collect();

    // Code generation.
    let t = Instant::now();
    observer.stage(Some(target), Stage::Generate);
    let analysis = inst_model::privileges::analyze(project, target, catalog);
    let (logo_png, windows_icon) = load_icons(req, project);
    if target.os == Os::Windows && windows_icon.is_none() {
        warnings.push("no .ico icon configured; the setup uses the default executable icon".into());
    }
    let runtime_sources = RuntimeSources {
        runtime: runtime.runtime(),
        runtime_ui: runtime.runtime_ui(),
    };
    let gui = req.gui && runtime_sources.runtime_ui.is_some();
    if req.gui && !gui {
        warnings
            .push("graphical installer UI is not available; building a console installer".into());
    }
    let project_scripts = load_project_scripts(req, project)?;
    let prereq_inputs: Vec<_> = resolved.iter().map(|r| r.input.clone()).collect();
    let installed_icon = project
        .product
        .icon
        .as_ref()
        .and_then(|i| i.file_name())
        .and_then(|n| n.to_str())
        .filter(|n| scan.files.iter().any(|f| f.rel == *n))
        .map(str::to_owned);
    let input = CodegenInput {
        project,
        target,
        privileges: &analysis,
        installed_size: scan.total_size,
        prerequisites: &prereq_inputs,
        runtime: &runtime_sources,
        codec_features: &codec_features,
        gui,
        profile: req.opt_profile,
        project_scripts: &project_scripts,
        logo_png,
        windows_icon,
        installed_icon,
    };
    let generated = inst_codegen::generate(&input).map_err(BuildError::Codegen)?;
    let crate_dir = req
        .work_dir
        .join("crates")
        .join(format!("{}-{}", project.product.id, triple));
    for f in &generated.files {
        write_if_changed(&crate_dir.join(f.path), &f.contents)
            .map_err(io("writing generated code"))?;
    }
    // Seed the lockfile from the runtime workspace on first use: cargo keeps
    // every pinned version and only adds the generated root package.
    if let Some(lock) = runtime.lockfile()
        && !crate_dir.join("Cargo.lock").exists()
    {
        fs::copy(&lock, crate_dir.join("Cargo.lock")).map_err(io("writing Cargo.lock"))?;
    }
    if let Some(tc) = runtime.toolchain_file() {
        let bytes = fs::read(&tc).map_err(io("reading rust-toolchain.toml"))?;
        write_if_changed(&crate_dir.join("rust-toolchain.toml"), &bytes)
            .map_err(io("writing rust-toolchain.toml"))?;
    }
    timings.push(("Code generation", t.elapsed()));

    // Compilation.
    let t = Instant::now();
    observer.stage(Some(target), Stage::Compile);
    let target_dir = req.work_dir.join("target");
    let exe = compile(
        &toolchain.cargo,
        &crate_dir,
        &target_dir,
        triple,
        target,
        res,
        req.reserve_ui,
        runtime,
        observer,
    )?;
    let runtime_size = fs::metadata(&exe)
        .map_err(io("reading the compiled runtime"))?
        .len();
    timings.push(("Compilation", t.elapsed()));

    // Payload.
    let t = Instant::now();
    observer.stage(Some(target), Stage::Payload);
    fs::create_dir_all(&req.output_dir).map_err(io("creating the output directory"))?;
    let stem = inst_model::platform::file_stem(&project.product.name);
    let final_name = target.artifact_name(&stem);
    let final_path = req.output_dir.join(&final_name);
    let staging = req.output_dir.join(format!(".{final_name}.staging"));
    let runtime_stub = if target.os == Os::Linux && req.appimage {
        req.output_dir.join(format!(".{final_name}.runtime"))
    } else {
        staging.clone()
    };
    fs::copy(&exe, &runtime_stub).map_err(io("copying the runtime"))?;
    let max_encoder_mem = plans
        .iter()
        .map(|p| compress::encoder_memory(&p.params))
        .max()
        .unwrap_or(0);
    let payload_tmp = req.work_dir.join("tmp");
    fs::create_dir_all(&payload_tmp).map_err(io("creating temp dir"))?;
    let opts = WriteOptions {
        threads: res.compression_workers(plans.len(), max_encoder_mem, req.reserve_ui),
        temp_dir: payload_tmp,
        buffer_size: 1 << 20,
    };
    let total_in: u64 = inputs.iter().map(|i| i.size).sum();
    let obs = PayloadObserver(observer, std::sync::atomic::AtomicU64::new(0), total_in);
    let payload_target = if target.os == Os::Linux && req.appimage {
        // AppImage: the payload is appended to the AppImage file itself.
        staging.with_extension("payload")
    } else {
        staging.clone()
    };
    if payload_target != staging {
        fs::File::create(&payload_target).map_err(io("creating payload file"))?;
    }
    let summary = inst_payload::write::append_to_executable(
        &payload_target,
        &inputs,
        &plans,
        &scan.empty_dirs,
        &opts,
        &obs,
    )
    .map_err(|e| match e {
        inst_payload::PayloadError::Cancelled => BuildError::Cancelled,
        e => BuildError::Payload(e),
    })?;
    timings.push(("Payload compression", t.elapsed()));

    // Packaging.
    let t = Instant::now();
    observer.stage(Some(target), Stage::Package);
    let packaging = if target.os == Os::Linux && req.appimage {
        match appimage::package(&appimage::AppImageInput {
            runtime_exe: &runtime_stub,
            payload: &payload_target,
            output: &staging,
            product_id: project.product.id.as_str(),
            name: &project.product.name,
            icon_png: input.logo_png.as_deref(),
            arch: target.arch,
            cache,
            work_dir: &req.work_dir,
        }) {
            Ok(()) => "AppImage".to_owned(),
            Err(e) => {
                warnings.push(format!(
                    "AppImage packaging unavailable ({e}); produced a self-contained executable"
                ));
                concat_files(&runtime_stub, &payload_target, &staging)
                    .map_err(io("writing the installer"))?;
                "executable".to_owned()
            }
        }
    } else {
        "executable".to_owned()
    };
    if runtime_stub != staging {
        let _ = fs::remove_file(&runtime_stub);
    }
    if payload_target != staging {
        let _ = fs::remove_file(&payload_target);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o755))
            .map_err(io("setting permissions"))?;
    }
    timings.push(("Packaging", t.elapsed()));

    // Signing.
    observer.stage(Some(target), Stage::Sign);
    let signing = if target.os == Os::Windows {
        sign::sign_windows(&staging, project.signing.windows.as_ref())
    } else {
        sign::SignStatus::NotConfigured
    };
    match &signing {
        sign::SignStatus::NotConfigured if target.os == Os::Windows => {
            if project.signing.require_signed {
                return Err(BuildError::Package(
                    "signing is required but not configured".into(),
                ));
            }
            warnings.push("unsigned Windows installer: SmartScreen will warn users".into());
        }
        sign::SignStatus::Failed(e) => {
            return Err(BuildError::Package(format!("signing failed: {e}")));
        }
        _ => {}
    }

    // Self-verification: the produced file must contain an intact payload.
    let t = Instant::now();
    observer.stage(Some(target), Stage::Verify);
    let integrity_verified = verify_artifact(&staging).map_err(BuildError::Verify)?;
    fs::rename(&staging, &final_path).map_err(io("finalizing the installer"))?;
    timings.push(("Verification", t.elapsed()));

    let exact_size = fs::metadata(&final_path)
        .map_err(io("reading the installer"))?
        .len();
    let payload_size = summary.payload_len;
    let peak = plans
        .iter()
        .map(|p| match p.params.codec {
            Codec::Zstd => 1u64 << p.params.window_log.unwrap_or(23).min(27),
            Codec::Xz => 64 << 20,
            Codec::None => 0,
        })
        .max()
        .unwrap_or(0)
        * 4
        + (4 << 20);
    let codec_names: Vec<String> = plans
        .iter()
        .map(|p| match p.params.codec {
            Codec::None => "stored".to_owned(),
            Codec::Zstd => format!("zstd-{}", p.params.level),
            Codec::Xz => format!(
                "xz-{}{}",
                p.params.level,
                if p.params.filter != inst_payload::Filter::None {
                    "+bcj"
                } else {
                    ""
                }
            ),
        })
        .fold(Vec::new(), |mut acc, n| {
            if !acc.contains(&n) {
                acc.push(n);
            }
            acc
        });
    Ok(ArtifactReport {
        target,
        triple: triple.to_owned(),
        path: final_path,
        exact_size,
        runtime_size,
        payload_size,
        original_size: total_in,
        file_count: inputs.len(),
        blocks: plans.len(),
        codecs: codec_names,
        compression_reason: decision.reason,
        features: generated.features,
        privileges: match analysis.effective {
            inst_model::privileges::Level::Admin => "Administrator / root required".into(),
            inst_model::privileges::Level::User => "Current user (no elevation)".into(),
        },
        privilege_reasons: analysis.reasons.iter().map(ToString::to_string).collect(),
        signing,
        prerequisites: resolved.iter().map(|r| r.report.clone()).collect(),
        integrity_verified,
        packaging,
        peak_extraction_memory: peak,
        timings,
        warnings,
    })
}

/// Component id reserved by the runtime for embedded prerequisites.
mod inst_runtime_component {
    pub const PREREQ_COMPONENT: u16 = u16::MAX;
}

fn concat_files(a: &Path, b: &Path, out: &Path) -> std::io::Result<()> {
    let mut o = fs::File::create(out)?;
    std::io::copy(&mut fs::File::open(a)?, &mut o)?;
    std::io::copy(&mut fs::File::open(b)?, &mut o)?;
    o.sync_all()
}

fn verify_artifact(path: &Path) -> Result<bool, String> {
    let mut f = fs::File::open(path).map_err(|e| e.to_string())?;
    let payload = inst_payload::Payload::locate(&mut f).map_err(|e| e.to_string())?;
    inst_payload::read::verify_blocks(&payload, &mut f, |_| {}).map_err(|e| e.to_string())?;
    Ok(true)
}

fn load_icons(req: &BuildRequest, project: &Project) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let base = req.project_file.parent().unwrap_or_else(|| Path::new("."));
    let Some(icon) = &project.product.icon else {
        return (None, None);
    };
    let path = if icon.is_absolute() {
        icon.clone()
    } else {
        base.join(icon)
    };
    let Ok(bytes) = fs::read(&path) else {
        return (None, None);
    };
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => (Some(bytes), None),
        Some("ico") => (None, Some(bytes)),
        _ => (None, None),
    }
}

fn load_project_scripts(
    req: &BuildRequest,
    project: &Project,
) -> Result<Vec<(String, String)>, BuildError> {
    let base = req.project_file.parent().unwrap_or_else(|| Path::new("."));
    let mut out = Vec::new();
    for a in &project.actions {
        if let inst_model::ActionKind::Script(s) = &a.kind
            && let inst_model::action::ScriptSource::Project { path } = &s.source
        {
            let text = fs::read_to_string(base.join(path))
                .map_err(io(format!("reading script {path}")))?;
            out.push((a.id.clone(), text));
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn compile(
    cargo: &Path,
    crate_dir: &Path,
    target_dir: &Path,
    triple: &str,
    target: Target,
    res: &Resources,
    reserve_ui: bool,
    runtime: &toolchain::RuntimeLocation,
    observer: &dyn BuildObserver,
) -> Result<PathBuf, BuildError> {
    // One compilation at a time per shared target directory.
    fs::create_dir_all(target_dir).map_err(io("creating the target directory"))?;
    let lock = fs::File::create(target_dir.join(".build.lock"))
        .map_err(io("locking the target directory"))?;
    lock.lock().map_err(io("locking the target directory"))?;
    let mut cmd = Command::new(cargo);
    cmd.current_dir(crate_dir)
        .args([
            "build",
            "--release",
            "--message-format=short",
            "--target",
            triple,
        ])
        .arg("-j")
        .arg(res.cpu_workers(reserve_ui).to_string())
        .env("CARGO_TARGET_DIR", target_dir)
        // A generic, reproducible binary: never target-cpu=native, no
        // developer paths embedded.
        .env_remove("RUSTFLAGS")
        .env(
            "CARGO_ENCODED_RUSTFLAGS",
            format!(
                "--remap-path-prefix={}=/runtime\x1f--remap-path-prefix={}=/build",
                runtime.root.display(),
                crate_dir.display()
            ),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| BuildError::Toolchain(format!("cannot start cargo: {e}")))?;
    let stderr = child.stderr.take().expect("piped");
    let mut errors = Vec::new();
    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
        if observer.cancelled() {
            let _ = child.kill();
            return Err(BuildError::Cancelled);
        }
        observer.log(&line);
        if line.starts_with("error") || line.contains(": error") || !errors.is_empty() {
            errors.push(line);
        }
    }
    let status = child
        .wait()
        .map_err(|e| BuildError::Compile(e.to_string()))?;
    if !status.success() {
        return Err(BuildError::Compile(errors.join("\n")));
    }
    let exe = target_dir
        .join(triple)
        .join("release")
        .join(if target.os == Os::Windows {
            "setup.exe"
        } else {
            "setup"
        });
    if !exe.is_file() {
        return Err(BuildError::Compile(format!(
            "{} was not produced",
            exe.display()
        )));
    }
    Ok(exe)
}
