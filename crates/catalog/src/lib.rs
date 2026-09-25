//! Smart Prerequisites catalog.
//!
//! * [`schema`] — the extensible TOML format (built-in + user catalogs).
//! * [`Catalog`] — lookup, source-redundancy evaluation and privilege facts.
//! * [`resolve`] — build-time resolution of a catalog entry into a
//!   [`ResolvedPackage`]: exact version, concrete URLs, hash and size. Only
//!   resolved data is compiled into an installer; the runtime never parses
//!   catalogs.

pub mod resolve;
pub mod schema;

use std::collections::BTreeSet;
use std::path::Path;

use inst_model::platform::{Arch, Os};
use inst_model::privileges::PrerequisiteFacts;

pub use schema::*;

const BUILTIN: &str = include_str!("../builtin.toml");

/// Minimum number of independent acquisition routes for full redundancy.
pub const REQUIRED_ROUTES: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Redundancy {
    /// At least [`REQUIRED_ROUTES`] independent official routes.
    Full { routes: usize },
    /// Fewer routes: "Source redundancy degraded".
    Degraded { routes: usize },
    /// No automatic download possible (no verifiable integrity metadata).
    Unavailable,
}

impl Redundancy {
    pub fn is_degraded(&self) -> bool {
        !matches!(self, Redundancy::Full { .. })
    }
}

#[derive(Debug)]
pub enum CatalogError {
    Parse { origin: String, message: String },
    Io(std::io::Error),
    Duplicate(String),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogError::Parse { origin, message } => write!(f, "{origin}: {message}"),
            CatalogError::Io(e) => write!(f, "{e}"),
            CatalogError::Duplicate(id) => write!(f, "catalog item {id:?} is defined twice"),
        }
    }
}

impl std::error::Error for CatalogError {}

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    items: Vec<CatalogItem>,
    /// Origin of each item (`builtin` or a file path), parallel to `items`.
    origins: Vec<String>,
}

impl Catalog {
    /// The built-in catalog.
    pub fn builtin() -> Catalog {
        let mut c = Catalog::default();
        c.merge_str(BUILTIN, "builtin")
            .expect("built-in catalog is valid (checked by tests)");
        c
    }

    /// Built-in catalog plus every `*.toml` in `dir`. User items with the
    /// same id replace built-in ones (so a company can override sources).
    pub fn with_user_dir(dir: &Path) -> Result<Catalog, CatalogError> {
        let mut c = Catalog::builtin();
        let mut files: Vec<_> = match std::fs::read_dir(dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(CatalogError::Io(e)),
        };
        files.sort();
        for f in files {
            let text = std::fs::read_to_string(&f).map_err(CatalogError::Io)?;
            c.merge_str(&text, &f.display().to_string())?;
        }
        Ok(c)
    }

    /// Adds items from TOML text. Items from a later origin override earlier
    /// ones with the same id; duplicates within one origin are an error.
    pub fn merge_str(&mut self, text: &str, origin: &str) -> Result<(), CatalogError> {
        let file: CatalogFile = toml::from_str(text).map_err(|e| CatalogError::Parse {
            origin: origin.to_owned(),
            message: e.to_string(),
        })?;
        let mut seen = BTreeSet::new();
        for item in file.items {
            if !seen.insert(item.id.clone()) {
                return Err(CatalogError::Duplicate(item.id));
            }
            match self.items.iter().position(|i| i.id == item.id) {
                Some(i) => {
                    self.items[i] = item;
                    self.origins[i] = origin.to_owned();
                }
                None => {
                    self.items.push(item);
                    self.origins.push(origin.to_owned());
                }
            }
        }
        Ok(())
    }

    pub fn items(&self) -> &[CatalogItem] {
        &self.items
    }

    pub fn get(&self, id: &str) -> Option<&CatalogItem> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn origin(&self, id: &str) -> Option<&str> {
        self.items
            .iter()
            .position(|i| i.id == id)
            .map(|i| self.origins[i].as_str())
    }

    /// Case-insensitive search over id, name, vendor and description.
    pub fn search<'a>(&'a self, query: &str) -> impl Iterator<Item = &'a CatalogItem> + 'a {
        let q = query.to_lowercase();
        self.items.iter().filter(move |i| {
            q.is_empty()
                || [&i.id, &i.name, &i.vendor, &i.description]
                    .iter()
                    .any(|s| s.to_lowercase().contains(&q))
        })
    }
}

/// Counts independent routes: distinct hosts among official sources.
pub fn redundancy(package: &Package) -> Redundancy {
    if matches!(package.integrity, IntegrityStrategy::Unverifiable) || package.sources.is_empty() {
        return Redundancy::Unavailable;
    }
    let hosts: BTreeSet<&str> = package
        .sources
        .iter()
        .filter_map(|s| host(&s.url))
        .collect();
    let routes = hosts.len();
    if routes >= REQUIRED_ROUTES {
        Redundancy::Full { routes }
    } else {
        Redundancy::Degraded { routes }
    }
}

/// Host part of an `https://` URL.
pub fn host(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = &rest[..end];
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    Some(host.split(':').next().unwrap_or(host))
}

impl PrerequisiteFacts for Catalog {
    fn requires_admin(&self, id: &str, os: Os) -> Option<bool> {
        self.get(id).map(|i| i.requires_admin.get(os))
    }
}

/// Convenience used by analyzers: packages available for a target.
pub fn supports(item: &CatalogItem, os: Os, arch: Arch) -> bool {
    item.package(os, arch).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalog_is_valid_and_complete() {
        let c = Catalog::builtin();
        for id in [
            "dotnet-desktop-runtime",
            "dotnet-runtime",
            "aspnet-runtime",
            "vc-redist",
            "webview2",
            "postgresql",
            "mysql",
            "mariadb",
            "sql-server-express",
            "java-runtime",
            "nodejs",
            "python",
            "git",
            "redis-compatible",
            "nssm",
        ] {
            let item = c.get(id).unwrap_or_else(|| panic!("{id} missing"));
            assert!(!item.packages.is_empty(), "{id} has no packages");
            assert!(!item.channels.is_empty(), "{id} has no channels");
            for p in &item.packages {
                for s in &p.sources {
                    assert!(s.url.starts_with("https://"), "{id}: {}", s.url);
                }
            }
        }
    }

    #[test]
    fn redundancy_is_reported_honestly() {
        let c = Catalog::builtin();
        let mysql = c
            .get("mysql")
            .and_then(|i| i.package(Os::Windows, Arch::X64))
            .expect("pkg");
        assert_eq!(redundancy(mysql), Redundancy::Full { routes: 3 });
        let vc = c
            .get("vc-redist")
            .and_then(|i| i.package(Os::Windows, Arch::X64))
            .expect("pkg");
        assert_eq!(redundancy(vc), Redundancy::Degraded { routes: 1 });
        let nssm = c
            .get("nssm")
            .and_then(|i| i.package(Os::Windows, Arch::X64))
            .expect("pkg");
        assert_eq!(redundancy(nssm), Redundancy::Unavailable);
    }

    #[test]
    fn user_catalog_overrides_builtin() {
        let mut c = Catalog::builtin();
        let n = c.items().len();
        c.merge_str(
            r#"
[[item]]
id = "nodejs"
name = "Node.js (company mirror)"
vendor = "OpenJS Foundation"
homepage = "https://nodejs.org"
license = "MIT"
category = "runtime"
channels = ["22"]
"#,
            "company.toml",
        )
        .expect("merge");
        assert_eq!(c.items().len(), n);
        assert_eq!(
            c.get("nodejs").map(|i| i.name.as_str()),
            Some("Node.js (company mirror)")
        );
        assert_eq!(c.origin("nodejs"), Some("company.toml"));
    }

    #[test]
    fn hosts() {
        assert_eq!(host("https://a.b.com/x?y"), Some("a.b.com"));
        assert_eq!(host("https://u:p@a.b.com:8443/x"), Some("a.b.com"));
        assert_eq!(host("http://a.com/"), None);
    }

    #[test]
    fn privilege_facts() {
        let c = Catalog::builtin();
        assert_eq!(c.requires_admin("python", Os::Windows), Some(false));
        assert_eq!(c.requires_admin("vc-redist", Os::Windows), Some(true));
        assert_eq!(c.requires_admin("unknown", Os::Windows), None);
    }
}
