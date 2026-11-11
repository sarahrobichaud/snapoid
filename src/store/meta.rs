//! Metadata sidecars (game.json / instance.json).

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

/// Sidecar of a stored game version.
#[derive(Debug, Serialize, Deserialize)]
pub struct GameMeta {
    pub version: String,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub buildid: Option<u64>,
    #[serde(default)]
    pub manifest: Option<u64>,
    /// Unix seconds at creation time.
    pub created: u64,
}

/// Sidecar of a game instance.
#[derive(Debug, Serialize, Deserialize)]
pub struct InstanceMeta {
    pub version: String,
    #[serde(default)]
    pub revision: Option<String>,
    /// Unix seconds at creation time.
    pub created: u64,
}

/// Read a sidecar; None when it does not exist.
pub fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write a sidecar.
pub fn write<T: Serialize>(path: &Path, meta: &T) -> Result<()> {
    serde_json::to_writer_pretty(
        fs::File::create(path).with_context(|| format!("writing {}", path.display()))?,
        meta,
    )
    .with_context(|| format!("writing {}", path.display()))
}
