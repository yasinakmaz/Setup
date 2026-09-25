//! Typed installer actions: scripts, database operations, HTTP requests,
//! registry, environment and services.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::condition::Condition;
use crate::value::{PathExpr, SecretRef, Value};

/// Lifecycle events an action can be attached to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LifecycleEvent {
    BeforeInstall,
    AfterInstall,
    BeforeUninstall,
    AfterUninstall,
    OnRepair,
    OnUpdate,
}

impl LifecycleEvent {
    pub const ALL: [LifecycleEvent; 6] = [
        LifecycleEvent::BeforeInstall,
        LifecycleEvent::AfterInstall,
        LifecycleEvent::BeforeUninstall,
        LifecycleEvent::AfterUninstall,
        LifecycleEvent::OnRepair,
        LifecycleEvent::OnUpdate,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            LifecycleEvent::BeforeInstall => "Before Install",
            LifecycleEvent::AfterInstall => "After Install",
            LifecycleEvent::BeforeUninstall => "Before Uninstall",
            LifecycleEvent::AfterUninstall => "After Uninstall",
            LifecycleEvent::OnRepair => "On Repair",
            LifecycleEvent::OnUpdate => "On Update",
        }
    }

    pub const fn is_uninstall(self) -> bool {
        matches!(
            self,
            LifecycleEvent::BeforeUninstall | LifecycleEvent::AfterUninstall
        )
    }
}

/// What happens when an action fails.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailurePolicy {
    /// Abort and roll back the installation.
    #[default]
    Rollback,
    /// Log a warning and continue.
    Continue,
    /// Ask the user to retry, skip or abort (abort in silent mode).
    Ask,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Action {
    /// Local identifier, unique in the project.
    pub id: String,
    /// Display name in the Studio and in progress/log output.
    #[serde(default)]
    pub name: String,
    /// When the action runs. `None` means it is placed explicitly in the
    /// installation graph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<LifecycleEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Condition>,
    #[serde(default)]
    pub on_failure: FailurePolicy,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(flatten)]
    pub kind: ActionKind,
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
pub enum ActionKind {
    Script(ScriptAction),
    Database(DatabaseAction),
    Http(HttpAction),
    Registry(RegistryAction),
    Environment(EnvironmentAction),
    Service(ServiceAction),
    /// Launch a program (e.g. a first-run configuration tool).
    Run(RunAction),
}

impl ActionKind {
    pub const fn kind_name(&self) -> &'static str {
        match self {
            ActionKind::Script(_) => "Script",
            ActionKind::Database(_) => "Database",
            ActionKind::Http(_) => "HTTP",
            ActionKind::Registry(_) => "Registry",
            ActionKind::Environment(_) => "Environment",
            ActionKind::Service(_) => "Service",
            ActionKind::Run(_) => "Run program",
        }
    }
}

// ---------------------------------------------------------------- scripts

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "shell",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
pub enum Shell {
    /// Windows PowerShell (`powershell.exe`) or PowerShell 7 (`pwsh`), run
    /// with `-NoProfile -NonInteractive -ExecutionPolicy Bypass -File`.
    PowerShell,
    /// POSIX `/bin/sh`.
    Sh,
    /// An explicitly configured interpreter, e.g. `/bin/bash`.
    Custom { program: String, args: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "source",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
pub enum ScriptSource {
    /// Script text stored in the project; embedded into the installer.
    Inline { code: String },
    /// A script file from the application payload.
    Payload { path: String },
    /// A script file next to the project; embedded into the installer.
    Project { path: String },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Elevation {
    /// Inherit the installer's privileges.
    #[default]
    Inherit,
    /// Requires administrator/root; forces an elevated installer.
    Required,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ScriptAction {
    #[serde(flatten)]
    pub shell: Shell,
    #[serde(flatten)]
    pub source: ScriptSource,
    /// Arguments passed as separate argv entries — never concatenated into a
    /// command line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<Value>,
    /// Environment passed to the script. Prefer this over arguments for
    /// values from user input: no quoting rules can be violated.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<PathExpr>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u32,
    #[serde(default)]
    pub elevation: Elevation,
    #[serde(default = "default_exit_codes")]
    pub allowed_exit_codes: Vec<i32>,
    #[serde(default = "yes")]
    pub capture_stdout: bool,
    #[serde(default = "yes")]
    pub capture_stderr: bool,
}

fn default_timeout() -> u32 {
    300
}

fn default_exit_codes() -> Vec<i32> {
    vec![0]
}

// --------------------------------------------------------------- database

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DbProvider {
    Postgres,
    MySql,
    Sqlite,
    MsSql,
}

impl DbProvider {
    pub const fn name(self) -> &'static str {
        match self {
            DbProvider::Postgres => "PostgreSQL",
            DbProvider::MySql => "MySQL / MariaDB",
            DbProvider::Sqlite => "SQLite",
            DbProvider::MsSql => "SQL Server",
        }
    }

    /// Cargo feature of the runtime that implements this provider.
    pub const fn runtime_feature(self) -> &'static str {
        match self {
            DbProvider::Postgres => "db-postgres",
            DbProvider::MySql => "db-mysql",
            DbProvider::Sqlite => "db-sqlite",
            DbProvider::MsSql => "db-mssql",
        }
    }

    pub const fn default_port(self) -> Option<u16> {
        match self {
            DbProvider::Postgres => Some(5432),
            DbProvider::MySql => Some(3306),
            DbProvider::MsSql => Some(1433),
            DbProvider::Sqlite => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TlsMode {
    Disable,
    #[default]
    Prefer,
    Require,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DbConnection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<SecretRef>,
    /// SQLite database file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathExpr>,
    #[serde(default)]
    pub tls: TlsMode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "op",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
pub enum DbOperation {
    TestConnection,
    WaitUntilAvailable {
        timeout_secs: u32,
    },
    CreateDatabase {
        name: Value,
        #[serde(default = "yes")]
        if_not_exists: bool,
    },
    /// SQL with positional bind parameters (`$1`/`?`/`@P1` per provider).
    /// Values are bound, never concatenated.
    RunSql {
        sql: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        params: Vec<Value>,
    },
    RunSqlFile {
        /// File from the payload or the project.
        path: String,
    },
    /// Applies `*.sql` migrations from a directory in lexical order, once.
    RunMigrations {
        dir: String,
    },
    CheckDatabase {
        name: Value,
    },
    CheckTable {
        table: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DatabaseAction {
    pub provider: DbProvider,
    pub connection: DbConnection,
    #[serde(flatten)]
    pub operation: DbOperation,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u32,
}

// ------------------------------------------------------------------- http

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
pub enum HttpBody {
    Text {
        text: String,
        content_type: String,
    },
    /// A JSON document with typed values substituted into string leaves
    /// by JSON pointer.
    Json {
        json: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        substitutions: BTreeMap<String, Value>,
    },
    Form {
        fields: BTreeMap<String, Value>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
pub enum HttpAuth {
    Bearer { token: SecretRef },
    Basic { user: Value, password: SecretRef },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "from",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
pub enum CaptureSource {
    Status,
    Header {
        name: String,
    },
    /// RFC 6901 JSON pointer into the response body.
    JsonPointer {
        pointer: String,
    },
    Body,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capture {
    pub name: String,
    #[serde(flatten)]
    pub source: CaptureSource,
    /// Treat the captured value as a secret (redacted in logs).
    #[serde(default)]
    pub secret: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Retry {
    pub attempts: u32,
    pub backoff_ms: u32,
}

impl Default for Retry {
    fn default() -> Self {
        Retry {
            attempts: 3,
            backoff_ms: 500,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct HttpAction {
    pub method: HttpMethod,
    /// Absolute `https://` URL (plain `http://` triggers a Doctor warning).
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query: Vec<(String, Value)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<(String, Value)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<HttpBody>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<HttpAuth>,
    #[serde(default = "default_http_timeout")]
    pub timeout_secs: u32,
    #[serde(default)]
    pub retry: Retry,
    /// Accepted status codes; empty means 200-299.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_status: Vec<u16>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capture: Vec<Capture>,
    /// Follow redirects only to the same scheme+host by default.
    #[serde(default)]
    pub allow_cross_origin_redirects: bool,
}

fn default_http_timeout() -> u32 {
    30
}

// --------------------------------------------------------------- registry

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegistryHive {
    CurrentUser,
    LocalMachine,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
pub enum RegistryData {
    String(Value),
    ExpandString(Value),
    MultiString(Vec<Value>),
    Dword(u32),
    Qword(u64),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RegistryAction {
    pub hive: RegistryHive,
    /// Key path below the hive, e.g. `Software\Acme\Orders`.
    pub key: String,
    /// Value name; empty for the default value.
    #[serde(default)]
    pub name: String,
    pub value: RegistryData,
    /// Remove the value on uninstall.
    #[serde(default = "yes")]
    pub remove_on_uninstall: bool,
}

// ------------------------------------------------------------ environment

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnvScope {
    User,
    Machine,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "op",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
pub enum EnvOperation {
    Set { value: Value },
    AppendPath { path: PathExpr },
    PrependPath { path: PathExpr },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct EnvironmentAction {
    pub scope: EnvScope,
    pub name: String,
    #[serde(flatten)]
    pub operation: EnvOperation,
}

// --------------------------------------------------------------- services

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceStart {
    #[default]
    Automatic,
    Manual,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceScope {
    /// Windows service / systemd system unit. Requires elevation.
    #[default]
    System,
    /// systemd user unit on Linux (no root). Not available on Windows.
    User,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ServiceAction {
    /// Service name (no spaces).
    pub name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    /// Executable inside the payload.
    pub executable: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<Value>,
    #[serde(default)]
    pub start: ServiceStart,
    #[serde(default)]
    pub scope: ServiceScope,
    #[serde(default = "yes")]
    pub restart_on_failure: bool,
    #[serde(default = "yes")]
    pub start_after_install: bool,
}

// -------------------------------------------------------------------- run

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RunAction {
    /// Program inside the payload (relative path).
    pub program: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<Value>,
    #[serde(default)]
    pub wait: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u32,
    #[serde(default = "default_exit_codes")]
    pub allowed_exit_codes: Vec<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::ValueRef;

    #[derive(Serialize, Deserialize)]
    struct H {
        actions: Vec<Action>,
    }

    #[test]
    fn actions_roundtrip_through_toml() {
        let actions = vec![
            Action {
                id: "migrate".into(),
                name: "Run migrations".into(),
                event: Some(LifecycleEvent::AfterInstall),
                condition: None,
                on_failure: FailurePolicy::Rollback,
                enabled: true,
                kind: ActionKind::Database(DatabaseAction {
                    provider: DbProvider::Postgres,
                    connection: DbConnection {
                        host: Some(Value::literal("localhost")),
                        port: None,
                        database: Some(Value::literal("orders")),
                        user: Some(Value::Ref(ValueRef::Input {
                            field: "db_user".into(),
                        })),
                        password: Some(SecretRef::Input {
                            field: "db_password".into(),
                        }),
                        file: None,
                        tls: TlsMode::Prefer,
                    },
                    operation: DbOperation::RunMigrations {
                        dir: "migrations".into(),
                    },
                    timeout_secs: 60,
                }),
            },
            Action {
                id: "configure".into(),
                name: String::new(),
                event: Some(LifecycleEvent::AfterInstall),
                condition: Some(Condition::Os {
                    os: crate::platform::Os::Windows,
                }),
                on_failure: FailurePolicy::Continue,
                enabled: true,
                kind: ActionKind::Script(ScriptAction {
                    shell: Shell::PowerShell,
                    source: ScriptSource::Inline {
                        code: "Write-Output $env:PORT".into(),
                    },
                    args: vec![],
                    env: [("PORT".to_owned(), Value::literal("8080"))].into(),
                    working_dir: Some(PathExpr::install("")),
                    timeout_secs: 30,
                    elevation: Elevation::Inherit,
                    allowed_exit_codes: vec![0, 3010],
                    capture_stdout: true,
                    capture_stderr: true,
                }),
            },
            Action {
                id: "register".into(),
                name: "Register".into(),
                event: Some(LifecycleEvent::AfterInstall),
                condition: None,
                on_failure: FailurePolicy::Ask,
                enabled: false,
                kind: ActionKind::Http(HttpAction {
                    method: HttpMethod::Post,
                    url: "https://api.example.com/register".into(),
                    query: vec![],
                    headers: vec![("X-Version".into(), Value::Ref(ValueRef::ProductVersion))],
                    body: Some(HttpBody::Json {
                        json: r#"{"key": ""}"#.into(),
                        substitutions: [("/key".to_owned(), Value::literal("x"))].into(),
                    }),
                    auth: Some(HttpAuth::Bearer {
                        token: SecretRef::Env {
                            var: "TOKEN".into(),
                        },
                    }),
                    timeout_secs: 10,
                    retry: Retry::default(),
                    expected_status: vec![200, 201],
                    capture: vec![Capture {
                        name: "license".into(),
                        source: CaptureSource::JsonPointer {
                            pointer: "/license".into(),
                        },
                        secret: true,
                    }],
                    allow_cross_origin_redirects: false,
                }),
            },
        ];
        let text = toml::to_string(&H {
            actions: actions.clone(),
        })
        .expect("serialize");
        let back: H = toml::from_str(&text).expect("deserialize");
        assert_eq!(back.actions, actions, "{text}");
    }
}
