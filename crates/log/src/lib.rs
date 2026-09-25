//! Structured logging with central secret redaction.
//!
//! Design goals:
//!
//! * **No secret ever reaches a sink.** Values registered with
//!   [`Redactor::register`] are replaced by `***` in every rendered record,
//!   whatever the call site did. Values wrapped in [`Secret`] print as `***`
//!   in `Debug` and `Display` as a second line of defence.
//! * **Small.** Generated installers include this crate; it has no
//!   dependencies and formats into a reused buffer.
//! * **Structured.** A record is a level, a target, a message and typed
//!   key/value fields; sinks decide how to render them (text or JSON lines).

use std::fmt::{self, Write as _};
use std::io::{self, Write};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Severity. Ordered from most to least severe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

impl Level {
    pub const fn as_str(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        }
    }

    pub fn parse(s: &str) -> Option<Level> {
        Some(match s.to_ascii_lowercase().as_str() {
            "error" => Level::Error,
            "warn" | "warning" => Level::Warn,
            "info" => Level::Info,
            "debug" => Level::Debug,
            "trace" => Level::Trace,
            _ => return None,
        })
    }
}

/// A value that must never be logged. `Debug` and `Display` print `***`.
///
/// Use [`Secret::expose`] at the single point where the plaintext is needed
/// (for example when opening a database connection).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub const fn new(value: T) -> Self {
        Secret(value)
    }

    #[inline]
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// Central registry of secret values to scrub from all output.
#[derive(Default)]
pub struct Redactor {
    secrets: RwLock<Vec<Box<str>>>,
}

impl Redactor {
    /// Minimum length of a registered secret. Shorter values would redact
    /// innocent text (e.g. a one-character password would blank every `a`).
    /// Short secrets are still protected by [`Secret`] wrapping.
    pub const MIN_LEN: usize = 3;

    pub fn register(&self, secret: &str) {
        if secret.len() < Self::MIN_LEN {
            return;
        }
        let mut secrets = self.secrets.write().unwrap_or_else(|e| e.into_inner());
        if !secrets.iter().any(|s| &**s == secret) {
            secrets.push(secret.into());
            // Longest first so that overlapping secrets are fully covered.
            secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.secrets
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }

    /// Replaces every registered secret in `text` in place.
    pub fn scrub(&self, text: &mut String) {
        let secrets = self.secrets.read().unwrap_or_else(|e| e.into_inner());
        for secret in secrets.iter() {
            if text.contains(&**secret) {
                *text = text.replace(&**secret, "***");
            }
        }
    }

    /// Returns a scrubbed copy of `text`.
    pub fn scrubbed(&self, text: &str) -> String {
        let mut out = text.to_owned();
        self.scrub(&mut out);
        out
    }
}

/// A typed field value.
#[derive(Clone, Copy)]
pub enum Value<'a> {
    Str(&'a str),
    U64(u64),
    I64(i64),
    Bool(bool),
    Display(&'a dyn fmt::Display),
    /// Always rendered as `***`.
    Redacted,
}

impl fmt::Display for Value<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Str(s) => f.write_str(s),
            Value::U64(v) => write!(f, "{v}"),
            Value::I64(v) => write!(f, "{v}"),
            Value::Bool(v) => write!(f, "{v}"),
            Value::Display(v) => write!(f, "{v}"),
            Value::Redacted => f.write_str("***"),
        }
    }
}

impl<'a> From<&'a str> for Value<'a> {
    fn from(v: &'a str) -> Self {
        Value::Str(v)
    }
}
impl<'a> From<&'a String> for Value<'a> {
    fn from(v: &'a String) -> Self {
        Value::Str(v)
    }
}
impl From<u64> for Value<'_> {
    fn from(v: u64) -> Self {
        Value::U64(v)
    }
}
impl From<u32> for Value<'_> {
    fn from(v: u32) -> Self {
        Value::U64(u64::from(v))
    }
}
impl From<usize> for Value<'_> {
    fn from(v: usize) -> Self {
        Value::U64(v as u64)
    }
}
impl From<i64> for Value<'_> {
    fn from(v: i64) -> Self {
        Value::I64(v)
    }
}
impl From<i32> for Value<'_> {
    fn from(v: i32) -> Self {
        Value::I64(i64::from(v))
    }
}
impl From<bool> for Value<'_> {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl<'a, T> From<&'a Secret<T>> for Value<'a> {
    fn from(_: &'a Secret<T>) -> Self {
        Value::Redacted
    }
}

/// A log record borrowed from the call site.
pub struct Record<'a> {
    pub level: Level,
    pub target: &'a str,
    pub message: fmt::Arguments<'a>,
    pub fields: &'a [(&'a str, Value<'a>)],
}

/// Output format of a [`WriterSink`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// `2026-09-25T10:00:00.123Z INFO target: message key=value`
    Text,
    /// One JSON object per line.
    JsonLines,
}

/// A destination for rendered log lines.
pub trait Sink: Send + Sync {
    fn max_level(&self) -> Level;
    /// Receives a fully rendered, already redacted line (without newline).
    fn write_line(&self, level: Level, line: &str);
    fn flush(&self) {}
}

/// Writes lines to any `io::Write` (a log file, stderr).
pub struct WriterSink<W: Write + Send> {
    writer: Mutex<W>,
    max_level: Level,
    format: Format,
}

impl<W: Write + Send> WriterSink<W> {
    pub fn new(writer: W, max_level: Level, format: Format) -> Self {
        WriterSink {
            writer: Mutex::new(writer),
            max_level,
            format,
        }
    }

    pub fn format(&self) -> Format {
        self.format
    }
}

impl<W: Write + Send> Sink for WriterSink<W> {
    fn max_level(&self) -> Level {
        self.max_level
    }

    fn write_line(&self, _level: Level, line: &str) {
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        // Logging must never abort an installation.
        let _ = w.write_all(line.as_bytes());
        let _ = w.write_all(b"\n");
    }

    fn flush(&self) {
        let _ = self
            .writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .flush();
    }
}

/// Collects lines in memory (tests, "Show details" panels).
#[derive(Default)]
pub struct MemorySink {
    lines: Mutex<Vec<(Level, String)>>,
    max_level: Option<Level>,
}

impl MemorySink {
    pub fn new(max_level: Level) -> Self {
        MemorySink {
            lines: Mutex::default(),
            max_level: Some(max_level),
        }
    }

    pub fn lines(&self) -> Vec<(Level, String)> {
        self.lines.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Sink for MemorySink {
    fn max_level(&self) -> Level {
        self.max_level.unwrap_or(Level::Trace)
    }

    fn write_line(&self, level: Level, line: &str) {
        self.lines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((level, line.to_owned()));
    }
}

struct SinkEntry {
    sink: Arc<dyn Sink>,
    format: Format,
}

/// A logger: a set of sinks sharing one [`Redactor`].
///
/// Loggers are passed explicitly (no global mutable state); cloning is cheap.
#[derive(Clone)]
pub struct Logger {
    inner: Arc<LoggerInner>,
}

struct LoggerInner {
    sinks: Vec<SinkEntry>,
    redactor: Arc<Redactor>,
    max_level: Level,
}

impl fmt::Debug for Logger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Logger")
            .field("sinks", &self.inner.sinks.len())
            .field("max_level", &self.inner.max_level)
            .finish()
    }
}

/// Builder for [`Logger`].
#[derive(Default)]
pub struct LoggerBuilder {
    sinks: Vec<SinkEntry>,
    redactor: Option<Arc<Redactor>>,
}

impl LoggerBuilder {
    pub fn sink(mut self, sink: Arc<dyn Sink>, format: Format) -> Self {
        self.sinks.push(SinkEntry { sink, format });
        self
    }

    pub fn redactor(mut self, redactor: Arc<Redactor>) -> Self {
        self.redactor = Some(redactor);
        self
    }

    pub fn build(self) -> Logger {
        let max_level = self
            .sinks
            .iter()
            .map(|s| s.sink.max_level())
            .max()
            .unwrap_or(Level::Error);
        Logger {
            inner: Arc::new(LoggerInner {
                sinks: self.sinks,
                redactor: self.redactor.unwrap_or_default(),
                max_level,
            }),
        }
    }
}

thread_local! {
    static LINE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

impl Logger {
    pub fn builder() -> LoggerBuilder {
        LoggerBuilder::default()
    }

    /// A logger that discards everything.
    pub fn disabled() -> Logger {
        LoggerBuilder::default().build()
    }

    pub fn redactor(&self) -> &Arc<Redactor> {
        &self.inner.redactor
    }

    #[inline]
    pub fn enabled(&self, level: Level) -> bool {
        !self.inner.sinks.is_empty() && level <= self.inner.max_level
    }

    pub fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.level) {
            return;
        }
        let timestamp = Timestamp::now();
        // Reuse a per-thread buffer: no allocation per record after warm-up
        // (except when a secret actually has to be replaced).
        LINE.with(|line| {
            let mut line = line.borrow_mut();
            for entry in &self.inner.sinks {
                if record.level > entry.sink.max_level() {
                    continue;
                }
                line.clear();
                let _ = render(&mut line, entry.format, timestamp, record);
                self.inner.redactor.scrub(&mut line);
                entry.sink.write_line(record.level, &line);
            }
        });
    }

    pub fn flush(&self) {
        for entry in &self.inner.sinks {
            entry.sink.flush();
        }
    }
}

fn render(out: &mut String, format: Format, ts: Timestamp, r: &Record<'_>) -> fmt::Result {
    match format {
        Format::Text => {
            write!(
                out,
                "{ts} {:<5} {}: {}",
                r.level.as_str(),
                r.target,
                r.message
            )?;
            for (key, value) in r.fields {
                write!(out, " {key}=")?;
                let start = out.len();
                write!(out, "{value}")?;
                if out[start..].contains([' ', '"', '=']) {
                    let rendered = out.split_off(start);
                    write!(out, "{rendered:?}")?;
                }
            }
            Ok(())
        }
        Format::JsonLines => {
            write!(
                out,
                "{{\"ts\":\"{ts}\",\"level\":\"{}\",\"target\":",
                r.level.as_str()
            )?;
            json_string(out, format_args!("{}", r.target))?;
            out.push_str(",\"msg\":");
            json_string(out, r.message)?;
            for (key, value) in r.fields {
                out.push(',');
                json_string(out, format_args!("{key}"))?;
                out.push(':');
                match value {
                    Value::U64(v) => write!(out, "{v}")?,
                    Value::I64(v) => write!(out, "{v}")?,
                    Value::Bool(v) => write!(out, "{v}")?,
                    other => json_string(out, format_args!("{other}"))?,
                }
            }
            out.push('}');
            Ok(())
        }
    }
}

fn json_string(out: &mut String, args: fmt::Arguments<'_>) -> fmt::Result {
    struct Escaper<'a>(&'a mut String);
    impl fmt::Write for Escaper<'_> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            for c in s.chars() {
                match c {
                    '"' => self.0.push_str("\\\""),
                    '\\' => self.0.push_str("\\\\"),
                    '\n' => self.0.push_str("\\n"),
                    '\r' => self.0.push_str("\\r"),
                    '\t' => self.0.push_str("\\t"),
                    c if (c as u32) < 0x20 => write!(self.0, "\\u{:04x}", c as u32)?,
                    c => self.0.push(c),
                }
            }
            Ok(())
        }
    }
    out.push('"');
    Escaper(out).write_fmt(args)?;
    out.push('"');
    Ok(())
}

/// UTC timestamp with millisecond precision, formatted as RFC 3339.
#[derive(Clone, Copy)]
struct Timestamp {
    secs: u64,
    millis: u32,
}

impl Timestamp {
    fn now() -> Self {
        let d = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Timestamp {
            secs: d.as_secs(),
            millis: d.subsec_millis(),
        }
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let days = self.secs / 86_400;
        let rem = self.secs % 86_400;
        let (y, m, d) = civil_from_days(days as i64);
        write!(
            f,
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
            rem / 3600,
            (rem / 60) % 60,
            rem % 60,
            self.millis
        )
    }
}

/// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Logs a record: `log!(logger, Info, "installer", "extracted {n} files"; "dir" => dir)`.
#[macro_export]
macro_rules! log {
    ($logger:expr, $level:ident, $target:expr, $($arg:tt)+) => {
        $crate::__log_impl!($logger, $crate::Level::$level, $target, [] $($arg)+)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __log_impl {
    ($logger:expr, $level:expr, $target:expr, [] $fmt:literal $(, $arg:expr)* ; $($key:literal => $value:expr),+ $(,)?) => {{
        let logger: &$crate::Logger = &$logger;
        if logger.enabled($level) {
            logger.log(&$crate::Record {
                level: $level,
                target: $target,
                message: format_args!($fmt $(, $arg)*),
                fields: &[$(($key, $crate::Value::from($value))),+],
            });
        }
    }};
    ($logger:expr, $level:expr, $target:expr, [] $fmt:literal $(, $arg:expr)* $(,)?) => {{
        let logger: &$crate::Logger = &$logger;
        if logger.enabled($level) {
            logger.log(&$crate::Record {
                level: $level,
                target: $target,
                message: format_args!($fmt $(, $arg)*),
                fields: &[],
            });
        }
    }};
}

/// Opens (appends to) a log file sink.
pub fn file_sink(
    path: &std::path::Path,
    max_level: Level,
    format: Format,
) -> io::Result<Arc<dyn Sink>> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    Ok(Arc::new(WriterSink::new(
        io::BufWriter::new(file),
        max_level,
        format,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logger_with_memory(format: Format) -> (Logger, Arc<MemorySink>) {
        let mem = Arc::new(MemorySink::new(Level::Debug));
        let logger = Logger::builder().sink(mem.clone(), format).build();
        (logger, mem)
    }

    #[test]
    fn redacts_registered_secrets_everywhere() {
        let (logger, mem) = logger_with_memory(Format::Text);
        logger.redactor().register("hunter2-pass");
        let conn = "postgres://admin:hunter2-pass@localhost/db";
        log!(logger, Info, "db", "connecting to {conn}"; "url" => conn);
        let lines = mem.lines();
        assert_eq!(lines.len(), 1);
        assert!(!lines[0].1.contains("hunter2"), "{}", lines[0].1);
        assert!(lines[0].1.contains("admin:***@localhost"));
    }

    #[test]
    fn secret_wrapper_never_prints() {
        let secret = Secret::new("s3cr3t".to_owned());
        assert_eq!(format!("{secret} {secret:?}"), "*** ***");
        let (logger, mem) = logger_with_memory(Format::JsonLines);
        log!(logger, Warn, "t", "x"; "password" => &secret, "n" => 3u64);
        let line = &mem.lines()[0].1;
        assert!(line.contains("\"password\":\"***\""), "{line}");
        assert!(line.contains("\"n\":3"), "{line}");
    }

    #[test]
    fn respects_levels() {
        let (logger, mem) = logger_with_memory(Format::Text);
        log!(logger, Trace, "t", "hidden");
        log!(logger, Debug, "t", "shown");
        assert_eq!(mem.lines().len(), 1);
        assert!(!Logger::disabled().enabled(Level::Error));
    }

    #[test]
    fn json_escapes() {
        let (logger, mem) = logger_with_memory(Format::JsonLines);
        log!(logger, Info, "t", "quote \" and \n newline");
        let line = &mem.lines()[0].1;
        assert!(line.contains(r#"quote \" and \n newline"#), "{line}");
    }

    #[test]
    fn timestamp_format() {
        let ts = Timestamp {
            secs: 1_790_000_000,
            millis: 7,
        };
        assert_eq!(ts.to_string(), "2026-09-21T14:13:20.007Z");
    }
}
