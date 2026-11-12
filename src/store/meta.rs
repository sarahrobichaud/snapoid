//! Metadata sidecars (game.json / instance.json / mod.json).

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::pz::item::{Layout, ModEntry};

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

/// How a stored workshop copy was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// Copied from the local Steam workshop dir.
    WorkshopDisk,
    /// Downloaded with steamcmd and adopted into the store.
    Steamcmd,
}

/// Sidecar of a stored workshop mod.
#[derive(Debug, Serialize, Deserialize)]
pub struct ModMeta {
    pub id: u64,
    /// Steam's timeupdated for the item at capture time; 0 when unknown.
    #[serde(default)]
    pub time_updated: u64,
    /// Unix seconds at creation time.
    pub created: u64,
    /// Workshop item title from the Steam Web API, when known.
    #[serde(default)]
    pub title: Option<String>,
    /// How the copy was obtained.
    #[serde(default)]
    pub source: Option<Source>,
    /// Mods bundled in this item, captured at save time.
    #[serde(default)]
    pub mods: Vec<ModEntry>,
    /// Layout of the bundled mods, when they share one.
    #[serde(default)]
    pub layout: Option<Layout>,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Sidecars written before the enriched mod metadata must keep
    /// parsing: every added field defaults.
    #[test]
    fn old_mod_meta_parses() {
        let text = r#"{
            "id": 123,
            "time_updated": 1700000000,
            "created": 1700000001
        }"#;
        let meta: ModMeta = serde_json::from_str(text).unwrap();
        assert_eq!(meta.id, 123);
        assert_eq!(meta.time_updated, 1_700_000_000);
        assert_eq!(meta.title, None);
        assert_eq!(meta.source, None);
        assert!(meta.mods.is_empty());
        assert_eq!(meta.layout, None);
    }

    #[test]
    fn mod_meta_roundtrip() {
        let meta = ModMeta {
            id: 123,
            time_updated: 1_700_000_000,
            created: 1_700_000_001,
            title: Some("A Mod Pack".to_owned()),
            source: Some(Source::Steamcmd),
            mods: vec![ModEntry {
                name: "Test Mod".to_owned(),
                mod_id: Some("TM".to_owned()),
                title: Some("Test Mod".to_owned()),
                author: Some("Someone".to_owned()),
                version_min: Some("42.16".to_owned()),
                version_max: None,
                inferred_min: Some("42.16".to_owned()),
                inferred_max: Some("42.22".to_owned()),
                requires: vec!["Lib".to_owned()],
                incompatible: Vec::new(),
                load_before: Vec::new(),
                load_after: Vec::new(),
                layout: Layout::B42,
            }],
            layout: Some(Layout::B42),
        };
        let text = serde_json::to_string(&meta).unwrap();
        let parsed: ModMeta = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.id, meta.id);
        assert_eq!(parsed.title.as_deref(), Some("A Mod Pack"));
        assert_eq!(parsed.source, Some(Source::Steamcmd));
        assert_eq!(parsed.mods.len(), 1);
        assert_eq!(parsed.mods[0].mod_id.as_deref(), Some("TM"));
        assert_eq!(parsed.mods[0].requires, vec!["Lib"]);
        assert_eq!(parsed.mods[0].inferred_min.as_deref(), Some("42.16"));
        assert_eq!(parsed.mods[0].inferred_max.as_deref(), Some("42.22"));
        assert_eq!(parsed.mods[0].layout, Layout::B42);
        assert_eq!(parsed.layout, Some(Layout::B42));
    }
}
