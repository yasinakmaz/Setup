//! Static action specifications instantiated by generated code.
//!
//! These mirror the project model's typed actions, reduced to `'static`
//! data. A generated installer contains e.g.
//! `static SCRIPT_0: Script = Script { shell: Shell::PowerShell, … };`
//! and calls `cx.run_script(&SCRIPT_0)` from its graph function.

use crate::platform::Folder;
use crate::services::StartMode;

/// A value resolved at install time.
#[derive(Clone, Copy, Debug)]
pub enum Value {
    Lit(&'static str),
    /// Non-secret installer input field.
    Input(&'static str),
    Secret(Secret),
    Path(Folder, &'static str),
    ProductVersion,
    ProductName,
    /// Value captured by an earlier action.
    Captured(&'static str),
}

#[derive(Clone, Copy, Debug)]
pub enum Secret {
    Input(&'static str),
    Env(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnFailure {
    Rollback,
    Continue,
    Ask,
}

#[derive(Clone, Copy, Debug)]
pub enum Shell {
    PowerShell,
    Sh,
    Custom {
        program: &'static str,
        args: &'static [&'static str],
    },
}

#[derive(Clone, Copy, Debug)]
pub enum ScriptSource {
    /// Script text compiled into the installer.
    Embedded(&'static str),
    /// Script file shipped in the application payload (install-relative).
    Payload(&'static str),
}

#[derive(Debug)]
pub struct Script {
    pub id: &'static str,
    pub name: &'static str,
    pub shell: Shell,
    pub source: ScriptSource,
    pub args: &'static [Value],
    pub env: &'static [(&'static str, Value)],
    pub working_dir: Option<(Folder, &'static str)>,
    pub timeout_secs: u32,
    pub allowed_exit_codes: &'static [i32],
    pub capture_stdout: bool,
    pub capture_stderr: bool,
    pub on_failure: OnFailure,
}

#[derive(Debug)]
pub struct Service {
    pub id: &'static str,
    pub name: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    /// Executable relative to the installation directory.
    pub executable: &'static str,
    pub args: &'static [Value],
    pub start: StartMode,
    pub user: bool,
    pub restart_on_failure: bool,
    pub start_after_install: bool,
    pub on_failure: OnFailure,
}

#[derive(Clone, Copy, Debug)]
pub enum RegData {
    Sz(Value),
    ExpandSz(Value),
    MultiSz(&'static [Value]),
    Dword(u32),
    Qword(u64),
}

#[derive(Debug)]
pub struct RegistryWrite {
    pub id: &'static str,
    /// [`crate::platform::HKCU`] or [`crate::platform::HKLM`].
    pub hive: u8,
    pub key: &'static str,
    pub name: &'static str,
    pub data: RegData,
    pub remove_on_uninstall: bool,
    pub on_failure: OnFailure,
}

#[derive(Clone, Copy, Debug)]
pub enum EnvOp {
    Set(Value),
    Append(Folder, &'static str),
    Prepend(Folder, &'static str),
}

#[derive(Debug)]
pub struct EnvWrite {
    pub id: &'static str,
    pub machine: bool,
    pub name: &'static str,
    pub op: EnvOp,
    pub on_failure: OnFailure,
}

#[derive(Debug)]
pub struct RunProgram {
    pub id: &'static str,
    pub name: &'static str,
    /// Program relative to the installation directory.
    pub program: &'static str,
    pub args: &'static [Value],
    pub wait: bool,
    pub timeout_secs: u32,
    pub allowed_exit_codes: &'static [i32],
    pub on_failure: OnFailure,
}
