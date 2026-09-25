//! Static Rust code generation.
//!
//! Turns a typed [`Project`](inst_model::Project) for one target into a
//! self-contained Cargo crate: static descriptors, the installation graph as
//! straight-line Rust, a size-oriented release profile, only the runtime
//! features the project uses, and Windows resources (UAC manifest derived
//! from the privilege analysis, version information, icon).
//!
//! There is no interpreter anywhere: conditions become boolean expressions,
//! actions become `static` specs passed to typed runtime functions.

mod cargo;
mod emit;
pub mod input;

pub use emit::folder_name;
pub use input::*;

use inst_model::features::runtime_features;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodegenError {
    /// The project uses something this runtime version does not implement.
    Unsupported {
        action: String,
        what: String,
    },
    Invalid(String),
}

impl std::fmt::Display for CodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodegenError::Unsupported { action, what } => write!(f, "action '{action}': {what}"),
            CodegenError::Invalid(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for CodegenError {}

/// A generated file, relative to the crate root.
#[derive(Clone, Debug)]
pub struct GeneratedFile {
    pub path: &'static str,
    pub contents: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Generated {
    pub files: Vec<GeneratedFile>,
    /// Runtime cargo features enabled, with the reason for each.
    pub features: Vec<(String, String)>,
}

/// Runtime features that exist as cargo features of `inst-runtime`.
const RUNTIME_FEATURES: &[&str] = &[
    "zstd",
    "xz",
    "lang-tr",
    "lang-ar",
    "lang-es",
    "lang-fr",
    "lang-de",
    "lang-ru",
    "download",
    "scripts",
    "services",
    "registry",
    "environment",
];

pub fn generate(input: &CodegenInput<'_>) -> Result<Generated, CodegenError> {
    let main = emit::main_rs(input)?;
    let mut features: Vec<(String, String)> =
        runtime_features(input.project, input.target, input.gui)
            .into_iter()
            .filter(|f| RUNTIME_FEATURES.contains(&f.name))
            .map(|f| (f.name.to_owned(), f.reason))
            .collect();
    if input
        .prerequisites
        .iter()
        .any(|p| matches!(p.source, PrereqSource::Download { .. }))
        && !features.iter().any(|(n, _)| n == "download")
    {
        features.push(("download".into(), "prerequisite downloads".into()));
    }
    for codec in input.codec_features {
        features.push(((*codec).to_owned(), "payload decoder".into()));
    }
    features.sort();
    features.dedup_by(|a, b| a.0 == b.0);
    let names: Vec<&str> = features.iter().map(|(n, _)| n.as_str()).collect();

    let mut files = vec![
        GeneratedFile {
            path: "Cargo.toml",
            contents: cargo::cargo_toml(input, &names).into_bytes(),
        },
        GeneratedFile {
            path: "build.rs",
            contents: cargo::BUILD_RS.as_bytes().to_vec(),
        },
        GeneratedFile {
            path: "app.manifest",
            contents: cargo::app_manifest(input).into_bytes(),
        },
        GeneratedFile {
            path: "app.rc",
            contents: cargo::app_rc(input).into_bytes(),
        },
        GeneratedFile {
            path: "src/main.rs",
            contents: main.into_bytes(),
        },
    ];
    if let Some(png) = &input.logo_png {
        files.push(GeneratedFile {
            path: "logo.png",
            contents: png.clone(),
        });
    }
    if let Some(ico) = &input.windows_icon {
        files.push(GeneratedFile {
            path: "icon.ico",
            contents: ico.clone(),
        });
    }
    Ok(Generated { files, features })
}

#[cfg(test)]
mod tests;
