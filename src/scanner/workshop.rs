//! Workshop scanner: combines appworkshop_<appid>.acf metadata with a
//! disk walk of workshop/content/<appid>/<pfid>/mods/ to produce an
//! inventory of what is installed right now, including each mod's
//! supported game version range parsed from its mod.info files.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context as _, Result};

use crate::parser::acf::{self, Acf};
use crate::pz::{self, item::inspect_item};
use crate::steam::SteamPaths;
use crate::warn;

pub use crate::pz::item::ModEntry;

#[derive(Debug)]
pub struct WorkshopItem {
    pub id: u64,
    /// Last update timestamp from the ACF (Steam's own version key).
    /// Items present on disk but absent from the ACF report 0.
    pub time_updated: u64,
    /// Mods bundled in this workshop item.
    pub mods: Vec<ModEntry>,
}

/// Inventory the installed workshop items for PZ.
pub fn scan(paths: &SteamPaths) -> Result<Vec<WorkshopItem>> {
    if !paths.workshop_content.is_dir() {
        return Ok(Vec::new());
    }

    let acf_path = workshop_acf_path(paths);
    let mut items = Vec::new();

    match fs::read_to_string(&acf_path) {
        Ok(text) => {
            let root =
                acf::parse(&text).with_context(|| format!("parsing {}", acf_path.display()))?;
            if let Some(Acf::Map(entries)) = root.get("WorkshopItemsInstalled") {
                for (id, entry) in entries {
                    let Ok(id) = id.parse::<u64>() else {
                        continue;
                    };
                    let time_updated = entry
                        .get("timeupdated")
                        .and_then(Acf::as_str)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    items.push(WorkshopItem {
                        id,
                        time_updated,
                        mods: inspect_item(&paths.workshop_content.join(id.to_string())),
                    });
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            warn!(
                "{} not found; listing disk contents only",
                acf_path.display()
            );
        }
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", acf_path.display()));
        }
    }

    // Pick up items on disk that the ACF does not know about (e.g. placed
    // manually or left behind by a Steam cleanup).
    for entry in fs::read_dir(&paths.workshop_content)
        .with_context(|| format!("reading {}", paths.workshop_content.display()))?
        .flatten()
    {
        let Ok(id) = entry.file_name().to_string_lossy().parse::<u64>() else {
            continue;
        };
        if items.iter().any(|item| item.id == id) {
            continue;
        }
        items.push(WorkshopItem {
            id,
            time_updated: 0,
            mods: inspect_item(&entry.path()),
        });
    }

    items.sort_by_key(|item| item.id);
    Ok(items)
}

/// appworkshop_<appid>.acf lives in the workshop/ dir of the library that
/// holds the workshop content:
/// .../steamapps/workshop/content/108600 -> .../steamapps/workshop/appworkshop_108600.acf
fn workshop_acf_path(paths: &SteamPaths) -> PathBuf {
    paths
        .workshop_content
        .ancestors()
        .nth(2)
        .unwrap_or(paths.workshop_content.as_path())
        .join(format!("appworkshop_{}.acf", pz::APP_ID))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fake Steam layout on disk and check that scan() merges ACF
    /// metadata with the disk listing and parses mod.info version fields.
    #[test]
    fn scans_fixture_layout() {
        let base = std::env::temp_dir().join(format!("snapoid-scan-test-{}", std::process::id()));
        let content = base.join("steamapps/workshop/content/108600");

        let acf = base.join("steamapps/workshop/appworkshop_108600.acf");
        fs::create_dir_all(acf.parent().unwrap()).unwrap();
        fs::write(
            &acf,
            "\"AppWorkshop_108600\"\n{\n\t\"WorkshopItemsInstalled\"\n\t{\n\t\t\"123456789\"\n\t\t{\n\t\t\t\"timeupdated\"\t\t\"1700000000\"\n\t\t}\n\t}\n}\n",
        )
        .unwrap();

        // Build 41 mod: mod.info directly in the mod folder, no version
        // fields.
        fs::create_dir_all(content.join("123456789/mods/Test Mod")).unwrap();
        fs::write(
            content.join("123456789/mods/Test Mod/mod.info"),
            "name=Test Mod\nid=TM\n",
        )
        .unwrap();
        fs::create_dir_all(content.join("123456789/mods/Not A Mod")).unwrap();

        // Build 42 layout: mod.info in per-version subfolders with
        // versionMin/versionMax to aggregate.
        fs::create_dir_all(content.join("123456789/mods/B42 Mod/42.16")).unwrap();
        fs::write(
            content.join("123456789/mods/B42 Mod/42.16/mod.info"),
            "name=B42 Mod\nversionMin=42.16.0\n",
        )
        .unwrap();
        fs::create_dir_all(content.join("123456789/mods/B42 Mod/42.22")).unwrap();
        fs::write(
            content.join("123456789/mods/B42 Mod/42.22/mod.info"),
            "name=B42 Mod\nversionMin=42.22.0\nversionMax=42.99.0\n",
        )
        .unwrap();
        fs::create_dir_all(content.join("123456789/mods/B42 Mod/common")).unwrap();

        // Mod with a top-level manifest plus version folders: fields
        // aggregate across all manifests, folder names are ignored.
        fs::create_dir_all(content.join("123456789/mods/Hybrid Mod")).unwrap();
        fs::write(
            content.join("123456789/mods/Hybrid Mod/mod.info"),
            "name=Hybrid\n",
        )
        .unwrap();
        fs::create_dir_all(content.join("123456789/mods/Hybrid Mod/42.5")).unwrap();
        fs::write(
            content.join("123456789/mods/Hybrid Mod/42.5/mod.info"),
            "name=Hybrid\nversionMin=42.5.0\n",
        )
        .unwrap();

        // Disk-only item the ACF does not know about.
        fs::create_dir_all(content.join("999/mods/Disk Only")).unwrap();
        fs::write(content.join("999/mods/Disk Only/mod.info"), "").unwrap();

        let paths = SteamPaths {
            steam_dir: base.clone(),
            workshop_content: content,
            game_dir: None,
            zomboid_user_dir: None,
        };

        let items = scan(&paths).unwrap();
        fs::remove_dir_all(&base).unwrap();

        assert_eq!(items.len(), 2);

        assert_eq!(items[0].id, 999);
        assert_eq!(items[0].time_updated, 0);
        assert_eq!(items[0].mods.len(), 1);
        assert_eq!(items[0].mods[0].name, "Disk Only");
        // B41 shape: root manifest only, no version folders.
        assert!(items[0].mods[0].folders.is_empty());
        assert!(items[0].mods[0].root.is_some());

        assert_eq!(items[1].id, 123456789);
        assert_eq!(items[1].time_updated, 1_700_000_000);
        assert_eq!(items[1].mods.len(), 3);

        assert_eq!(items[1].mods[0].name, "B42 Mod");
        let folders = &items[1].mods[0].folders;
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].name, "42.16");
        assert_eq!(folders[0].min.as_deref(), Some("42.16.0"));
        assert_eq!(folders[1].name, "42.22");
        assert_eq!(folders[1].min.as_deref(), Some("42.22.0"));
        assert_eq!(folders[1].max.as_deref(), Some("42.99.0"));

        assert_eq!(items[1].mods[1].name, "Hybrid Mod");
        assert_eq!(items[1].mods[1].folders.len(), 1);
        assert_eq!(items[1].mods[1].folders[0].min.as_deref(), Some("42.5.0"));
        assert!(items[1].mods[1].root.is_some());

        assert_eq!(items[1].mods[2].name, "Test Mod");
        assert!(items[1].mods[2].folders.is_empty());
        assert!(items[1].mods[2].root.is_some());
    }
}
