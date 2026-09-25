//! Code signing. Secrets never come from the project file and are never
//! put on a command line where a better channel exists.

use std::path::Path;
use std::process::Command;

use inst_model::project::{CredentialSource, WindowsSigning};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignStatus {
    Signed(String),
    NotConfigured,
    Failed(String),
}

fn credential(source: &CredentialSource) -> Result<String, String> {
    match source {
        CredentialSource::Env { var } => {
            std::env::var(var).map_err(|_| format!("environment variable {var} is not set"))
        }
        CredentialSource::Keychain { entry } => Err(format!(
            "OS credential store lookup ({entry}) is not implemented yet; use an environment variable"
        )),
        CredentialSource::Prompt => {
            Err("interactive password prompts are only available in the Studio UI".into())
        }
    }
}

fn run(mut cmd: Command) -> Result<(), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("cannot start {:?}: {e}", cmd.get_program()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{:?} failed: {}",
            cmd.get_program(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Signs a Windows executable in place (after the payload was appended, so
/// the signature covers it).
pub fn sign_windows(file: &Path, settings: Option<&WindowsSigning>) -> SignStatus {
    let Some(settings) = settings else {
        return SignStatus::NotConfigured;
    };
    let result = match settings {
        WindowsSigning::CertificateStore {
            thumbprint,
            timestamp_url,
        } => {
            let mut cmd = Command::new("signtool");
            cmd.args([
                "sign",
                "/sha1",
                thumbprint,
                "/fd",
                "sha256",
                "/tr",
                timestamp_url,
                "/td",
                "sha256",
            ])
            .arg(file);
            run(cmd).map(|()| format!("certificate {thumbprint}"))
        }
        WindowsSigning::PfxFile {
            path,
            password,
            timestamp_url,
        } => credential(password).and_then(|pw| {
            if cfg!(windows) {
                // signtool only accepts the password as an argument.
                let mut cmd = Command::new("signtool");
                cmd.args(["sign", "/f"])
                    .arg(path)
                    .args([
                        "/p",
                        &pw,
                        "/fd",
                        "sha256",
                        "/tr",
                        timestamp_url,
                        "/td",
                        "sha256",
                    ])
                    .arg(file);
                run(cmd)
            } else {
                // osslsigncode reads the password from a private file.
                let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
                let pass = dir.path().join("pass");
                std::fs::write(&pass, pw.as_bytes()).map_err(|e| e.to_string())?;
                let out = file.with_extension("signed.tmp");
                let mut cmd = Command::new("osslsigncode");
                cmd.args(["sign", "-pkcs12"])
                    .arg(path)
                    .arg("-readpass")
                    .arg(&pass)
                    .args(["-h", "sha256", "-ts", timestamp_url, "-in"])
                    .arg(file)
                    .arg("-out")
                    .arg(&out);
                run(cmd).and_then(|()| std::fs::rename(&out, file).map_err(|e| e.to_string()))
            }
            .map(|()| format!("PFX {}", path.display()))
        }),
        WindowsSigning::Command { program, args } => {
            let mut cmd = Command::new(program);
            for a in args {
                if a == "{file}" {
                    cmd.arg(file);
                } else {
                    cmd.arg(a);
                }
            }
            run(cmd).map(|()| format!("external command {program}"))
        }
    };
    match result {
        Ok(what) => SignStatus::Signed(what),
        Err(e) => SignStatus::Failed(e),
    }
}
