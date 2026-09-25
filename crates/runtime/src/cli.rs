//! Command-line interface of generated installers.
//!
//! Accepts both POSIX-style and common Windows installer switches so that
//! deployment tools work without per-product knowledge.

use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CliOptions {
    pub silent: bool,
    pub uninstall: bool,
    pub dir: Option<PathBuf>,
    pub language: Option<String>,
    pub sets: Vec<(String, String)>,
    pub log: Option<PathBuf>,
    pub desktop_shortcut: Option<bool>,
    pub launch: bool,
    pub verify_only: bool,
    pub help: bool,
}

pub const USAGE: &str = "\
Options:
  -s, --silent, /S, /quiet   Install (or uninstall) without user interface
      --dir <path>, /D=<path> Installation folder
      --uninstall            Remove the installed product
      --lang <code>          Language (en, tr, ar, es, fr, de, ru)
      --set <field>=<value>  Value of an installer input field (repeatable)
      --desktop-shortcut     Create a desktop shortcut
      --no-desktop-shortcut  Do not create a desktop shortcut
      --launch               Start the application after installation
      --log <path>           Write the detailed log to <path>
      --verify               Verify the setup file's integrity and exit
  -h, --help, /?             Show this help

Exit codes (Windows): 0 success, 3010 success/reboot required, 1602 cancelled,
1603 failure, 1620 damaged setup, 1638 other version installed, 5 access denied,
112 disk full, 87 invalid arguments.";

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<CliOptions, String> {
    let mut o = CliOptions::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let s = arg.to_string_lossy().into_owned();
        let mut value = |name: &str| -> Result<String, String> {
            it.next()
                .map(|v| v.to_string_lossy().into_owned())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match s.as_str() {
            "-s" | "--silent" | "/S" | "/s" | "/silent" | "/quiet" | "/qn" | "--quiet" => {
                o.silent = true
            }
            "--uninstall" | "/uninstall" | "/U" => o.uninstall = true,
            "--dir" => o.dir = Some(PathBuf::from(value("--dir")?)),
            "--lang" => o.language = Some(value("--lang")?),
            "--log" => o.log = Some(PathBuf::from(value("--log")?)),
            "--set" => {
                let kv = value("--set")?;
                let (k, v) = kv
                    .split_once('=')
                    .ok_or_else(|| format!("--set expects field=value, got {kv:?}"))?;
                o.sets.push((k.to_owned(), v.to_owned()));
            }
            "--desktop-shortcut" => o.desktop_shortcut = Some(true),
            "--no-desktop-shortcut" => o.desktop_shortcut = Some(false),
            "--launch" => o.launch = true,
            "--verify" => o.verify_only = true,
            "-h" | "--help" | "/?" | "/help" => o.help = true,
            other => {
                if let Some(d) = other
                    .strip_prefix("/D=")
                    .or_else(|| other.strip_prefix("--dir="))
                {
                    o.dir = Some(PathBuf::from(d));
                } else if let Some(l) = other.strip_prefix("--lang=") {
                    o.language = Some(l.to_owned());
                } else if let Some(kv) = other.strip_prefix("--set=") {
                    let (k, v) = kv
                        .split_once('=')
                        .ok_or_else(|| format!("invalid --set {kv:?}"))?;
                    o.sets.push((k.to_owned(), v.to_owned()));
                } else {
                    return Err(format!("unknown option {other:?}"));
                }
            }
        }
    }
    if let Some(d) = &o.dir
        && !d.is_absolute()
    {
        return Err("--dir must be an absolute path".into());
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<CliOptions, String> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn parses_common_switches() {
        let dir = if cfg!(windows) {
            "C:\\Apps\\Acme"
        } else {
            "/opt/acme"
        };
        let o = p(&[
            "/S",
            &format!("/D={dir}"),
            "--set",
            "db_password=p@ss=word",
            "--lang",
            "tr",
        ])
        .expect("parse");
        assert!(o.silent);
        assert_eq!(o.dir, Some(PathBuf::from(dir)));
        assert_eq!(o.sets, vec![("db_password".into(), "p@ss=word".into())]);
        assert_eq!(o.language.as_deref(), Some("tr"));
        assert!(p(&["--uninstall", "--silent"]).expect("parse").uninstall);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(p(&["--bogus"]).is_err());
        assert!(p(&["--dir"]).is_err());
        assert!(p(&["--dir", "relative/path"]).is_err());
        assert!(p(&["--set", "novalue"]).is_err());
    }
}
