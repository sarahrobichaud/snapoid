//! Version registry: maps game versions to depot manifest IDs. The
//! authoritative source is a registry file fetched from GitHub on every
//! load, so new builds can be added without releasing a new tool version.
//!
//! ```json
//! { "builds": [ { "version": "42.22.2", "manifest": 2378094061158458897 } ] }
//! ```

use std::time::Duration;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::config;
use crate::fsops::unix_now;

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Version as reported by the probe, e.g. "42.22.2".
    pub version: String,
    /// Depot manifest ID for `download_depot`.
    pub manifest: u64,
    /// Depot ID for `download_depot`, when known.
    pub depot: Option<u64>,
    /// Steam build ID of the build, when known.
    pub buildid: Option<u64>,
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
    let url = cache_bust(&config::registry_url());
    Ok(Registry {
        entries: fetch(&url)?,
    })
}

/// raw.githubusercontent.com serves through a CDN keyed on the URL; a
/// one-minute-bucketed query parameter forces a fresh fetch.
fn cache_bust(url: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}t={}", unix_now() / 60)
}

fn fetch(url: &str) -> Result<Vec<Entry>> {
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
            buildid: build.buildid,
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
    #[serde(default)]
    buildid: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(version: &str, manifest: u64) -> Entry {
        Entry {
            version: version.to_owned(),
            manifest,
            depot: None,
            buildid: None,
        }
    }

    #[test]
    fn parses_registry_file() {
        let text = r#"{
            "builds": [
                { "version": "42.22.2", "manifest": 2378094061158458897 },
                { "version": "", "manifest": 1 },
                { "version": "42.21.0", "manifest": 7544133343391543313, "depot": 108604, "buildid": 25485521 }
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
                    depot: Some(108604),
                    buildid: Some(25485521),
                },
            ]
        );
    }

    #[test]
    fn rejects_malformed_registry_file() {
        assert!(parse("not json").is_err());
        assert!(parse(r#"{ "builds": [{ "version": "1.0" }] }"#).is_err());
    }

    #[test]
    fn cache_busts_the_url() {
        let busted = cache_bust("https://example.com/registry.json");
        assert!(busted.starts_with("https://example.com/registry.json?t="));
        let busted = cache_bust("https://example.com/registry.json?x=1");
        assert!(busted.contains("&t="));
    }
}
