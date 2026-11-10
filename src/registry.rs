//! Component 9: Version registry.
//!
//! Maps game versions to depot manifest IDs. The authoritative source is
//! a registry file fetched from GitHub on every load, so new builds can be
//! added without releasing a new tool version. The fetched file is cached
//! locally; when the fetch fails, the last cache is used, so lookups keep
//! working offline. With neither network nor cache, loading fails.
//!
//! The registry file is JSON:
//!
//! ```json
//! { "builds": [ { "version": "42.22.2", "manifest": 2378094061158458897 } ] }
//! ```

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{debug, warn};

/// Where the registry file is fetched from; override with the
/// SNAPOID_REGISTRY_URL environment variable.
const DEFAULT_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/sarahrobichaud/snapoid/main/registry.json";

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Version as reported by the probe, e.g. "42.22.2".
    pub version: String,
    /// Depot manifest ID for `download_depot`.
    pub manifest: u64,
}

/// Where the loaded entries came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Fetched from GitHub during this load.
    Remote,
    /// Served from the local cache (fetch failed).
    Cache,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Source::Remote => "remote",
            Source::Cache => "cache",
        }
    }
}

#[derive(Debug)]
pub struct Registry {
    pub entries: Vec<Entry>,
    pub source: Source,
}

impl Registry {
    /// Look up the manifest ID for a version string.
    pub fn manifest_for(&self, version: &str) -> Option<u64> {
        self.entries
            .iter()
            .find(|entry| entry.version == version)
            .map(|entry| entry.manifest)
    }
}

/// Load the registry, always fetching from GitHub first. On failure,
/// fall back to the last cached file; with neither network nor cache,
/// return an error.
pub fn load() -> Result<Registry> {
    match fetch_and_cache() {
        Ok(remote) => Ok(Registry {
            entries: remote,
            source: Source::Remote,
        }),
        Err(error) => {
            if let Some(cache) = read_cache() {
                warn!("registry fetch failed ({error:#}); using cache");
                Ok(Registry {
                    entries: cache.builds,
                    source: Source::Cache,
                })
            } else {
                Err(error.context("registry fetch failed and no cache exists"))
            }
        }
    }
}

/// Force a refetch from GitHub, updating the local cache. Unlike
/// [`load`], this never falls back to the cache.
pub fn refresh() -> Result<Registry> {
    let remote = fetch_and_cache()?;
    Ok(Registry {
        entries: remote,
        source: Source::Remote,
    })
}

fn fetch_and_cache() -> Result<Vec<Entry>> {
    let url = registry_url();
    let remote = fetch(&url)?;
    write_cache(&remote)
        .with_context(|| format!("caching the registry after fetching {url}"))?;
    Ok(remote)
}

fn registry_url() -> String {
    std::env::var("SNAPOID_REGISTRY_URL").unwrap_or_else(|_| DEFAULT_REGISTRY_URL.to_owned())
}

fn fetch(url: &str) -> Result<Vec<Entry>> {
    debug!("registry: fetching {url}");
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .build()
        .new_agent();
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("fetching {url}"))?;
    let text = response
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading the response from {url}"))?;
    parse(&text).with_context(|| format!("parsing the registry from {url}"))
}

/// Parse a registry file. Entries with an empty version are skipped.
fn parse(text: &str) -> Result<Vec<Entry>> {
    let file: RegistryFile =
        serde_json::from_str(text).context("invalid registry JSON")?;
    Ok(file
        .builds
        .into_iter()
        .filter(|build| !build.version.is_empty())
        .map(|build| Entry {
            version: build.version,
            manifest: build.manifest,
        })
        .collect())
}

struct Cache {
    fetched: u64,
    builds: Vec<Entry>,
}

fn read_cache() -> Option<Cache> {
    let text = std::fs::read_to_string(cache_path()?).ok()?;
    let file: CacheFile = serde_json::from_str(&text).ok()?;
    Some(Cache {
        fetched: file.fetched,
        builds: file
            .builds
            .into_iter()
            .map(|build| Entry {
                version: build.version,
                manifest: build.manifest,
            })
            .collect(),
    })
}

fn write_cache(entries: &[Entry]) -> Result<()> {
    let path = cache_path().context("no cache directory available")?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = CacheFile {
        fetched: unix_now(),
        builds: entries
            .iter()
            .map(|entry| FileEntry {
                version: entry.version.clone(),
                manifest: entry.manifest,
            })
            .collect(),
    };
    std::fs::write(&path, serde_json::to_string_pretty(&file)?)
        .with_context(|| format!("writing {}", path.display()))?;
    debug!("registry cached to {}", path.display());
    Ok(())
}

/// Cache location, e.g. ~/.cache/snapoid/registry.json (Linux/WSL) or
/// %LocalAppData%\snapoid\registry.json (Windows).
fn cache_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("snapoid/registry.json"))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

#[derive(Serialize, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    builds: Vec<FileEntry>,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    fetched: u64,
    builds: Vec<FileEntry>,
}

#[derive(Serialize, Deserialize)]
struct FileEntry {
    version: String,
    manifest: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(version: &str, manifest: u64) -> Entry {
        Entry {
            version: version.to_owned(),
            manifest,
        }
    }

    #[test]
    fn parses_registry_file() {
        let text = r#"{
            "builds": [
                { "version": "42.22.2", "manifest": 2378094061158458897 },
                { "version": "", "manifest": 1 },
                { "version": "42.21.0", "manifest": 7544133343391543313 }
            ]
        }"#;
        let entries = parse(text).unwrap();
        assert_eq!(
            entries,
            vec![
                entry("42.22.2", 2378094061158458897),
                entry("42.21.0", 7544133343391543313),
            ]
        );
    }

    #[test]
    fn rejects_malformed_registry_file() {
        assert!(parse("not json").is_err());
        assert!(parse(r#"{ "builds": [{ "version": "1.0" }] }"#).is_err());
    }
}
