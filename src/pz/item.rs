//! Workshop item inspection: which mods an item dir bundles, from their
//! mod.info manifests.
//!
//! A folder under `mods/` counts as a mod when it holds a mod.info
//! directly (Build 41 shape) or in an immediate subdirectory (Build 42
//! version folders or `common/`). Manifests are the single source of
//! truth; version folder names are never read as version data (see
//! docs/research/pz-modding.md §5.3).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::mod_info::{self, ModInfo};
use super::version::Version;

/// Which build layouts a mod's manifests were found in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Layout {
    /// Root `mod.info` only — the Build 41 shape.
    B41,
    /// Version/`common` subfolder manifests only — the Build 42 shape.
    B42,
    /// Both layouts in one folder — one item serving both builds.
    Hybrid,
}

/// One mod bundled in a workshop item, aggregated from its mod.info
/// manifests. This exact type is serialized into the store's mod.json
/// sidecar, so the capture path and the file format cannot drift.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEntry {
    /// Folder name under `mods/`.
    pub name: String,
    /// `id=` of the manifest the game would read: the Mod ID the game
    /// keys activation and dependencies on.
    #[serde(default)]
    pub mod_id: Option<String>,
    /// `name=` of that manifest.
    #[serde(default)]
    pub title: Option<String>,
    /// `author=` of that manifest.
    #[serde(default)]
    pub author: Option<String>,
    /// Lowest `versionMin` across all manifests; None = unbounded.
    #[serde(default)]
    pub version_min: Option<String>,
    /// Highest `versionMax` across all manifests; None = unbounded.
    #[serde(default)]
    pub version_max: Option<String>,
    /// Version range inferred from the version-folder names, captured
    /// always but shown only when the manifests declare nothing. A
    /// packaging hint, not a compatibility claim — folder names lie
    /// (docs/research/pz-modding.md §5.3).
    #[serde(default)]
    pub inferred_min: Option<String>,
    #[serde(default)]
    pub inferred_max: Option<String>,
    /// `require=` Mod IDs, unioned across manifests.
    #[serde(default)]
    pub requires: Vec<String>,
    /// `incompatible=` Mod IDs, unioned across manifests.
    #[serde(default)]
    pub incompatible: Vec<String>,
    /// `loadModBefore=` Mod IDs, unioned across manifests.
    #[serde(default)]
    pub load_before: Vec<String>,
    /// `loadModAfter=` Mod IDs, unioned across manifests.
    #[serde(default)]
    pub load_after: Vec<String>,
    pub layout: Layout,
}

/// The mods bundled in a workshop item dir, sorted by folder name.
pub fn inspect_item(item_dir: &Path) -> Vec<ModEntry> {
    let mut mods = Vec::new();
    let Ok(entries) = fs::read_dir(item_dir.join("mods")) else {
        return mods;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(mod_entry) = inspect_mod(&name, &dir) {
            mods.push(mod_entry);
        }
    }
    mods.sort_by(|a, b| a.name.cmp(&b.name));
    mods
}

fn inspect_mod(name: &str, dir: &Path) -> Option<ModEntry> {
    let root = dir.join("mod.info");
    let root = root.is_file().then_some(root);

    let mut common = None;
    let mut versions: Vec<(u64, String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let sub = entry.path();
        let info = sub.join("mod.info");
        if !info.is_file() {
            continue;
        }
        if sub.file_name().is_some_and(|n| n == "common") {
            common = Some(info);
        } else {
            let folder = sub
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let key = version_dir_int(&folder);
            versions.push((key, folder, info));
        }
    }
    versions.sort_by_key(|(key, _, _)| *key);

    if root.is_none() && common.is_none() && versions.is_empty() {
        return None;
    }

    // The manifest the game would read: the lowest compatible version
    // folder (its selection rule, floor 42.0), then `common/`, then the
    // B41 root manifest.
    let floor = 42 * 1000;
    let identity = versions
        .iter()
        .find(|(key, _, _)| *key >= floor)
        .or(versions.first())
        .map(|(_, _, path)| path)
        .or(common.as_ref())
        .or(root.as_ref());

    let manifests: Vec<PathBuf> = root
        .iter()
        .chain(common.iter())
        .chain(versions.iter().map(|(_, _, path)| path))
        .cloned()
        .collect();
    let infos: Vec<ModInfo> = manifests.iter().map(|path| mod_info::parse(path)).collect();
    let identity_info = identity.map(|path| mod_info::parse(path));

    let mins: Vec<&Version> = infos
        .iter()
        .filter_map(|i| i.version_min.as_ref())
        .collect();
    let maxs: Vec<&Version> = infos
        .iter()
        .filter_map(|i| i.version_max.as_ref())
        .collect();

    // Version range inferred from the version-folder names, using the
    // game's own name→version mapping. Folders below the game's 42.0
    // floor are never selected by the B42 loader, so they do not bound
    // the hint.
    let packaged: Vec<&str> = versions
        .iter()
        .filter(|(key, _, _)| *key >= floor)
        .map(|(_, folder, _)| folder.as_str())
        .collect();
    let (inferred_min, inferred_max) = match (packaged.first(), packaged.last()) {
        (Some(min), Some(max)) => (Some((*min).to_owned()), Some((*max).to_owned())),
        _ => (None, None),
    };

    Some(ModEntry {
        name: name.to_owned(),
        mod_id: identity_info.as_ref().and_then(|i| i.id.clone()),
        title: identity_info.as_ref().and_then(|i| i.name.clone()),
        author: identity_info.as_ref().and_then(|i| i.author.clone()),
        version_min: mins.iter().min().map(|v| v.text().to_owned()),
        version_max: maxs.iter().max().map(|v| v.text().to_owned()),
        inferred_min,
        inferred_max,
        requires: union(infos.iter().map(|i| &i.requires)),
        incompatible: union(infos.iter().map(|i| &i.incompatible)),
        load_before: union(infos.iter().map(|i| &i.load_before)),
        load_after: union(infos.iter().map(|i| &i.load_after)),
        layout: match (root.is_some(), common.is_some() || !versions.is_empty()) {
            (true, true) => Layout::Hybrid,
            (true, false) => Layout::B41,
            (false, _) => Layout::B42,
        },
    })
}

/// Union of ID lists across manifests, preserving first-seen order.
fn union<'a>(lists: impl Iterator<Item = &'a Vec<String>>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for list in lists {
        for id in list {
            if !out.contains(id) {
                out.push(id.clone());
            }
        }
    }
    out
}

/// The game's version-folder key: `major * 1000 + min(minor, 999)`,
/// split on '.'; the third component is never read and non-numeric
/// parts count as 0 (docs/research/mod-resolution.md §3).
fn version_dir_int(name: &str) -> u64 {
    let mut parts = name.trim().split('.');
    let major: u64 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor: u64 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    major * 1000 + minor.min(999)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, relative: &str, text: &str) {
        fs::create_dir_all(dir.join(relative).parent().unwrap()).unwrap();
        fs::write(dir.join(relative), text).unwrap();
    }

    #[test]
    fn recognizes_b41_b42_and_hybrid() {
        let base = std::env::temp_dir().join(format!("snapoid-item-{}", std::process::id()));

        // B41: root manifest only.
        let item = base.join("b41");
        write(&item, "mods/Old Mod/mod.info", "name=Old Mod\nid=OM\n");
        // B42: version folders plus common.
        let item = base.join("b42");
        write(
            &item,
            "mods/New Mod/42.16/mod.info",
            "name=New Mod\nid=NM\nversionMin=42.16\n",
        );
        write(
            &item,
            "mods/New Mod/common/mod.info",
            "name=New Mod\nid=NM\n",
        );
        // Hybrid: root manifest plus a version folder.
        let item = base.join("hybrid");
        write(&item, "mods/Both/mod.info", "name=Both\nid=B\n");
        write(
            &item,
            "mods/Both/42.5/mod.info",
            "name=Both\nid=B\nversionMin=42.5\n",
        );
        // Not a mod: no manifest anywhere.
        let item = base.join("empty");
        write(&item, "mods/Not A Mod/readme.txt", "hi\n");

        assert_eq!(inspect_item(&base.join("b41"))[0].layout, Layout::B41);
        assert_eq!(inspect_item(&base.join("b42"))[0].layout, Layout::B42);
        assert_eq!(inspect_item(&base.join("hybrid"))[0].layout, Layout::Hybrid);
        assert!(inspect_item(&base.join("empty")).is_empty());

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn identity_comes_from_the_lowest_compatible_version_folder() {
        let base = std::env::temp_dir().join(format!("snapoid-item-id-{}", std::process::id()));
        let item = base.join("item");
        write(
            &item,
            "mods/M/42.16/mod.info",
            "name=M 16\nid=M16\nauthor=A\n",
        );
        write(&item, "mods/M/42.22/mod.info", "name=M 22\nid=M22\n");
        write(&item, "mods/M/common/mod.info", "name=M common\nid=MC\n");

        let mods = inspect_item(&item);
        assert_eq!(mods.len(), 1);
        // The game picks the lowest compatible folder; its manifest is
        // the identity source.
        assert_eq!(mods[0].mod_id.as_deref(), Some("M16"));
        assert_eq!(mods[0].title.as_deref(), Some("M 16"));
        assert_eq!(mods[0].author.as_deref(), Some("A"));

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn versions_aggregate_and_ids_union_across_manifests() {
        let base = std::env::temp_dir().join(format!("snapoid-item-agg-{}", std::process::id()));
        let item = base.join("item");
        write(
            &item,
            "mods/M/42.16/mod.info",
            "name=M\nid=M\nversionMin=42.16\nrequire=\\Alpha, Beta\n",
        );
        write(
            &item,
            "mods/M/42.22/mod.info",
            "name=M\nid=M\nversionMin=42.10\nversionMax=42.99\nrequire=Beta, Gamma\nloadModAfter=Late\n",
        );

        let mods = inspect_item(&item);
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].version_min.as_deref(), Some("42.10"));
        assert_eq!(mods[0].version_max.as_deref(), Some("42.99"));
        assert_eq!(mods[0].requires, vec!["Alpha", "Beta", "Gamma"]);
        assert_eq!(mods[0].load_after, vec!["Late"]);

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn version_folder_names_are_not_version_data() {
        let base = std::env::temp_dir().join(format!("snapoid-item-vm-{}", std::process::id()));
        let item = base.join("item");
        // Folder names lie: 42.0.5 truncates to 42.0, and a manifest
        // without version fields means unbounded regardless of layout.
        write(&item, "mods/M/42.0.5/mod.info", "name=M\nid=M\n");
        write(&item, "mods/M/43/mod.info", "name=M\nid=M\n");

        let mods = inspect_item(&item);
        assert_eq!(mods[0].version_min, None);
        assert_eq!(mods[0].version_max, None);
        assert_eq!(mods[0].layout, Layout::B42);

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn infers_range_from_folder_names_when_manifests_are_silent() {
        let base = std::env::temp_dir().join(format!("snapoid-item-inf-{}", std::process::id()));
        let item = base.join("item");
        // Folders below the game's 42.0 floor (B41-era packaging) do
        // not bound the hint; names are kept as written.
        write(&item, "mods/M/41/mod.info", "name=M\nid=M\n");
        write(&item, "mods/M/42.16/mod.info", "name=M\nid=M\n");
        write(&item, "mods/M/42.22/mod.info", "name=M\nid=M\n");

        let mods = inspect_item(&item);
        assert_eq!(mods[0].version_min, None);
        assert_eq!(mods[0].version_max, None);
        assert_eq!(mods[0].inferred_min.as_deref(), Some("42.16"));
        assert_eq!(mods[0].inferred_max.as_deref(), Some("42.22"));

        // A single folder bounds both ends with its own name.
        let item = base.join("single");
        write(&item, "mods/S/42.5/mod.info", "name=S\nid=S\n");
        let mods = inspect_item(&item);
        assert_eq!(mods[0].inferred_min.as_deref(), Some("42.5"));
        assert_eq!(mods[0].inferred_max.as_deref(), Some("42.5"));

        // Declared fields are captured independently of the hint.
        let item = base.join("declared");
        write(
            &item,
            "mods/D/42.16/mod.info",
            "name=D\nid=D\nversionMin=42.16\n",
        );
        let mods = inspect_item(&item);
        assert_eq!(mods[0].version_min.as_deref(), Some("42.16"));
        assert_eq!(mods[0].inferred_min.as_deref(), Some("42.16"));

        // No version folders at all (B41 layout): no hint.
        let item = base.join("b41");
        write(&item, "mods/O/mod.info", "name=O\nid=O\n");
        let mods = inspect_item(&item);
        assert_eq!(mods[0].inferred_min, None);
        assert_eq!(mods[0].inferred_max, None);

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn missing_mods_dir_is_empty() {
        assert!(inspect_item(Path::new("nonexistent")).is_empty());
    }
}
