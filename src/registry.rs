//! Version registry: maps game versions to depot manifest IDs, and
//! vouches for the steamcmd package the installer downloads.
//!
//! Every read and write of the registry file goes through this module —
//! the remote fetch from GitHub, the copy bundled with the binary
//! (fallback when the fetch fails), and maintainer-side load/save of the
//! file in the repo — all sharing one parser and one serializer, so the
//! file format cannot drift from the code.
//!
//! ```json
//! {
//!   "builds": [ { "version": "42.22.2", "manifest": 2378094061158458897 } ],
//!   "steamcmd": { "url": "https://.../steamcmd.zip", "sha256": "..." }
//! }
//! ```

use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::config;
use crate::fsops::unix_now;
use crate::warn;

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// The registry file bundled with the binary: the fallback when the
/// remote fetch fails, and the reference the tests keep in sync with the
/// parser.
const BUNDLED: &str = include_str!("../registry.json");

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

/// The steamcmd distribution the installer should trust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamcmdPackage {
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    pub entries: Vec<Entry>,
    /// The steamcmd package the registry vouches for, when listed.
    pub steamcmd: Option<SteamcmdPackage>,
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

/// Load the registry from GitHub, falling back to the bundled copy when
/// the fetch or parse fails.
pub fn load() -> Result<Registry> {
    let url = cache_bust(&config::registry_url());
    match fetch(&url) {
        Ok(registry) => Ok(registry),
        Err(error) => {
            warn!("could not load the registry from {url} ({error:#}); using the bundled copy");
            parse(BUNDLED).context("parsing the bundled registry")
        }
    }
}

/// Load a registry file from disk — the maintainer copy in the repo.
pub fn load_file(path: &Path) -> Result<Registry> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Write the registry in canonical file form: pretty JSON, trailing
/// newline, same shape the parser reads.
pub fn save_file(path: &Path, registry: &Registry) -> Result<()> {
    fs::write(path, to_json(registry)?).with_context(|| format!("writing {}", path.display()))
}

/// The canonical JSON form of a registry: what `save_file` writes and
/// what the parser reads back.
pub fn to_json(registry: &Registry) -> Result<String> {
    let file = RegistryFile {
        builds: registry
            .entries
            .iter()
            .map(|entry| FileEntry {
                version: entry.version.clone(),
                manifest: entry.manifest,
                depot: entry.depot,
                buildid: entry.buildid,
            })
            .collect(),
        steamcmd: registry.steamcmd.as_ref().map(|package| FileSteamcmd {
            url: package.url.clone(),
            sha256: package.sha256.clone(),
        }),
    };
    let mut text = serde_json::to_string_pretty(&file).context("serializing the registry")?;
    text.push('\n');
    Ok(text)
}

/// Best-effort cache busting: the CDN bounds staleness at max-age=300
/// regardless, and keys per Accept-Encoding variant (see `fetch`).
fn cache_bust(url: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}t={}", unix_now() / 60)
}

fn fetch(url: &str) -> Result<Registry> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .build()
        .new_agent();
    let mut response = agent
        .get(url)
        // The CDN caches per Accept-Encoding variant and does not honor
        // query-string cache busting; the gzip variant served stale
        // content long after pushes, so pin the identity variant.
        .header("accept-encoding", "identity")
        .call()
        .with_context(|| format!("fetching {url}"))?;
    let text = response
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading the response from {url}"))?;
    parse(&text).with_context(|| format!("parsing the registry from {url}"))
}

/// Parse a registry file. Entries with an empty version are skipped.
fn parse(text: &str) -> Result<Registry> {
    let file: RegistryFile = serde_json::from_str(text).context("invalid registry JSON")?;
    Ok(Registry {
        entries: file
            .builds
            .into_iter()
            .filter(|build| !build.version.is_empty())
            .map(|build| Entry {
                version: build.version,
                manifest: build.manifest,
                depot: build.depot,
                buildid: build.buildid,
            })
            .collect(),
        steamcmd: file.steamcmd.map(|package| SteamcmdPackage {
            url: package.url,
            sha256: package.sha256,
        }),
    })
}

#[derive(Serialize, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    builds: Vec<FileEntry>,
    #[serde(default)]
    steamcmd: Option<FileSteamcmd>,
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

#[derive(Serialize, Deserialize)]
struct FileSteamcmd {
    url: String,
    sha256: String,
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
            ],
            "steamcmd": {
                "url": "https://client-update.steamstatic.com/installer/steamcmd.zip",
                "sha256": "7669b170dee42db8ee2273775ed7dfb2d173bdba1b849f70d2c7b379290bce13"
            }
        }"#;
        let registry = parse(text).unwrap();
        assert_eq!(
            registry.entries,
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
        assert_eq!(
            registry.steamcmd,
            Some(SteamcmdPackage {
                url: "https://client-update.steamstatic.com/installer/steamcmd.zip".to_owned(),
                sha256: "7669b170dee42db8ee2273775ed7dfb2d173bdba1b849f70d2c7b379290bce13"
                    .to_owned(),
            })
        );
    }

    #[test]
    fn steamcmd_section_is_optional() {
        let text = r#"{ "builds": [ { "version": "42.22.2", "manifest": 1 } ] }"#;
        let registry = parse(text).unwrap();
        assert_eq!(registry.entries, vec![entry("42.22.2", 1)]);
        assert_eq!(registry.steamcmd, None);
    }

    #[test]
    fn rejects_malformed_registry_file() {
        assert!(parse("not json").is_err());
        assert!(parse(r#"{ "builds": [{ "version": "1.0" }] }"#).is_err());
    }

    /// The bundled copy must always parse: it is the offline fallback,
    /// and this is the guard against the file format drifting from the
    /// parser.
    #[test]
    fn bundled_registry_parses() {
        let registry = parse(BUNDLED).unwrap();
        assert!(!registry.entries.is_empty());
    }

    /// save_file's output must parse back into the same registry — the
    /// write path and the read path agree.
    #[test]
    fn save_and_load_roundtrip() -> Result<()> {
        let registry = Registry {
            entries: vec![
                entry("42.22.2", 2378094061158458897),
                Entry {
                    version: "42.21.0".to_owned(),
                    manifest: 7544133343391543313,
                    depot: Some(108604),
                    buildid: Some(25485521),
                },
            ],
            steamcmd: Some(SteamcmdPackage {
                url: "https://example.com/steamcmd.zip".to_owned(),
                sha256: "7669b170dee42db8ee2273775ed7dfb2d173bdba1b849f70d2c7b379290bce13"
                    .to_owned(),
            }),
        };

        let path =
            std::env::temp_dir().join(format!("snapoid-registry-{}.json", std::process::id()));
        save_file(&path, &registry)?;
        let loaded = load_file(&path)?;
        assert_eq!(loaded, registry);

        fs::remove_file(&path)?;
        Ok(())
    }

    #[test]
    fn cache_busts_the_url() {
        let busted = cache_bust("https://example.com/registry.json");
        assert!(busted.starts_with("https://example.com/registry.json?t="));
        let busted = cache_bust("https://example.com/registry.json?x=1");
        assert!(busted.contains("&t="));
    }
}
