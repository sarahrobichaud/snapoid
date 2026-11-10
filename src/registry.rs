//! Component 9: Version registry.
//!
//! Maps game versions to depot manifest IDs. The authoritative source is
//! a registry file fetched from GitHub on every load, so new builds can be
//! added without releasing a new tool version. When offline, loading
//! fails.
//!
//! The registry file is JSON:
//!
//! ```json
//! { "builds": [ { "version": "42.22.2", "manifest": 2378094061158458897 } ] }
//! ```

use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::debug;

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
    /// Depot ID for `download_depot`, when known.
    pub depot: Option<u64>,
}

#[derive(Debug)]
pub struct Registry {
    pub entries: Vec<Entry>,
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

/// Load the registry from GitHub.
pub fn load() -> Result<Registry> {
    let url = registry_url();
    Ok(Registry {
        entries: fetch(&url)?,
    })
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
    let file: RegistryFile = serde_json::from_str(text).context("invalid registry JSON")?;
    Ok(file
        .builds
        .into_iter()
        .filter(|build| !build.version.is_empty())
        .map(|build| Entry {
            version: build.version,
            manifest: build.manifest,
            depot: build.depot,
        })
        .collect())
}

#[derive(Serialize, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    builds: Vec<FileEntry>,
}

#[derive(Serialize, Deserialize)]
struct FileEntry {
    version: String,
    manifest: u64,
    #[serde(default)]
    depot: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(version: &str, manifest: u64) -> Entry {
        Entry {
            version: version.to_owned(),
            manifest,
            depot: None,
        }
    }

    #[test]
    fn parses_registry_file() {
        let text = r#"{
            "builds": [
                { "version": "42.22.2", "manifest": 2378094061158458897 },
                { "version": "", "manifest": 1 },
                { "version": "42.21.0", "manifest": 7544133343391543313, "depot": 108601 }
            ]
        }"#;
        let entries = parse(text).unwrap();
        assert_eq!(
            entries,
            vec![
                entry("42.22.2", 2378094061158458897),
                Entry {
                    version: "42.21.0".to_owned(),
                    manifest: 7544133343391543313,
                    depot: Some(108601),
                },
            ]
        );
    }

    #[test]
    fn rejects_malformed_registry_file() {
        assert!(parse("not json").is_err());
        assert!(parse(r#"{ "builds": [{ "version": "1.0" }] }"#).is_err());
    }
}
