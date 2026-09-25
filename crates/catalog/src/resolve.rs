//! Build-time resolution of catalog entries.
//!
//! Resolution turns "`.NET Desktop Runtime` `^8.0` for Windows x64" into a
//! [`ResolvedPackage`]: exact version, concrete URLs in priority order, a
//! content hash and size. That record is what gets compiled into the
//! installer, so the runtime only has to download and verify.

use std::fmt;

use inst_fetch::ContentHash;
use inst_model::ids::{Version, VersionReq};
use inst_model::platform::{Arch, Os};

use crate::schema::{
    CatalogItem, Detection, FileKind, InstallSpec, IntegrityStrategy, Package, SignatureSpec,
};
use crate::{Redundancy, redundancy};

/// Network access needed by resolution. Implemented by the build system on
/// top of `inst-fetch`; tests use fixtures.
pub trait Fetcher {
    fn get_text(&self, url: &str) -> Result<String, String>;
    /// Size of the resource (HEAD / Content-Length).
    fn content_length(&self, url: &str) -> Result<u64, String>;
    /// Downloads the resource and hashes it with the algorithm of `like`.
    /// Implementations should also verify the Authenticode publisher when
    /// `publisher` is set and the host can check it.
    fn download_hash(
        &self,
        url: &str,
        like: &ContentHash,
        publisher: Option<&str>,
    ) -> Result<(ContentHash, u64), String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPackage {
    pub id: String,
    pub name: String,
    pub version: String,
    pub os: Os,
    pub arch: Arch,
    pub file: FileKind,
    pub file_name: String,
    pub urls: Vec<String>,
    pub hash: ContentHash,
    pub size: u64,
    pub detection: Detection,
    pub install: InstallSpec,
    pub signature: Option<SignatureSpec>,
    pub requires_admin: bool,
    pub redundancy: Redundancy,
    /// How the hash was obtained, for the build report.
    pub integrity_source: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResolveError {
    NoPackage { id: String, os: Os, arch: Arch },
    Unverifiable(String),
    NeedsExactVersion { id: String, example: String },
    NoMatchingVersion { id: String, req: String },
    Metadata(String),
    Network(String),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::NoPackage { id, os, arch } => {
                write!(f, "{id} has no package for {} {arch:?}", os.name())
            }
            ResolveError::Unverifiable(id) => write!(
                f,
                "{id} has no verifiable integrity metadata; use Embedded or Custom sources with a hash"
            ),
            ResolveError::NeedsExactVersion { id, example } => {
                write!(f, "{id}: specify an exact version, e.g. ={example}")
            }
            ResolveError::NoMatchingVersion { id, req } => {
                write!(f, "{id}: no release matches {req}")
            }
            ResolveError::Metadata(e) => write!(f, "invalid vendor metadata: {e}"),
            ResolveError::Network(e) => write!(f, "network error: {e}"),
        }
    }
}

impl std::error::Error for ResolveError {}

fn fill(template: &str, version: &str, channel: &str) -> String {
    let major = version.split('.').next().unwrap_or(version);
    template
        .replace("{version}", version)
        .replace("{channel}", channel)
        .replace("{version_major}", major)
}

/// Picks the channel for a requirement: the first catalog channel whose
/// prefix matches the requirement's minimum version, else the newest.
fn pick_channel<'a>(item: &'a CatalogItem, req: &VersionReq) -> &'a str {
    if let Some(min) = req.minimum() {
        let parts = min.parts();
        for ch in &item.channels {
            let ch_parts: Vec<&str> = ch.split('.').collect();
            let matches = ch_parts
                .iter()
                .zip(parts)
                .all(|(c, p)| c.parse::<u32>().ok() == Some(*p));
            if matches {
                return ch;
            }
        }
    }
    item.channels.first().map_or("", String::as_str)
}

fn exact_version(req: &VersionReq) -> Option<String> {
    let s = req.to_string();
    s.strip_prefix('=').map(str::to_owned)
}

pub fn resolve(
    item: &CatalogItem,
    os: Os,
    arch: Arch,
    req: &VersionReq,
    fetcher: &dyn Fetcher,
) -> Result<ResolvedPackage, ResolveError> {
    let package = item
        .package(os, arch)
        .ok_or_else(|| ResolveError::NoPackage {
            id: item.id.clone(),
            os,
            arch,
        })?;
    let channel = pick_channel(item, req).to_owned();
    let (version, urls, hash, size, integrity_source) = match &package.integrity {
        IntegrityStrategy::Unverifiable => return Err(ResolveError::Unverifiable(item.id.clone())),
        IntegrityStrategy::DotnetReleaseMetadata {
            metadata_url,
            component,
            file_name,
        } => {
            let json = fetcher
                .get_text(&fill(metadata_url, "", &channel))
                .map_err(ResolveError::Network)?;
            let (version, url, hash) = dotnet_pick(&json, component, file_name, req)
                .map_err(ResolveError::Metadata)?
                .ok_or_else(|| ResolveError::NoMatchingVersion {
                    id: item.id.clone(),
                    req: req.to_string(),
                })?;
            let mut urls = template_urls(package, &version, &channel);
            if !urls.contains(&url) {
                urls.push(url);
            }
            let size = fetcher
                .content_length(&urls[0])
                .map_err(ResolveError::Network)?;
            (
                version,
                urls,
                hash,
                size,
                "vendor release metadata (SHA-512)",
            )
        }
        IntegrityStrategy::ChecksumFile { url, file_name } => {
            let version = match exact_version(req) {
                Some(v) => v,
                None if item.id == "nodejs" => {
                    let index = fetcher
                        .get_text("https://nodejs.org/dist/index.json")
                        .map_err(ResolveError::Network)?;
                    node_pick(&index, &channel, req)
                        .map_err(ResolveError::Metadata)?
                        .ok_or_else(|| ResolveError::NoMatchingVersion {
                            id: item.id.clone(),
                            req: req.to_string(),
                        })?
                }
                None => {
                    return Err(ResolveError::NeedsExactVersion {
                        id: item.id.clone(),
                        example: format!("{channel}.0"),
                    });
                }
            };
            let sums = fetcher
                .get_text(&fill(url, &version, &channel))
                .map_err(ResolveError::Network)?;
            let wanted = fill(file_name, &version, &channel);
            let hash = checksum_lookup(&sums, &wanted).ok_or_else(|| {
                ResolveError::Metadata(format!("{wanted} not listed in checksum file"))
            })?;
            let urls = template_urls(package, &version, &channel);
            let size = fetcher
                .content_length(&urls[0])
                .map_err(ResolveError::Network)?;
            (version, urls, hash, size, "vendor checksum file (SHA-256)")
        }
        IntegrityStrategy::PinAtBuild => {
            let templated = package.sources.iter().any(|s| s.url.contains("{version}"));
            let version = match exact_version(req) {
                Some(v) => v,
                None if templated => {
                    return Err(ResolveError::NeedsExactVersion {
                        id: item.id.clone(),
                        example: format!("{channel}.0"),
                    });
                }
                None => format!("latest ({channel})"),
            };
            let urls = template_urls(package, &version, &channel);
            let publisher = package
                .signature
                .as_ref()
                .map(|s| s.authenticode_publisher.as_str());
            let (hash, size) = fetcher
                .download_hash(&urls[0], &ContentHash::Sha256([0; 32]), publisher)
                .map_err(ResolveError::Network)?;
            (version, urls, hash, size, "pinned at build time (SHA-256)")
        }
    };
    let file_name = urls
        .first()
        .and_then(|u| u.rsplit('/').next())
        .filter(|n| !n.is_empty() && !n.contains('?'))
        .unwrap_or(&item.id)
        .to_owned();
    Ok(ResolvedPackage {
        id: item.id.clone(),
        name: item.name.clone(),
        version,
        os,
        arch,
        file: package.file,
        file_name,
        urls,
        hash,
        size,
        detection: package.detection.clone(),
        install: package.install.clone(),
        signature: package.signature.clone(),
        requires_admin: item.requires_admin.get(os),
        redundancy: redundancy(package),
        integrity_source,
    })
}

fn template_urls(package: &Package, version: &str, channel: &str) -> Vec<String> {
    package
        .sources
        .iter()
        .map(|s| fill(&s.url, version, channel))
        .collect()
}

/// Picks the newest release in a .NET `releases.json` whose `component`
/// version satisfies `req` and that ships `file_name`.
pub fn dotnet_pick(
    json: &str,
    component: &str,
    file_name: &str,
    req: &VersionReq,
) -> Result<Option<(String, String, ContentHash)>, String> {
    let doc: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let releases = doc
        .get("releases")
        .and_then(|r| r.as_array())
        .ok_or("missing releases array")?;
    let mut best: Option<(Version, String, ContentHash)> = None;
    for rel in releases {
        let Some(comp) = rel.get(component) else {
            continue;
        };
        let Some(version) = comp.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        let Ok(parsed) = Version::parse(version) else {
            continue;
        };
        if !req.matches(&parsed) {
            continue;
        }
        let Some(files) = comp.get("files").and_then(|f| f.as_array()) else {
            continue;
        };
        for f in files {
            if f.get("name").and_then(|n| n.as_str()) != Some(file_name) {
                continue;
            }
            let (Some(url), Some(hash)) = (
                f.get("url").and_then(|u| u.as_str()),
                f.get("hash").and_then(|h| h.as_str()),
            ) else {
                continue;
            };
            let Ok(hash) = ContentHash::parse(&format!("sha512:{hash}")) else {
                continue;
            };
            if !url.starts_with("https://") {
                continue;
            }
            if best.as_ref().is_none_or(|(v, _, _)| parsed > *v) {
                best = Some((parsed.clone(), url.to_owned(), hash));
            }
        }
    }
    Ok(best.map(|(v, u, h)| (v.to_string(), u, h)))
}

/// Picks the newest Node.js release in `index.json` for a channel.
pub fn node_pick(json: &str, channel: &str, req: &VersionReq) -> Result<Option<String>, String> {
    let doc: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let list = doc.as_array().ok_or("expected an array")?;
    let mut best: Option<Version> = None;
    for entry in list {
        let Some(v) = entry.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        let Ok(parsed) = Version::parse(v) else {
            continue;
        };
        if parsed.major().to_string() != channel || !req.matches(&parsed) || parsed.pre().is_some()
        {
            continue;
        }
        if best.as_ref().is_none_or(|b| parsed > *b) {
            best = Some(parsed);
        }
    }
    Ok(best.map(|v| v.to_string()))
}

/// Finds `file_name` in a `sha256sum`-style listing (`<hex>  [*./]name`).
pub fn checksum_lookup(sums: &str, file_name: &str) -> Option<ContentHash> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hex = parts.next()?;
        let name = parts.next()?;
        let name = name.trim_start_matches('*').trim_start_matches("./");
        if name == file_name {
            ContentHash::parse(&format!("sha256:{hex}")).ok()
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Catalog;
    use std::cell::RefCell;
    use std::collections::HashMap;

    struct Fixture {
        texts: HashMap<String, String>,
        sizes: HashMap<String, u64>,
        downloads: RefCell<Vec<String>>,
    }

    impl Fetcher for Fixture {
        fn get_text(&self, url: &str) -> Result<String, String> {
            self.texts
                .get(url)
                .cloned()
                .ok_or_else(|| format!("404 {url}"))
        }
        fn content_length(&self, url: &str) -> Result<u64, String> {
            self.sizes
                .get(url)
                .copied()
                .ok_or_else(|| format!("404 {url}"))
        }
        fn download_hash(
            &self,
            url: &str,
            _like: &ContentHash,
            _p: Option<&str>,
        ) -> Result<(ContentHash, u64), String> {
            self.downloads.borrow_mut().push(url.to_owned());
            Ok((ContentHash::Sha256([7; 32]), 1234))
        }
    }

    const DOTNET_JSON: &str = r#"{"releases":[
      {"windowsdesktop":{"version":"8.0.31","files":[
        {"name":"windowsdesktop-runtime-win-x64.exe","url":"https://builds.dotnet.microsoft.com/dotnet/WindowsDesktop/8.0.31/windowsdesktop-runtime-8.0.31-win-x64.exe","hash":"605189223cf0a64bfb5453520b794d1d386a97c7e65e3944feaf9214db1a8b247f66c569265aef4204447baf5b5ff19559c323d887e1318f17deb28a5af0fa12"}]}},
      {"windowsdesktop":{"version":"8.0.30","files":[
        {"name":"windowsdesktop-runtime-win-x64.exe","url":"https://builds.dotnet.microsoft.com/dotnet/WindowsDesktop/8.0.30/windowsdesktop-runtime-8.0.30-win-x64.exe","hash":"00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"}]}}
    ]}"#;

    #[test]
    fn resolves_dotnet_from_release_metadata() {
        let c = Catalog::builtin();
        let item = c.get("dotnet-desktop-runtime").expect("item");
        let primary = "https://builds.dotnet.microsoft.com/dotnet/WindowsDesktop/8.0.31/windowsdesktop-runtime-8.0.31-win-x64.exe";
        let fx = Fixture {
            texts: [(
                "https://builds.dotnet.microsoft.com/dotnet/release-metadata/8.0/releases.json"
                    .to_owned(),
                DOTNET_JSON.to_owned(),
            )]
            .into(),
            sizes: [
                (primary.to_owned(), 58_000_000),
                (primary.replace("8.0.31", "8.0.30"), 57_000_000),
            ]
            .into(),
            downloads: RefCell::default(),
        };
        let req = VersionReq::parse("^8.0").expect("req");
        let r = resolve(item, Os::Windows, Arch::X64, &req, &fx).expect("resolve");
        assert_eq!(r.version, "8.0.31");
        assert_eq!(r.urls[0], primary);
        assert_eq!(r.urls.len(), 2);
        assert_eq!(r.hash.algorithm(), "sha512");
        assert_eq!(r.size, 58_000_000);
        assert!(r.requires_admin);
        assert_eq!(r.file_name, "windowsdesktop-runtime-8.0.31-win-x64.exe");
        assert!(
            fx.downloads.borrow().is_empty(),
            "metadata resolution must not download the file"
        );

        let old = VersionReq::parse("=8.0.30").expect("req");
        assert_eq!(
            resolve(item, Os::Windows, Arch::X64, &old, &fx)
                .expect("resolve")
                .version,
            "8.0.30"
        );
    }

    #[test]
    fn resolves_node_with_checksum_file() {
        let c = Catalog::builtin();
        let item = c.get("nodejs").expect("item");
        let fx = Fixture {
            texts: [
                (
                    "https://nodejs.org/dist/index.json".to_owned(),
                    r#"[{"version":"v23.1.0"},{"version":"v22.11.0"},{"version":"v22.10.0"},{"version":"v20.18.0"}]"#.to_owned(),
                ),
                (
                    "https://nodejs.org/dist/v22.11.0/SHASUMS256.txt".to_owned(),
                    "aaaa  node-v22.11.0.tar.gz\n1111111111111111111111111111111111111111111111111111111111111111  node-v22.11.0-x64.msi\n".to_owned(),
                ),
            ]
            .into(),
            sizes: [("https://nodejs.org/dist/v22.11.0/node-v22.11.0-x64.msi".to_owned(), 30_000_000)].into(),
            downloads: RefCell::default(),
        };
        let r = resolve(
            item,
            Os::Windows,
            Arch::X64,
            &VersionReq::parse("^22").expect("req"),
            &fx,
        )
        .expect("resolve");
        assert_eq!(r.version, "22.11.0");
        assert_eq!(r.hash.to_string(), format!("sha256:{}", "1".repeat(64)));
        assert_eq!(r.redundancy, Redundancy::Degraded { routes: 1 });
    }

    #[test]
    fn pin_at_build_downloads_and_needs_exact_versions_for_templates() {
        let c = Catalog::builtin();
        let fx = Fixture {
            texts: HashMap::new(),
            sizes: HashMap::new(),
            downloads: RefCell::default(),
        };
        let vc = c.get("vc-redist").expect("vc");
        let r = resolve(vc, Os::Windows, Arch::X64, &VersionReq::ANY, &fx).expect("resolve");
        assert_eq!(r.size, 1234);
        assert_eq!(
            fx.downloads.borrow().as_slice(),
            ["https://aka.ms/vs/17/release/vc_redist.x64.exe"]
        );

        let git = c.get("git").expect("git");
        assert!(matches!(
            resolve(git, Os::Windows, Arch::X64, &VersionReq::ANY, &fx),
            Err(ResolveError::NeedsExactVersion { .. })
        ));
        let nssm = c.get("nssm").expect("nssm");
        assert!(matches!(
            resolve(nssm, Os::Windows, Arch::X64, &VersionReq::ANY, &fx),
            Err(ResolveError::Unverifiable(_))
        ));
    }

    #[test]
    fn checksum_file_variants() {
        let sums = "dda018607640f015db90498bbfbd63edfd18f860376b388748ce281be82201a3  ./mariadb-11.4.3-winx64.msi\n";
        assert!(checksum_lookup(sums, "mariadb-11.4.3-winx64.msi").is_some());
        assert!(checksum_lookup(sums, "other.msi").is_none());
    }
}
