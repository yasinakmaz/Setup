//! Build-time prerequisite resolution.

use std::io::Read;
use std::path::{Path, PathBuf};

use inst_catalog::resolve::{self, Fetcher, ResolvedPackage};
use inst_catalog::{Catalog, Detection, InstallKind};
use inst_codegen::{PrereqDetection, PrereqInput, PrereqKind, PrereqSource};
use inst_fetch::transport::{Transport, TransportConfig, UreqTransport};
use inst_fetch::{Cache, ContentHash, DownloadRequest, Downloader, ItemInfo, RetryPolicy};
use inst_model::platform::Target;
use inst_model::project::{Acquisition, PrerequisiteRef, Project};

/// Network access for catalog resolution, backed by `inst-fetch`.
pub struct NetFetcher {
    transport: UreqTransport,
    temp: PathBuf,
}

impl NetFetcher {
    pub fn new(temp: PathBuf) -> Result<NetFetcher, String> {
        Ok(NetFetcher {
            transport: UreqTransport::new(&TransportConfig::default())
                .map_err(|e| e.to_string())?,
            temp,
        })
    }
}

impl Fetcher for NetFetcher {
    fn get_text(&self, url: &str) -> Result<String, String> {
        let resp = self
            .transport
            .get(url, 0, None)
            .map_err(|e| e.to_string())?;
        if resp.status != 200 {
            return Err(format!("HTTP {} for {url}", resp.status));
        }
        let mut body = String::new();
        resp.body
            .take(16 << 20)
            .read_to_string(&mut body)
            .map_err(|e| e.to_string())?;
        Ok(body)
    }

    fn content_length(&self, url: &str) -> Result<u64, String> {
        let resp = self
            .transport
            .get(url, 0, None)
            .map_err(|e| e.to_string())?;
        if resp.status != 200 {
            return Err(format!("HTTP {} for {url}", resp.status));
        }
        resp.total_len
            .ok_or_else(|| format!("{url} did not report its size"))
    }

    fn download_hash(
        &self,
        url: &str,
        like: &ContentHash,
        _publisher: Option<&str>,
    ) -> Result<(ContentHash, u64), String> {
        // Publisher (Authenticode) verification of pinned packages happens
        // on Windows build hosts via signtool; the hash pins the content.
        let resp = self
            .transport
            .get(url, 0, None)
            .map_err(|e| e.to_string())?;
        if resp.status != 200 {
            return Err(format!("HTTP {} for {url}", resp.status));
        }
        std::fs::create_dir_all(&self.temp).map_err(|e| e.to_string())?;
        let mut hasher = like.hasher();
        let mut body = resp.body;
        let mut buf = vec![0u8; 256 << 10];
        let mut total = 0u64;
        loop {
            let n = body.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            total += n as u64;
        }
        Ok((hasher.finalize(), total))
    }
}

/// A prerequisite resolved for one target, plus the package file when it is
/// embedded in the payload.
pub struct ResolvedPrereq {
    pub input: PrereqInput,
    pub embedded_file: Option<(String, PathBuf, u64)>,
    pub report: String,
}

fn detection(d: &Detection) -> PrereqDetection {
    match d {
        Detection::Registry {
            key,
            value,
            version_value,
            per_user_too,
        } => PrereqDetection::Registry {
            key: key.clone(),
            value: value.clone(),
            version_value: version_value.clone(),
            per_user_too: *per_user_too,
        },
        Detection::DotnetSharedFramework { framework } => PrereqDetection::DotnetSharedFramework {
            framework: framework.clone(),
        },
        Detection::Command {
            program,
            args,
            stderr,
        } => PrereqDetection::Command {
            program: program.clone(),
            args: args.clone(),
            stderr: *stderr,
        },
        Detection::Service { name } => PrereqDetection::Service { name: name.clone() },
    }
}

fn input_from(
    r: &ResolvedPackage,
    pref: &PrerequisiteRef,
    source: PrereqSource,
) -> Result<PrereqInput, String> {
    let kind = match r.install.kind {
        InstallKind::Exe => PrereqKind::Exe,
        InstallKind::Msi => PrereqKind::Msi,
        InstallKind::ExtractUserLocal => {
            return Err(format!(
                "{}: per-user archive installation is not supported by this runtime version",
                r.name
            ));
        }
    };
    Ok(PrereqInput {
        id: r.id.clone(),
        name: r.name.clone(),
        version: r.version.clone(),
        min_version: pref.version.minimum().map(ToString::to_string),
        same_major: matches!(r.detection, Detection::DotnetSharedFramework { .. }),
        detection: detection(&r.detection),
        source,
        file_name: r.file_name.clone(),
        kind,
        args: r.install.args.clone(),
        success_codes: r.install.success_codes.clone(),
        reboot_codes: r.install.reboot_codes.clone(),
        already_installed_codes: r.install.already_installed_codes.clone(),
        publisher: r
            .signature
            .as_ref()
            .map(|s| s.authenticode_publisher.clone()),
    })
}

/// Resolves every prerequisite that applies to `target`.
pub fn resolve_all(
    project: &Project,
    target: Target,
    catalog: &Catalog,
    fetcher: &dyn Fetcher,
    cache: &Cache,
) -> Result<Vec<ResolvedPrereq>, String> {
    let mut out = Vec::new();
    for pref in &project.prerequisites {
        let applies = pref
            .condition
            .as_ref()
            .and_then(|c| c.resolve_static(target.os, target.arch))
            .unwrap_or(true);
        if !applies {
            continue;
        }
        let item = catalog
            .get(&pref.id)
            .ok_or_else(|| format!("prerequisite {:?} is not in the catalog", pref.id))?;
        if item.package(target.os, target.arch).is_none() {
            return Err(format!(
                "{} has no package for {target}; add a condition excluding {} or remove it",
                item.name,
                target.os.name()
            ));
        }
        let resolved = if pref.acquisition == Acquisition::Custom {
            let first = pref
                .custom_sources
                .first()
                .ok_or("custom acquisition without sources")?;
            let hash = ContentHash::parse(&first.hash).map_err(|e| e.to_string())?;
            let pkg = item.package(target.os, target.arch).expect("checked");
            ResolvedPackage {
                id: item.id.clone(),
                name: item.name.clone(),
                version: pref.version.to_string(),
                os: target.os,
                arch: target.arch,
                file: pkg.file,
                file_name: first.url.rsplit('/').next().unwrap_or(&item.id).to_owned(),
                urls: pref.custom_sources.iter().map(|s| s.url.clone()).collect(),
                hash,
                size: first.size,
                detection: pkg.detection.clone(),
                install: pkg.install.clone(),
                signature: pkg.signature.clone(),
                requires_admin: item.requires_admin.get(target.os),
                redundancy: inst_catalog::Redundancy::Degraded {
                    routes: pref.custom_sources.len(),
                },
                integrity_source: "developer-supplied hash",
            }
        } else {
            resolve::resolve(item, target.os, target.arch, &pref.version, fetcher)
                .map_err(|e| e.to_string())?
        };
        let (source, embedded_file) = match pref.acquisition {
            Acquisition::Embedded => {
                let urls: Vec<&str> = resolved.urls.iter().map(String::as_str).collect();
                let req = DownloadRequest {
                    sources: &urls,
                    expected_hash: resolved.hash,
                    expected_size: resolved.size,
                };
                let transport =
                    UreqTransport::new(&TransportConfig::default()).map_err(|e| e.to_string())?;
                let mut downloader = Downloader::new(&transport, RetryPolicy::default());
                let info = ItemInfo {
                    vendor: item.vendor.clone(),
                    product: item.name.clone(),
                    version: resolved.version.clone(),
                    arch: format!("{:?}", target.arch),
                    platform: target.os.name().to_owned(),
                    file_name: resolved.file_name.clone(),
                };
                let path = cache
                    .fetch(
                        &mut downloader,
                        &req,
                        &info,
                        &mut inst_fetch::download::NoopObserver,
                    )
                    .map_err(|e| e.to_string())?;
                let payload_path = format!(".prerequisites/{}", resolved.file_name);
                (
                    PrereqSource::Embedded {
                        payload_path: payload_path.clone(),
                    },
                    Some((payload_path, path, resolved.size)),
                )
            }
            Acquisition::Automatic | Acquisition::Custom => (
                PrereqSource::Download {
                    urls: resolved.urls.clone(),
                    hash: resolved.hash.to_string(),
                    size: resolved.size,
                },
                None,
            ),
        };
        let report = format!(
            "{} {} ({}, {}, {} source(s){})",
            resolved.name,
            resolved.version,
            match pref.acquisition {
                Acquisition::Embedded => "embedded",
                Acquisition::Custom => "custom sources",
                Acquisition::Automatic => "downloaded when missing",
            },
            resolved.integrity_source,
            resolved.urls.len(),
            if resolved.redundancy.is_degraded() {
                ", source redundancy degraded"
            } else {
                ""
            }
        );
        out.push(ResolvedPrereq {
            input: input_from(&resolved, pref, source)?,
            embedded_file,
            report,
        });
    }
    Ok(out)
}

/// Default Studio cache directory.
pub fn studio_cache_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cache")))
    };
    base.unwrap_or_else(std::env::temp_dir)
        .join(inst_brand::STUDIO_DIR_NAME)
}
