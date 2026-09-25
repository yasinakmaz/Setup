//! Rust source emission.

use std::fmt::Write as _;

use inst_i18n::Language;
use inst_model::action::{
    Action, ActionKind, EnvOperation, FailurePolicy, LifecycleEvent, RegistryData, RegistryHive,
    ScriptSource, ServiceScope, ServiceStart, Shell,
};
use inst_model::condition::Condition;
use inst_model::graph::{Step, implicit_step};
use inst_model::platform::Os;
use inst_model::policy::{RebootPolicy, SameVersionPolicy};
use inst_model::privileges::Level;
use inst_model::project::{InputKind, InstallBase};
use inst_model::text::LocalizedText;
use inst_model::value::{KnownFolder, PathExpr, SecretRef, Value, ValueRef};

use crate::CodegenError;
use crate::input::{CodegenInput, PrereqDetection, PrereqKind, PrereqSource};

/// A Rust string literal for `s`.
pub fn lit(s: &str) -> String {
    format!("{s:?}")
}

fn lang(l: Language) -> &'static str {
    match l {
        Language::En => "Language::En",
        Language::Tr => "Language::Tr",
        Language::Ar => "Language::Ar",
        Language::Es => "Language::Es",
        Language::Fr => "Language::Fr",
        Language::De => "Language::De",
        Language::Ru => "Language::Ru",
    }
}

fn folder(f: KnownFolder) -> &'static str {
    match f {
        KnownFolder::Install => "Folder::Install",
        KnownFolder::Home => "Folder::Home",
        KnownFolder::UserConfig => "Folder::UserConfig",
        KnownFolder::UserData => "Folder::UserData",
        KnownFolder::MachineData => "Folder::MachineData",
        KnownFolder::Temp => "Folder::Temp",
        KnownFolder::Desktop => "Folder::Desktop",
    }
}

fn path_rel(p: &PathExpr) -> Result<String, CodegenError> {
    if !p.rel.is_empty() {
        inst_fsx::relpath::validate(&p.rel)
            .map_err(|e| CodegenError::Invalid(format!("path {:?}: {e}", p.rel)))?;
    }
    Ok(lit(&p.rel))
}

/// `&[(Language::En, "…"), …]` for the enabled languages, fallback first.
fn localized(t: &LocalizedText, langs: &[Language], fallback: Language) -> String {
    let mut out = String::from("&[");
    let mut order: Vec<Language> = vec![fallback];
    order.extend(langs.iter().copied().filter(|l| *l != fallback));
    let mut seen = Vec::new();
    for l in order {
        let text = t.get(l);
        if text.is_empty() || seen.contains(&text) && l != fallback && t.get(fallback) == text {
            continue;
        }
        seen.push(text);
        let _ = write!(out, "({}, {}), ", lang(l), lit(text));
    }
    out.push(']');
    out
}

fn value(v: &Value) -> Result<String, CodegenError> {
    Ok(match v {
        Value::Literal(s) => format!("Value::Lit({})", lit(s)),
        Value::Ref(r) => match r {
            ValueRef::Input { field } => format!("Value::Input({})", lit(field)),
            ValueRef::Secret { secret } => match secret {
                SecretRef::Input { field } => {
                    format!("Value::Secret(Secret::Input({}))", lit(field))
                }
                SecretRef::Env { var } => format!("Value::Secret(Secret::Env({}))", lit(var)),
            },
            ValueRef::Path { path } => {
                format!("Value::Path({}, {})", folder(path.base), path_rel(path)?)
            }
            ValueRef::ProductVersion => "Value::ProductVersion".into(),
            ValueRef::ProductName => "Value::ProductName".into(),
            ValueRef::Captured { name } => format!("Value::Captured({})", lit(name)),
        },
    })
}

fn values(vs: &[Value]) -> Result<String, CodegenError> {
    let mut out = String::from("&[");
    for v in vs {
        out.push_str(&value(v)?);
        out.push_str(", ");
    }
    out.push(']');
    Ok(out)
}

fn on_failure(p: FailurePolicy) -> &'static str {
    match p {
        FailurePolicy::Rollback => "OnFailure::Rollback",
        FailurePolicy::Continue => "OnFailure::Continue",
        FailurePolicy::Ask => "OnFailure::Ask",
    }
}

fn i32s(v: &[i32]) -> String {
    let items: Vec<String> = v.iter().map(i32::to_string).collect();
    format!("&[{}]", items.join(", "))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Install,
    Uninstall,
}

struct Emitter<'a> {
    input: &'a CodegenInput<'a>,
    statics: String,
    next_static: usize,
}

impl Emitter<'_> {
    fn os(&self) -> Os {
        self.input.target.os
    }

    fn prereq_index(&self, id: &str) -> Result<usize, CodegenError> {
        self.input
            .prerequisites
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| {
                CodegenError::Invalid(format!("condition references unknown prerequisite {id:?}"))
            })
    }

    /// Compiles a condition to a Rust boolean expression over `cx`.
    fn cond(&self, c: &Condition, ctx: Ctx) -> Result<String, CodegenError> {
        if let Some(b) = c.resolve_static(self.input.target.os, self.input.target.arch) {
            return Ok(b.to_string());
        }
        let install_only = |what: &str| -> Result<String, CodegenError> {
            if ctx == Ctx::Uninstall {
                Err(CodegenError::Invalid(format!(
                    "condition '{what}' is not available in uninstall actions"
                )))
            } else {
                Ok(String::new())
            }
        };
        Ok(match c {
            Condition::All { all } => {
                if all.is_empty() {
                    return Ok("true".into());
                }
                let parts: Result<Vec<_>, _> = all.iter().map(|c| self.cond(c, ctx)).collect();
                format!("({})", parts?.join(" && "))
            }
            Condition::Any { any } => {
                if any.is_empty() {
                    return Ok("false".into());
                }
                let parts: Result<Vec<_>, _> = any.iter().map(|c| self.cond(c, ctx)).collect();
                format!("({})", parts?.join(" || "))
            }
            Condition::Not { not } => format!("!{}", self.cond(not, ctx)?),
            Condition::Os { os } => format!("(cx.env().os == Os::{os:?})"),
            Condition::Arch { arch } => format!("(cx.env().arch == Arch::{arch:?})"),
            Condition::WindowsBuildAtLeast { build } => {
                install_only("windows build")?;
                format!("cx.windows_build_at_least({build})")
            }
            Condition::PrerequisiteMissing { prerequisite } => {
                install_only("prerequisite missing")?;
                format!(
                    "cx.prerequisite_missing({})",
                    self.prereq_index(prerequisite)?
                )
            }
            Condition::PrerequisiteInstalled { prerequisite } => {
                install_only("prerequisite installed")?;
                format!(
                    "!cx.prerequisite_missing({})",
                    self.prereq_index(prerequisite)?
                )
            }
            Condition::FileExists { path } => {
                format!("cx.file_exists({}, {})", folder(path.base), path_rel(path)?)
            }
            Condition::EnvVarSet { name } => format!("cx.env_var_set({})", lit(name)),
            Condition::EnvVarEquals { name, value } => {
                format!("cx.env_var_equals({}, {})", lit(name), lit(value))
            }
            Condition::CommandAvailable { name } => format!("cx.command_available({})", lit(name)),
            Condition::Elevated => "cx.elevated()".into(),
            Condition::FreshInstall => {
                install_only("fresh install")?;
                "cx.is_fresh_install()".into()
            }
            Condition::Upgrade => {
                install_only("upgrade")?;
                "cx.is_upgrade()".into()
            }
            Condition::Repair => {
                install_only("repair")?;
                "cx.is_repair()".into()
            }
            Condition::OptionSelected { option } => {
                install_only("option selected")?;
                format!("cx.option({})", lit(option))
            }
            Condition::CapturedEquals { name, value } => {
                install_only("captured value")?;
                format!("cx.captured_equals({}, {})", lit(name), lit(value))
            }
        })
    }

    fn new_static(&mut self, ty: &str, body: String) -> String {
        let name = format!("ACTION_{}", self.next_static);
        self.next_static += 1;
        let _ = writeln!(self.statics, "static {name}: {ty} = {body};\n");
        name
    }

    /// Emits the static spec for an action and returns the call statement,
    /// or `None` when the action does not apply to this target.
    fn action_call(&mut self, a: &Action, ctx: Ctx) -> Result<Option<String>, CodegenError> {
        let unsupported = |what: &str| CodegenError::Unsupported {
            action: a.id.clone(),
            what: what.to_owned(),
        };
        // Resolve the condition first so statically false actions emit nothing.
        let guard = match &a.condition {
            None => None,
            Some(c) => match self.cond(c, ctx)?.as_str() {
                "false" => return Ok(None),
                "true" => None,
                expr => Some(expr.to_owned()),
            },
        };
        let name = if a.name.is_empty() { &a.id } else { &a.name };
        let call = match &a.kind {
            ActionKind::Script(s) => {
                let shell = match &s.shell {
                    Shell::PowerShell => "Shell::PowerShell".to_owned(),
                    Shell::Sh => "Shell::Sh".to_owned(),
                    Shell::Custom { program, args } => {
                        let args: Vec<String> = args.iter().map(|a| lit(a)).collect();
                        format!(
                            "Shell::Custom {{ program: {}, args: &[{}] }}",
                            lit(program),
                            args.join(", ")
                        )
                    }
                };
                let source = match &s.source {
                    ScriptSource::Inline { code } => {
                        format!("ScriptSource::Embedded({})", lit(code))
                    }
                    ScriptSource::Project { path } => {
                        let code = self
                            .input
                            .project_scripts
                            .iter()
                            .find(|(id, _)| id == &a.id)
                            .map(|(_, c)| c)
                            .ok_or_else(|| {
                                CodegenError::Invalid(format!(
                                    "script file {path:?} was not loaded"
                                ))
                            })?;
                        format!("ScriptSource::Embedded({})", lit(code))
                    }
                    ScriptSource::Payload { path } => {
                        inst_fsx::relpath::validate(path)
                            .map_err(|e| CodegenError::Invalid(e.to_string()))?;
                        format!("ScriptSource::Payload({})", lit(path))
                    }
                };
                let mut env = String::from("&[");
                for (k, v) in &s.env {
                    let _ = write!(env, "({}, {}), ", lit(k), value(v)?);
                }
                env.push(']');
                let wd = match &s.working_dir {
                    Some(p) => format!("Some(({}, {}))", folder(p.base), path_rel(p)?),
                    None => "None".into(),
                };
                let body = format!(
                    "Script {{ id: {}, name: {}, shell: {shell}, source: {source}, args: {}, env: {env}, working_dir: {wd}, timeout_secs: {}, allowed_exit_codes: {}, capture_stdout: {}, capture_stderr: {}, on_failure: {} }}",
                    lit(&a.id),
                    lit(name),
                    values(&s.args)?,
                    s.timeout_secs,
                    i32s(&s.allowed_exit_codes),
                    s.capture_stdout,
                    s.capture_stderr,
                    on_failure(a.on_failure),
                );
                let n = self.new_static("Script", body);
                format!("cx.run_script(&{n})?;")
            }
            ActionKind::Run(r) => {
                inst_fsx::relpath::validate(&r.program)
                    .map_err(|e| CodegenError::Invalid(e.to_string()))?;
                let body = format!(
                    "RunProgram {{ id: {}, name: {}, program: {}, args: {}, wait: {}, timeout_secs: {}, allowed_exit_codes: {}, on_failure: {} }}",
                    lit(&a.id),
                    lit(name),
                    lit(&r.program),
                    values(&r.args)?,
                    r.wait,
                    r.timeout_secs,
                    i32s(&r.allowed_exit_codes),
                    on_failure(a.on_failure),
                );
                let n = self.new_static("RunProgram", body);
                format!("cx.run_program(&{n})?;")
            }
            _ if ctx == Ctx::Uninstall => {
                return Err(unsupported(
                    "only script and program actions run during uninstall",
                ));
            }
            ActionKind::Service(s) => {
                let start = match s.start {
                    ServiceStart::Automatic => "StartMode::Automatic",
                    ServiceStart::Manual => "StartMode::Manual",
                    ServiceStart::Disabled => "StartMode::Disabled",
                };
                let user = s.scope == ServiceScope::User && self.os() == Os::Linux;
                let body = format!(
                    "Service {{ id: {}, name: {}, display_name: {}, description: {}, executable: {}, args: {}, start: {start}, user: {user}, restart_on_failure: {}, start_after_install: {}, on_failure: {} }}",
                    lit(&a.id),
                    lit(&s.name),
                    lit(&s.display_name),
                    lit(&s.description),
                    lit(&s.executable),
                    values(&s.args)?,
                    s.restart_on_failure,
                    s.start_after_install,
                    on_failure(a.on_failure),
                );
                let n = self.new_static("Service", body);
                format!("cx.install_service(&{n})?;")
            }
            ActionKind::Registry(r) => {
                if self.os() != Os::Windows {
                    return Ok(None);
                }
                let data = match &r.value {
                    RegistryData::String(v) => format!("RegData::Sz({})", value(v)?),
                    RegistryData::ExpandString(v) => format!("RegData::ExpandSz({})", value(v)?),
                    RegistryData::MultiString(vs) => format!("RegData::MultiSz({})", values(vs)?),
                    RegistryData::Dword(d) => format!("RegData::Dword({d})"),
                    RegistryData::Qword(q) => format!("RegData::Qword({q})"),
                };
                let hive = match r.hive {
                    RegistryHive::CurrentUser => "HKCU",
                    RegistryHive::LocalMachine => "HKLM",
                };
                let body = format!(
                    "RegistryWrite {{ id: {}, hive: {hive}, key: {}, name: {}, data: {data}, remove_on_uninstall: {}, on_failure: {} }}",
                    lit(&a.id),
                    lit(&r.key),
                    lit(&r.name),
                    r.remove_on_uninstall,
                    on_failure(a.on_failure),
                );
                let n = self.new_static("RegistryWrite", body);
                format!("cx.set_registry(&{n})?;")
            }
            ActionKind::Environment(e) => {
                let op = match &e.operation {
                    EnvOperation::Set { value: v } => format!("EnvOp::Set({})", value(v)?),
                    EnvOperation::AppendPath { path } => {
                        format!("EnvOp::Append({}, {})", folder(path.base), path_rel(path)?)
                    }
                    EnvOperation::PrependPath { path } => {
                        format!("EnvOp::Prepend({}, {})", folder(path.base), path_rel(path)?)
                    }
                };
                let body = format!(
                    "EnvWrite {{ id: {}, machine: {}, name: {}, op: {op}, on_failure: {} }}",
                    lit(&a.id),
                    e.scope == inst_model::action::EnvScope::Machine,
                    lit(&e.name),
                    on_failure(a.on_failure),
                );
                let n = self.new_static("EnvWrite", body);
                format!("cx.set_env(&{n})?;")
            }
            ActionKind::Database(_) => {
                return Err(unsupported(
                    "database actions (SQLx/Tiberius providers) are not implemented in this runtime version",
                ));
            }
            ActionKind::Http(_) => {
                return Err(unsupported(
                    "HTTP actions are not implemented in this runtime version",
                ));
            }
        };
        Ok(Some(match guard {
            None => call,
            Some(expr) => format!("if {expr} {{ {call} }}"),
        }))
    }

    fn step_body(&mut self, step: &Step) -> Result<Vec<String>, CodegenError> {
        let p = self.input.project;
        let mut out = Vec::new();
        match step {
            Step::Start | Step::Finish => {}
            Step::CheckEnvironment => out.push("cx.check_environment()?;".into()),
            Step::CheckPrerequisites => out.push("cx.check_prerequisites()?;".into()),
            Step::AcquirePrerequisites => out.push("cx.acquire_prerequisites()?;".into()),
            Step::InstallPrerequisites => out.push("cx.install_prerequisites()?;".into()),
            Step::ResolveLocation => out.push("cx.resolve_location()?;".into()),
            Step::ExtractApplication => out.push("cx.extract_application()?;".into()),
            Step::CreateShortcuts => out.push("cx.create_shortcuts()?;".into()),
            Step::ApplyIntegration => out.push("cx.apply_integration()?;".into()),
            Step::RegisterUninstaller => out.push("cx.register_uninstaller()?;".into()),
            Step::VerifyInstallation => out.push("cx.verify_installation()?;".into()),
            Step::RunAction { action } => {
                let a = p
                    .action(action)
                    .ok_or_else(|| CodegenError::Invalid(format!("unknown action {action:?}")))?;
                if a.enabled
                    && let Some(call) = self.action_call(a, Ctx::Install)?
                {
                    out.push(call);
                }
            }
            Step::RunActions { .. } | Step::ConfigureDatabase | Step::InstallServices => {
                let event_of = |s: &Step| match s {
                    Step::RunActions { event } => *event,
                    _ => LifecycleEvent::AfterInstall,
                };
                let event = event_of(step);
                if event.is_uninstall() {
                    return Err(CodegenError::Invalid(format!(
                        "'{}' actions cannot be placed in the installation graph",
                        event.label()
                    )));
                }
                let actions: Vec<&Action> = p
                    .actions
                    .iter()
                    .filter(|a| {
                        a.enabled
                            && a.event == Some(event)
                            && implicit_step(&a.kind, event) == *step
                    })
                    .collect();
                for a in actions {
                    if let Some(call) = self.action_call(a, Ctx::Install)? {
                        out.push(call);
                    }
                }
                let guard = match event {
                    LifecycleEvent::OnRepair => Some("cx.is_repair()"),
                    LifecycleEvent::OnUpdate => Some("cx.is_upgrade()"),
                    _ => None,
                };
                if let (Some(g), false) = (guard, out.is_empty()) {
                    out = vec![format!("if {g} {{ {} }}", out.join(" "))];
                }
            }
        }
        Ok(out)
    }
}

fn step_label(step: &Step) -> (&'static str, u32) {
    match step {
        Step::CheckEnvironment | Step::ResolveLocation => ("Msg::StepPreparing", 1),
        Step::CheckPrerequisites => ("Msg::StepPrerequisites", 1),
        Step::AcquirePrerequisites => ("Msg::StepPrerequisites", 25),
        Step::InstallPrerequisites => ("Msg::StepPrerequisites", 20),
        Step::ExtractApplication => ("Msg::StepExtracting", 60),
        Step::ConfigureDatabase => ("Msg::StepDatabase", 5),
        Step::RunActions { .. } | Step::RunAction { .. } | Step::InstallServices => {
            ("Msg::StepTasks", 3)
        }
        Step::CreateShortcuts => ("Msg::StepShortcuts", 1),
        Step::ApplyIntegration | Step::RegisterUninstaller => ("Msg::StepRegistering", 1),
        Step::VerifyInstallation => ("Msg::StepVerifying", 2),
        Step::Start | Step::Finish => ("Msg::StepFinishing", 0),
    }
}

/// Parses a catalog argument template into a runtime `Arg` expression.
fn prereq_arg(a: &str) -> Result<String, CodegenError> {
    for (token, variant) in [("{secret:", "Secret"), ("{input:", "Input")] {
        if let Some(start) = a.find(token) {
            let rest = &a[start + token.len()..];
            let field = rest
                .strip_suffix('}')
                .filter(|f| !f.contains(['{', '}']))
                .ok_or_else(|| {
                    CodegenError::Invalid(format!("unsupported argument template {a:?}"))
                })?;
            let prefix = &a[..start];
            return Ok(if prefix.is_empty() {
                format!("Arg::{variant}({})", lit(field))
            } else {
                format!("Arg::Prefixed{variant}({}, {})", lit(prefix), lit(field))
            });
        }
    }
    Ok(format!("Arg::Lit({})", lit(a)))
}

fn prerequisites(input: &CodegenInput<'_>) -> Result<String, CodegenError> {
    let mut out = String::from("static PREREQUISITES: &[Prerequisite] = &[\n");
    for p in input.prerequisites {
        let detection = match &p.detection {
            PrereqDetection::Registry {
                key,
                value,
                version_value,
                per_user_too,
            } => format!(
                "Detection::Registry {{ key: {}, value: {}, version_value: {}, per_user_too: {per_user_too} }}",
                lit(key),
                lit(value),
                version_value
                    .as_deref()
                    .map_or("None".into(), |v| format!("Some({})", lit(v)))
            ),
            PrereqDetection::DotnetSharedFramework { framework } => {
                format!(
                    "Detection::DotnetSharedFramework {{ framework: {} }}",
                    lit(framework)
                )
            }
            PrereqDetection::Command {
                program,
                args,
                stderr,
            } => {
                let args: Vec<String> = args.iter().map(|a| lit(a)).collect();
                format!(
                    "Detection::Command {{ program: {}, args: &[{}], stderr: {stderr} }}",
                    lit(program),
                    args.join(", ")
                )
            }
            PrereqDetection::Service { name } => {
                format!("Detection::Service {{ name: {} }}", lit(name))
            }
        };
        let source = match &p.source {
            PrereqSource::Download { urls, hash, size } => {
                let urls: Vec<String> = urls.iter().map(|u| lit(u)).collect();
                format!(
                    "Source::Download {{ urls: &[{}], hash: {}, size: {size} }}",
                    urls.join(", "),
                    lit(hash)
                )
            }
            PrereqSource::Embedded { payload_path } => {
                format!("Source::Embedded {{ path: {} }}", lit(payload_path))
            }
        };
        let args: Result<Vec<String>, _> = p.args.iter().map(|a| prereq_arg(a)).collect();
        let _ = writeln!(
            out,
            "    Prerequisite {{ id: {}, name: {}, version: {}, min_version: {}, same_major: {}, detection: {detection}, source: {source}, file_name: {}, kind: InstallKind::{}, args: &[{}], success_codes: {}, reboot_codes: {}, already_installed_codes: {}, publisher: {} }},",
            lit(&p.id),
            lit(&p.name),
            lit(&p.version),
            p.min_version
                .as_deref()
                .map_or("None".into(), |v| format!("Some({})", lit(v))),
            p.same_major,
            lit(&p.file_name),
            match p.kind {
                PrereqKind::Exe => "Exe",
                PrereqKind::Msi => "Msi",
            },
            args?.join(", "),
            i32s(&p.success_codes),
            i32s(&p.reboot_codes),
            i32s(&p.already_installed_codes),
            p.publisher
                .as_deref()
                .map_or("None".into(), |v| format!("Some({})", lit(v))),
        );
    }
    out.push_str("];\n");
    Ok(out)
}

/// A folder name valid on every supported file system.
pub fn folder_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "\\/:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']);
    if trimmed.is_empty() || inst_fsx::relpath::validate_component(trimmed).is_err() {
        "Application".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn parse_accent(s: Option<&str>) -> u32 {
    s.and_then(|s| s.strip_prefix('#'))
        .filter(|h| h.len() == 6)
        .and_then(|h| u32::from_str_radix(h, 16).ok())
        .unwrap_or(0x002F_6FEB)
}

pub fn main_rs(input: &CodegenInput<'_>) -> Result<String, CodegenError> {
    let p = input.project;
    let langs = &p.ui.languages;
    let fallback = p.ui.fallback_language;
    let mut e = Emitter {
        input,
        statics: String::new(),
        next_static: 0,
    };
    let mut s = String::with_capacity(16 << 10);
    let _ = writeln!(
        s,
        "// @generated by {} {} for {}. Do not edit: regenerate from the project.",
        inst_brand::STUDIO_NAME,
        env!("CARGO_PKG_VERSION"),
        input.target
    );
    s.push_str("#![cfg_attr(windows, windows_subsystem = \"windows\")]\n");
    s.push_str("#![allow(unused_variables, unused_parens, unused_mut, clippy::all)]\n\n");
    s.push_str("use inst_runtime::prelude::*;\n\n");

    // Product.
    let logo = if input.logo_png.is_some() {
        "Some(include_bytes!(\"../logo.png\"))"
    } else {
        "None"
    };
    let opt = |v: &Option<String>| {
        v.as_deref()
            .map_or("None".to_owned(), |x| format!("Some({})", lit(x)))
    };
    let args: Vec<String> = p.application.arguments.iter().map(|a| lit(a)).collect();
    let _ = writeln!(
        s,
        "static PRODUCT: Product = Product {{\n    id: {},\n    name: {},\n    publisher: {},\n    version: {},\n    description: {},\n    homepage: {},\n    support_url: {},\n    main_executable: {},\n    arguments: &[{}],\n    icon: {},\n    logo_png: {logo},\n}};\n",
        lit(p.product.id.as_str()),
        lit(&p.product.name),
        lit(&p.product.publisher),
        lit(&p.product.version.to_string()),
        localized(&p.product.description, langs, fallback),
        opt(&p.product.homepage),
        opt(&p.product.support_url),
        opt(&p.application.main_executable),
        args.join(", "),
        opt(&input.installed_icon),
    );

    // Fields.
    s.push_str("static FIELDS: &[Field] = &[\n");
    for f in &p.ui.fields {
        let kind = match &f.kind {
            InputKind::Text { default } => {
                format!("FieldKind::Text {{ default: {} }}", lit(default))
            }
            InputKind::Password => "FieldKind::Password".into(),
            InputKind::Checkbox { default } => {
                format!("FieldKind::Checkbox {{ default: {default} }}")
            }
            InputKind::Select { options, default } => {
                let opts: Vec<String> = options
                    .iter()
                    .map(|(k, v)| format!("({}, {})", lit(k), lit(v.get(fallback))))
                    .collect();
                format!(
                    "FieldKind::Select {{ options: &[{}], default: {} }}",
                    opts.join(", "),
                    lit(default)
                )
            }
            InputKind::Number { default, min, max } => {
                format!("FieldKind::Number {{ default: {default}, min: {min}, max: {max} }}")
            }
        };
        let _ = writeln!(
            s,
            "    Field {{ id: {}, label: {}, kind: {kind}, required: {}, prominent: {} }},",
            lit(&f.id),
            localized(&f.label, langs, fallback),
            f.required,
            f.prominent
        );
    }
    s.push_str("];\n\n");

    // Settings.
    let pol = &p.policy;
    let integ = &p.integration;
    let scope = if input.privileges.install_base == InstallBase::PerMachine {
        "Scope::Machine"
    } else {
        "Scope::User"
    };
    let folder = folder_name(
        p.install
            .location
            .folder
            .as_deref()
            .unwrap_or(&p.product.name),
    );
    let lang_list: Vec<&str> = langs.iter().map(|l| lang(*l)).collect();
    let _ = writeln!(
        s,
        "static SETTINGS: Settings = Settings {{\n    scope: {scope},\n    requires_elevation: {},\n    folder: {},\n    allow_change_location: {},\n    languages: &[{}],\n    fallback_language: {},\n    language_selector: {},\n    policy: Policy {{ integrity_verification: {}, rollback: {}, logging: {}, silent_install: {}, silent_uninstall: {}, uninstaller: {}, detect_existing_version: {}, disk_space_check: {}, failure_recovery: {}, allow_downgrade: {}, same_version: SameVersion::{}, reboot: Reboot::{} }},\n    integration: Integration {{ start_menu: {}, desktop_shortcut: {}, launch_at_startup: {}, add_to_path: {}, offer_launch: {}, file_associations: &[], protocols: &[] }},\n    fields: FIELDS,\n    installed_size: {},\n    template: {},\n    accent: {:#08x},\n}};\n",
        input.privileges.effective == Level::Admin,
        lit(&folder),
        p.install.allow_change_location,
        lang_list.join(", "),
        lang(fallback),
        p.ui.language_selector && langs.len() > 1,
        pol.integrity_verification,
        pol.rollback,
        pol.logging,
        pol.silent_install,
        pol.silent_uninstall,
        pol.uninstaller,
        pol.detect_existing_version,
        pol.disk_space_check,
        pol.failure_recovery,
        pol.allow_downgrade,
        match pol.same_version {
            SameVersionPolicy::Repair => "Repair",
            SameVersionPolicy::Reinstall => "Reinstall",
            SameVersionPolicy::Block => "Block",
        },
        match pol.reboot {
            RebootPolicy::Never => "Never",
            RebootPolicy::Prompt => "Prompt",
            RebootPolicy::Automatic => "Automatic",
        },
        integ.start_menu,
        integ.desktop_shortcut,
        integ.launch_at_startup,
        integ.add_to_path,
        integ.offer_launch && p.application.main_executable.is_some(),
        input.installed_size,
        lit(&p.ui.template),
        parse_accent(p.ui.accent_color.as_deref()),
    );

    s.push_str(&prerequisites(input)?);
    s.push('\n');

    // Installation graph → straight-line code.
    let graph = p.effective_graph();
    let issues: Vec<_> = graph
        .validate(p)
        .into_iter()
        .filter(|i| !i.is_warning())
        .collect();
    if let Some(issue) = issues.first() {
        return Err(CodegenError::Invalid(format!(
            "installation graph: {issue}"
        )));
    }
    let order = graph
        .topo_order()
        .map_err(|e| CodegenError::Invalid(format!("installation graph: {e}")))?;
    let mut steps = String::from("static STEPS: &[StepInfo] = &[\n");
    let mut body = String::from("fn install(cx: &mut Install<'_>) -> Result<(), InstallError> {\n");
    let mut step_index = 0usize;
    for &n in &order {
        let node = &graph.nodes[n];
        let incoming: Vec<String> = graph
            .edges
            .iter()
            .filter(|edge| edge.to == node.id)
            .filter_map(|edge| {
                let from = graph.node_index(&edge.from)?;
                match &edge.condition {
                    None => Some(Ok(format!("a{from}"))),
                    Some(c) => match e.cond(c, Ctx::Install) {
                        Ok(x) if x == "false" => None,
                        Ok(x) if x == "true" => Some(Ok(format!("a{from}"))),
                        Ok(x) => Some(Ok(format!("(a{from} && {x})"))),
                        Err(err) => Some(Err(err)),
                    },
                }
            })
            .collect::<Result<_, _>>()?;
        let active = if node.step == Step::Start {
            "true".to_owned()
        } else if incoming.is_empty() {
            "false".to_owned()
        } else {
            incoming.join(" || ")
        };
        let _ = writeln!(body, "    // {} ({})", node.id, node.step.label());
        let _ = writeln!(body, "    let a{n} = {active};");
        if matches!(node.step, Step::Start | Step::Finish) {
            continue;
        }
        let calls = e.step_body(&node.step)?;
        let (label, weight) = step_label(&node.step);
        let _ = writeln!(
            steps,
            "    StepInfo {{ id: {}, label: {label}, weight: {weight} }},",
            lit(&node.id)
        );
        let k = step_index;
        step_index += 1;
        let node_cond = match &node.condition {
            None => "true".to_owned(),
            Some(c) => e.cond(c, Ctx::Install)?,
        };
        let run = format!("cx.step({k}, |cx| {{ {} Ok(()) }})?;", calls.join(" "));
        match node_cond.as_str() {
            "true" => {
                let _ = writeln!(body, "    if a{n} {{ {run} }}");
            }
            "false" => {
                let _ = writeln!(body, "    if a{n} {{ cx.skip({k}); }}");
            }
            c => {
                let _ = writeln!(
                    body,
                    "    if a{n} {{ if {c} {{ {run} }} else {{ cx.skip({k}); }} }}"
                );
            }
        }
    }
    body.push_str("    Ok(())\n}\n\n");
    steps.push_str("];\n\n");

    // Uninstall hooks.
    let mut hooks = String::new();
    for (fname, event) in [
        ("before_uninstall", LifecycleEvent::BeforeUninstall),
        ("after_uninstall", LifecycleEvent::AfterUninstall),
    ] {
        let _ = writeln!(
            hooks,
            "fn {fname}(cx: &mut Uninstall<'_>) -> Result<(), InstallError> {{"
        );
        for a in p
            .actions
            .iter()
            .filter(|a| a.enabled && a.event == Some(event))
        {
            if let Some(call) = e.action_call(a, Ctx::Uninstall)? {
                let _ = writeln!(hooks, "    {call}");
            }
        }
        hooks.push_str("    Ok(())\n}\n\n");
    }

    s.push_str(&e.statics);
    s.push_str(&steps);
    s.push_str(&body);
    s.push_str(&hooks);
    s.push_str(
        "static APP: App = App {\n    product: &PRODUCT,\n    settings: &SETTINGS,\n    steps: STEPS,\n    install,\n    before_uninstall,\n    after_uninstall,\n    prerequisites: PREREQUISITES,\n};\n\n",
    );
    if input.gui {
        s.push_str("fn main() {\n    inst_runtime::main(&APP, Some(inst_runtime_ui::run));\n}\n");
    } else {
        s.push_str("fn main() {\n    inst_runtime::main(&APP, None);\n}\n");
    }
    Ok(s)
}
