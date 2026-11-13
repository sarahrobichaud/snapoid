//! Workshop item inspection: which mods an item dir bundles, from their
//! mod.info manifests.
//!
//! A folder under `mods/` counts as a mod when it holds a mod.info
//! directly (Build 41 shape) or in an immediate subdirectory (Build 42
//! version folders or `common/`). Manifests are the single source of
//! truth for identity and dependencies; version folder names are never
//! read as version data — but they are captured as the game's
//! selection key, since the game loads the highest version folder ≤
//! the running version and checks that folder's manifest range
//! (docs/research/mod-resolution.md §3–§4).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::mod_info::{self, ModInfo};
use super::version;

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

/// The versionMin/versionMax one manifest declares; a missing end is
/// unbounded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestRange {
    #[serde(default)]
    pub min: Option<String>,
    #[serde(default)]
    pub max: Option<String>,
}

/// One version folder, keyed by the folder's game-version int — the
/// key the game's folder selection runs on. Selection is by name only
/// (`getModVersionDirName` never checks for mod.info), and the
/// selected folder's content loads even when it ships no manifest —
/// the manifest is then read from `common/` as the fallback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderRange {
    /// `major * 1000 + min(minor, 999)` of the folder name.
    pub version: u64,
    /// The folder name as written.
    pub name: String,
    /// The folder's mod.info, when it ships one; its min/max are the
    /// declared range, when it declares one. None means the folder
    /// carries content only.
    #[serde(default)]
    pub manifest: Option<ManifestRange>,
}

/// One mod bundled in a workshop item, from its mod.info manifests.
/// This exact type is serialized into the store's mod.json sidecar, so
/// the capture path and the file format cannot drift.
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
    /// Declared ranges per version folder. The game selects the
    /// highest folder ≤ the running version and checks that folder's
    /// manifest range — never an aggregate across folders.
    #[serde(default)]
    pub folders: Vec<FolderRange>,
    /// The `common/` directory ships; its content loads on every B42
    /// version (overwritten by the selected version folder), manifest
    /// or not.
    #[serde(default)]
    pub common_dir: bool,
    /// The `common/` mod.info's declared range, when it ships one.
    #[serde(default)]
    pub common: Option<ManifestRange>,
    /// The B41 root manifest, when the mod ships one.
    #[serde(default)]
    pub root: Option<ManifestRange>,
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
    let root_path = dir.join("mod.info");
    let root_path = root_path.is_file().then_some(root_path);

    let mut common_path = None;
    let mut common_dir = false;
    // Every numeric-named subdirectory is a version folder candidate:
    // the game selects them by name alone, manifest or not.
    let mut versions: Vec<(u64, String, Option<PathBuf>)> = Vec::new();
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let sub = entry.path();
        if !sub.is_dir() {
            continue;
        }
        if sub.file_name().is_some_and(|n| n == "common") {
            // The directory's content loads even without a manifest;
            // only its mod.info (when present) declares a range.
            common_dir = true;
            let info = sub.join("mod.info");
            common_path = info.is_file().then_some(info);
            continue;
        }
        let folder = sub
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let key = version::game_int(&folder);
        if key == 0 {
            continue;
        }
        let info = sub.join("mod.info");
        versions.push((key, folder, info.is_file().then_some(info)));
    }
    versions.sort_by_key(|(key, _, _)| *key);

    // The game recognizes the mod through a manifest somewhere: the
    // root (B41), common/, or a version folder's own mod.info.
    let recognized = root_path.is_some()
        || common_path.is_some()
        || versions
            .iter()
            .any(|(_, _, info)| info.is_some());
    if !recognized {
        return None;
    }

    let root_info = root_path.as_ref().map(|path| mod_info::parse(path));
    let common_info = common_path.as_ref().map(|path| mod_info::parse(path));
    let folder_infos: Vec<(FolderRange, Option<ModInfo>)> = versions
        .into_iter()
        .map(|(key, folder, info_path)| {
            let manifest = info_path.map(|path| mod_info::parse(&path));
            let range = FolderRange {
                version: key,
                name: folder,
                manifest: manifest.as_ref().map(|info| ManifestRange {
                    min: info.version_min.as_ref().map(|v| v.text().to_owned()),
                    max: info.version_max.as_ref().map(|v| v.text().to_owned()),
                }),
            };
            (range, manifest)
        })
        .collect();

    // Identity comes from the manifests the game reads on current
    // builds — the highest version folder first, then `common/`, then
    // the B41 root manifest — with the first manifest providing each
    // field winning.
    let mut mod_id = None;
    let mut title = None;
    let mut author = None;
    for (_, manifest) in folder_infos.iter().rev() {
        if let Some(info) = manifest {
            fill_identity(&mut mod_id, &info.id);
            fill_identity(&mut title, &info.name);
            fill_identity(&mut author, &info.author);
        }
    }
    if let Some(info) = &common_info {
        fill_identity(&mut mod_id, &info.id);
        fill_identity(&mut title, &info.name);
        fill_identity(&mut author, &info.author);
    }
    if let Some(info) = &root_info {
        fill_identity(&mut mod_id, &info.id);
        fill_identity(&mut title, &info.name);
        fill_identity(&mut author, &info.author);
    }

    let info_refs: Vec<&ModInfo> = folder_infos
        .iter()
        .filter_map(|(_, manifest)| manifest.as_ref())
        .chain(common_info.iter())
        .chain(root_info.iter())
        .collect();
    let requires = union(info_refs.iter().map(|i| &i.requires));
    let incompatible = union(info_refs.iter().map(|i| &i.incompatible));
    let load_before = union(info_refs.iter().map(|i| &i.load_before));
    let load_after = union(info_refs.iter().map(|i| &i.load_after));
    let folders: Vec<FolderRange> = folder_infos.into_iter().map(|(range, _)| range).collect();
    let common = common_info.map(|info| ManifestRange {
        min: info.version_min.as_ref().map(|v| v.text().to_owned()),
        max: info.version_max.as_ref().map(|v| v.text().to_owned()),
    });
    let root = root_info.map(|info| ManifestRange {
        min: info.version_min.as_ref().map(|v| v.text().to_owned()),
        max: info.version_max.as_ref().map(|v| v.text().to_owned()),
    });
    let layout = match (root_path.is_some(), common_path.is_some() || !folders.is_empty()) {
        (true, true) => Layout::Hybrid,
        (true, false) => Layout::B41,
        (false, _) => Layout::B42,
    };

    Some(ModEntry {
        name: name.to_owned(),
        mod_id,
        title,
        author,
        folders,
        common_dir,
        common,
        root,
        requires,
        incompatible,
        load_before,
        load_after,
        layout,
    })
}

fn fill_identity(slot: &mut Option<String>, value: &Option<String>) {
    if slot.is_none() {
        *slot = value.clone();
    }
}

/// The lowest version folder the B42 loader could select: the load
/// lower bound when no `common/` manifest covers the versions below.
pub fn lowest_folder(mod_entry: &ModEntry) -> Option<&FolderRange> {
    mod_entry
        .folders
        .iter()
        .filter(|f| f.version >= 42 * 1000)
        .min_by_key(|f| f.version)
}

/// True when any manifest declares a versionMin or versionMax.
pub fn has_declared_range(mod_entry: &ModEntry) -> bool {
    mod_entry
        .folders
        .iter()
        .any(|f| {
            f.manifest
                .as_ref()
                .is_some_and(|r| r.min.is_some() || r.max.is_some())
        })
        || mod_entry
            .common
            .as_ref()
            .is_some_and(|r| r.min.is_some() || r.max.is_some())
        || mod_entry
            .root
            .as_ref()
            .is_some_and(|r| r.min.is_some() || r.max.is_some())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, relative: &str, text: &str) {
        fs::create_dir_all(dir.join(relative).parent().unwrap()).unwrap();
        fs::write(dir.join(relative), text).unwrap();
    }

    #[test]
    fn recognizes_b41_b42_and_hybrid() {
        let base = std::env::temp_dir().join(format!("yapzm-item-{}", std::process::id()));

        // B41: root manifest only.
        let item = base.join("b41");
        write(&item, "mods/Old Mod/mod.info", "name=Old Mod\nid=OM\n");
        // B42: version folders plus common.
        let item = base.join("b42");
        write(&item, "mods/New Mod/42.16/mod.info", "name=New Mod\nid=NM\nversionMin=42.16\n");
        write(&item, "mods/New Mod/common/mod.info", "name=New Mod\nid=NM\n");
        // Hybrid: root manifest plus a version folder.
        let item = base.join("hybrid");
        write(&item, "mods/Both/mod.info", "name=Both\nid=B\n");
        write(&item, "mods/Both/42.5/mod.info", "name=Both\nid=B\nversionMin=42.5\n");
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
    fn identity_comes_from_the_highest_version_folder() {
        let base = std::env::temp_dir().join(format!("yapzm-item-id-{}", std::process::id()));
        let item = base.join("item");
        write(&item, "mods/M/42.16/mod.info", "name=M 16\nid=M16\nauthor=A\n");
        write(&item, "mods/M/42.22/mod.info", "name=M 22\nid=M22\n");
        write(&item, "mods/M/common/mod.info", "name=M common\nid=MC\n");

        let mods = inspect_item(&item);
        assert_eq!(mods.len(), 1);
        // The game reads the highest folder ≤ the running version; on
        // current builds that is the highest folder, so its manifest
        // is the identity source.
        assert_eq!(mods[0].mod_id.as_deref(), Some("M22"));
        assert_eq!(mods[0].title.as_deref(), Some("M 22"));
        // Fields the highest manifest omits fall through to lower ones.
        assert_eq!(mods[0].author.as_deref(), Some("A"));

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn captures_per_folder_ranges() {
        let base = std::env::temp_dir().join(format!("yapzm-item-agg-{}", std::process::id()));
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
        let folders = &mods[0].folders;
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].version, 42_016);
        assert_eq!(folders[0].name, "42.16");
        assert_eq!(
            folders[0].manifest.as_ref().unwrap().min.as_deref(),
            Some("42.16")
        );
        assert_eq!(folders[0].manifest.as_ref().unwrap().max, None);
        assert_eq!(folders[1].version, 42_022);
        assert_eq!(folders[1].manifest.as_ref().unwrap().min.as_deref(), Some("42.10"));
        assert_eq!(folders[1].manifest.as_ref().unwrap().max.as_deref(), Some("42.99"));
        assert_eq!(mods[0].requires, vec!["Alpha", "Beta", "Gamma"]);
        assert_eq!(mods[0].load_after, vec!["Late"]);

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn captures_common_and_root_manifests() {
        let base = std::env::temp_dir().join(format!("yapzm-item-cm-{}", std::process::id()));
        let item = base.join("item");
        write(&item, "mods/M/mod.info", "name=M\nid=M\nversionMax=41.99\n");
        write(&item, "mods/M/common/mod.info", "name=M\nid=M\nversionMin=42.0\n");
        write(&item, "mods/M/42.20/mod.info", "name=M\nid=M\n");

        let mods = inspect_item(&item);
        assert_eq!(mods[0].root.as_ref().unwrap().max.as_deref(), Some("41.99"));
        assert_eq!(mods[0].common.as_ref().unwrap().min.as_deref(), Some("42.0"));
        assert_eq!(mods[0].folders.len(), 1);
        assert_eq!(mods[0].folders[0].name, "42.20");

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn captures_media_only_version_folders() {
        let base = std::env::temp_dir().join(format!("yapzm-item-mo-{}", std::process::id()));
        let item = base.join("item");
        // The Project Viewpoint shape: a version folder with content
        // but no mod.info, recognized through common/.
        write(&item, "mods/M/42/media/lua/a.lua", "lua");
        write(&item, "mods/M/common/mod.info", "name=M\nid=M\n");

        let mods = inspect_item(&item);
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].folders.len(), 1);
        assert_eq!(mods[0].folders[0].version, 42_000);
        assert_eq!(mods[0].folders[0].name, "42");
        assert_eq!(mods[0].folders[0].manifest, None);
        assert_eq!(mods[0].common.as_ref().unwrap().min, None);

        // A numeric-named folder alone is not a mod: no manifest
        // anywhere means the game cannot recognize it.
        let item = base.join("bare");
        write(&item, "mods/N/42/media/a.lua", "lua");
        assert!(inspect_item(&item).is_empty());

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn version_folder_names_are_not_version_data() {
        let base = std::env::temp_dir().join(format!("yapzm-item-vm-{}", std::process::id()));
        let item = base.join("item");
        // Folder names lie: 42.0.5 truncates to 42.0 in the game's key.
        write(&item, "mods/M/42.0.5/mod.info", "name=M\nid=M\n");
        write(&item, "mods/M/43/mod.info", "name=M\nid=M\n");

        let mods = inspect_item(&item);
        assert_eq!(mods[0].folders[0].version, 42_000);
        assert_eq!(
            mods[0].folders[0].manifest.as_ref().unwrap().min,
            None
        );
        assert_eq!(mods[0].folders[0].manifest.as_ref().unwrap().max, None);
        assert_eq!(mods[0].layout, Layout::B42);

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn lowest_folder_skips_sub_floor_folders() {
        let base = std::env::temp_dir().join(format!("yapzm-item-lf-{}", std::process::id()));
        let item = base.join("item");
        write(&item, "mods/M/41/mod.info", "name=M\nid=M\n");
        write(&item, "mods/M/42.16/mod.info", "name=M\nid=M\n");

        let mods = inspect_item(&item);
        // The 41 folder is below the game's 42.0 floor and never
        // selected by the B42 loader.
        assert_eq!(lowest_folder(&mods[0]).map(|f| f.name.as_str()), Some("42.16"));
        assert!(!has_declared_range(&mods[0]));

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn missing_mods_dir_is_empty() {
        assert!(inspect_item(Path::new("nonexistent")).is_empty());
    }
}
