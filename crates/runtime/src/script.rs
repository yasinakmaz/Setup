//! Script and program actions.
//!
//! Security rules:
//! * values are passed as separate argv entries or environment variables —
//!   never concatenated into a command line or script text;
//! * interpreters are started by absolute path (no PATH / DLL search
//!   hijacking of `powershell.exe`);
//! * embedded scripts are written to a private, exclusively created
//!   temporary directory;
//! * captured output is logged through the redactor.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use inst_log::{Logger, log};

use crate::context::{Common, Install, Uninstall};
use crate::error::{ErrorKind, InstallError};
use crate::spec::{OnFailure, RunProgram, Script, ScriptSource, Shell, Value};

/// A private temporary directory removed on drop.
struct PrivateTemp(PathBuf);

impl PrivateTemp {
    fn create() -> std::io::Result<PrivateTemp> {
        let base = std::env::temp_dir();
        loop {
            let dir = base.join(format!(
                "inst-script-{:x}",
                inst_fsx::atomic::unique_token()
            ));
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            match builder.create(&dir) {
                Ok(()) => return Ok(PrivateTemp(dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

impl Drop for PrivateTemp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn powershell() -> PathBuf {
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot")
            .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let p = root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        if p.exists() {
            return p;
        }
    }
    crate::detect::find_on_path("pwsh").unwrap_or_else(|| PathBuf::from("pwsh"))
}

struct Outcome {
    code: Option<i32>,
    timed_out: bool,
    tail: Vec<String>,
}

/// Runs a prepared command, streaming captured output to the log.
fn execute(
    mut cmd: Command,
    name: &str,
    timeout: Duration,
    capture_out: bool,
    capture_err: bool,
    logger: &Logger,
) -> std::io::Result<Outcome> {
    cmd.stdin(Stdio::null())
        .stdout(if capture_out {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(if capture_err {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    // Own process group so a timeout can stop everything the script spawned.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.spawn()?;
    let tail = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let mut readers = Vec::new();
    for (stream, is_err) in [
        (
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            false,
        ),
        (
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            true,
        ),
    ] {
        let Some(stream) = stream else { continue };
        let logger = logger.clone();
        let name = name.to_owned();
        let tail = tail.clone();
        readers.push(std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if is_err {
                    log!(logger, Warn, "script", "{name}: {line}");
                    let mut t = tail.lock().unwrap_or_else(|p| p.into_inner());
                    if t.len() == 20 {
                        t.remove(0);
                    }
                    t.push(logger.redactor().scrubbed(&line));
                } else {
                    log!(logger, Info, "script", "{name}: {line}");
                }
            }
        }));
    }
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if started.elapsed() > timeout {
            timed_out = true;
            kill_tree(&mut child);
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    if timed_out {
        // Descendants may still hold the pipes; never wait for them.
        drop(readers);
    } else {
        for r in readers {
            let _ = r.join();
        }
    }
    let tail = std::mem::take(&mut *tail.lock().unwrap_or_else(|p| p.into_inner()));
    Ok(Outcome {
        code: status.and_then(|s| s.code()),
        timed_out,
        tail,
    })
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn kill_tree(child: &mut std::process::Child) {
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: signals the process group we created for this child.
        unsafe { libc::kill(-pid, libc::SIGKILL) };
    }
    let _ = child.kill();
}

#[cfg(not(unix))]
fn kill_tree(child: &mut std::process::Child) {
    let _ = child.kill();
}

fn build_script_command(
    c: &Common<'_>,
    script: &Script,
    resolve: &dyn Fn(&Value) -> Result<String, InstallError>,
    temp: &mut Option<PrivateTemp>,
) -> Result<Command, InstallError> {
    let path: PathBuf = match script.source {
        ScriptSource::Embedded(code) => {
            let dir =
                PrivateTemp::create().map_err(|e| InstallError::io("preparing a script", &e))?;
            let ext = match script.shell {
                Shell::PowerShell => "ps1",
                Shell::Sh => "sh",
                Shell::Custom { .. } => "script",
            };
            let file = dir.0.join(format!("{}.{ext}", script.id));
            // PowerShell reads BOM-less files as ANSI on Windows PowerShell 5.
            let mut bytes = Vec::with_capacity(code.len() + 3);
            if matches!(script.shell, Shell::PowerShell) {
                bytes.extend_from_slice(b"\xEF\xBB\xBF");
            }
            bytes.extend_from_slice(code.as_bytes());
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)
                .map_err(|e| InstallError::io("preparing a script", &e))?;
            std::io::Write::write_all(&mut f, &bytes)
                .map_err(|e| InstallError::io("preparing a script", &e))?;
            *temp = Some(dir);
            file
        }
        ScriptSource::Payload(rel) => inst_fsx::RelPath::new(rel)
            .map_err(|e| InstallError::unexpected(e.to_string()))?
            .to_path_under(&c.install_dir),
    };
    let mut cmd = match script.shell {
        Shell::PowerShell => {
            let mut cmd = Command::new(powershell());
            cmd.args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ]);
            cmd.arg(&path);
            cmd
        }
        Shell::Sh => {
            let mut cmd = Command::new("/bin/sh");
            cmd.arg(&path);
            cmd
        }
        Shell::Custom { program, args } => {
            let mut cmd = Command::new(program);
            cmd.args(args).arg(&path);
            cmd
        }
    };
    for a in script.args {
        cmd.arg(resolve(a)?);
    }
    for (k, v) in script.env {
        cmd.env(k, resolve(v)?);
    }
    set_standard_env(&mut cmd, c);
    let wd = match script.working_dir {
        Some((folder, rel)) => c.folder(folder, rel),
        None => c.install_dir.clone(),
    };
    if wd.is_dir() {
        cmd.current_dir(wd);
    }
    Ok(cmd)
}

fn set_standard_env(cmd: &mut Command, c: &Common<'_>) {
    cmd.env("INST_INSTALL_DIR", &c.install_dir)
        .env("INST_PRODUCT_ID", c.app.product.id)
        .env("INST_PRODUCT_NAME", c.app.product.name)
        .env("INST_PRODUCT_VERSION", c.app.product.version)
        .env("INST_LANGUAGE", c.language.code());
}

fn finish(
    c: &Common<'_>,
    name: &str,
    on_failure: OnFailure,
    allowed: &[i32],
    result: std::io::Result<Outcome>,
) -> Result<(), InstallError> {
    let failure = match result {
        Err(e) => Some(format!("could not start: {e}")),
        Ok(o) if o.timed_out => Some(format!("timed out; last output: {}", o.tail.join(" | "))),
        Ok(o) => match o.code {
            Some(code) if allowed.contains(&code) => None,
            code => Some(format!(
                "exit code {}; last output: {}",
                code.map_or_else(|| "none (killed)".into(), |c| c.to_string()),
                o.tail.join(" | ")
            )),
        },
    };
    let Some(details) = failure else {
        log!(c.logger, Info, "script", "task succeeded"; "task" => name);
        return Ok(());
    };
    let details = c.logger.redactor().scrubbed(&details);
    match on_failure {
        OnFailure::Continue => {
            log!(c.logger, Warn, "script", "task failed; continuing"; "task" => name, "details" => &details);
            Ok(())
        }
        // `Ask` would need a UI round-trip from the worker thread; until the
        // frontend protocol supports it, it behaves like `Rollback`.
        OnFailure::Rollback | OnFailure::Ask => Err(InstallError::new(
            ErrorKind::TaskFailed {
                name: name.to_owned(),
            },
            details,
        )),
    }
}

fn run_script_in(
    c: &Common<'_>,
    script: &Script,
    resolve: &dyn Fn(&Value) -> Result<String, InstallError>,
) -> Result<(), InstallError> {
    let label = if script.name.is_empty() {
        script.id
    } else {
        script.name
    };
    log!(c.logger, Info, "script", "running task"; "task" => label);
    let mut temp = None;
    let cmd = match build_script_command(c, script, resolve, &mut temp) {
        Ok(cmd) => cmd,
        Err(e) if script.on_failure == OnFailure::Continue => {
            log!(c.logger, Warn, "script", "task skipped"; "task" => label, "error" => &e.to_string());
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    let timeout = Duration::from_secs(u64::from(script.timeout_secs.max(1)));
    let result = execute(
        cmd,
        label,
        timeout,
        script.capture_stdout,
        script.capture_stderr,
        &c.logger,
    );
    drop(temp);
    finish(
        c,
        label,
        script.on_failure,
        script.allowed_exit_codes,
        result,
    )
}

fn run_program_in(
    c: &Common<'_>,
    p: &RunProgram,
    resolve: &dyn Fn(&Value) -> Result<String, InstallError>,
) -> Result<(), InstallError> {
    let label = if p.name.is_empty() { p.id } else { p.name };
    let program = inst_fsx::RelPath::new(p.program)
        .map_err(|e| InstallError::unexpected(e.to_string()))?
        .to_path_under(&c.install_dir);
    let mut args = Vec::with_capacity(p.args.len());
    for a in p.args {
        args.push(resolve(a)?);
    }
    if !p.wait {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        return crate::platform::launch_detached(&program, &refs, &c.install_dir)
            .map_err(|e| InstallError::io(&format!("starting {label}"), &e));
    }
    let mut cmd = Command::new(&program);
    cmd.args(&args).current_dir(&c.install_dir);
    set_standard_env(&mut cmd, c);
    let result = execute(
        cmd,
        label,
        Duration::from_secs(u64::from(p.timeout_secs.max(1))),
        true,
        true,
        &c.logger,
    );
    finish(c, label, p.on_failure, p.allowed_exit_codes, result)
}

impl Install<'_> {
    pub fn run_script(&mut self, script: &Script) -> Result<(), InstallError> {
        let resolve = |v: &Value| self.resolve(v);
        run_script_in(&self.c, script, &resolve)
    }

    pub fn run_program(&mut self, p: &RunProgram) -> Result<(), InstallError> {
        let resolve = |v: &Value| self.resolve(v);
        run_program_in(&self.c, p, &resolve)
    }
}

impl Uninstall<'_> {
    pub fn run_script(&mut self, script: &Script) -> Result<(), InstallError> {
        let resolve = |v: &Value| self.resolve(v);
        run_script_in(&self.c, script, &resolve)
    }

    pub fn run_program(&mut self, p: &RunProgram) -> Result<(), InstallError> {
        let resolve = |v: &Value| self.resolve(v);
        run_program_in(&self.c, p, &resolve)
    }
}

/// Used by tests to run a command with the same machinery.
#[doc(hidden)]
pub fn run_for_test(program: &Path, args: &[&str], timeout: Duration) -> (Option<i32>, bool) {
    let mut cmd = Command::new(program);
    cmd.args(args);
    match execute(cmd, "test", timeout, true, true, &Logger::disabled()) {
        Ok(o) => (o.code, o.timed_out),
        Err(_) => (None, false),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn enforces_timeout() {
        let started = Instant::now();
        let (code, timed_out) = run_for_test(
            Path::new("/bin/sh"),
            &["-c", "sleep 5"],
            Duration::from_millis(200),
        );
        assert!(timed_out);
        assert_eq!(code, None);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn reports_exit_codes() {
        let (code, timed_out) = run_for_test(
            Path::new("/bin/sh"),
            &["-c", "echo out; echo err >&2; exit 3"],
            Duration::from_secs(5),
        );
        assert_eq!(code, Some(3));
        assert!(!timed_out);
    }
}
