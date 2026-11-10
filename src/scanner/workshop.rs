//! Component 4: Workshop scanner.
//!
//! Combines appworkshop_108600.acf metadata with a disk walk of
//! workshop/content/108600/<pfid>/mods/ to produce an inventory of what is
//! installed right now, including each mod's supported game version range
//! parsed from its mod.info files.

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::parser::acf::{self, Acf};
use crate::warn;
use super::steam::{self, SteamPaths};

#[derive(Debug)]
pub struct WorkshopItem {
    pub id: u64,
    /// Last update timestamp from the ACF (Steam's own version key).
    /// Items present on disk but absent from the ACF report 0.
    pub time_updated: u64,
    /// Mods bundled in this workshop item.
    pub mods: Vec<ModEntry>,
}

#[derive(Debug)]
pub struct ModEntry {
    /// Mod folder name under <pfid>/mods/.
    pub name: String,
    /// versionMin from the mod's mod.info manifests (lowest declared).
    /// None when no manifest declares it.
    pub version_min: Option<String>,
    /// versionMax from the mod's mod.info manifests (highest declared).
    pub version_max: Option<String>,
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
            let root = acf::parse(&text)
                .with_context(|| format!("parsing {}", acf_path.display()))?;
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
                        mods: list_mods(&paths.workshop_content.join(id.to_string())),
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
            mods: list_mods(&entry.path()),
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
        .join(format!("appworkshop_{}.acf", steam::PZ_APP_ID))
}

/// Mods bundled in a workshop item. A folder under <pfid>/mods/ counts as
/// a mod when it contains a mod.info (Build 41 style) or when any of its
/// immediate subdirectories does (Build 42 style version folders).
///
/// The mod.info manifests are the single source of truth for the supported
/// version range: versionMin/versionMax fields are aggregated across every
/// manifest found (top-level, version folders, common/). Version folder
/// names are deliberately ignored — deriving versions from on-disk layout
/// is a compatibility concern for a separate component.
fn list_mods(item_dir: &Path) -> Vec<ModEntry> {
    let mut mods = Vec::new();
    let Ok(entries) = fs::read_dir(item_dir.join("mods")) else {
        return mods;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();

        let mut manifests: Vec<PathBuf> = Vec::new();
        let direct = dir.join("mod.info");
        if direct.is_file() {
            manifests.push(direct);
        }
        if let Ok(versions) = fs::read_dir(&dir) {
            for version in versions.flatten() {
                let info = version.path().join("mod.info");
                if info.is_file() {
                    manifests.push(info);
                }
            }
        }
        if manifests.is_empty() {
            continue;
        }

        let mut declared_min: Vec<(Vec<u64>, String)> = Vec::new();
        let mut declared_max: Vec<(Vec<u64>, String)> = Vec::new();
        for manifest in &manifests {
            let (version_min, version_max) = parse_mod_info(manifest);
            if let Some(min) = version_min {
                declared_min.push((version_key(&min), min));
            }
            if let Some(max) = version_max {
                declared_max.push((version_key(&max), max));
            }
        }

        let version_min = declared_min
            .iter()
            .min_by(|a, b| cmp_candidates(a, b))
            .map(|(_, value)| value.clone());
        let version_max = declared_max
            .iter()
            .max_by(|a, b| cmp_candidates(a, b))
            .map(|(_, value)| value.clone());

        mods.push(ModEntry {
            name,
            version_min,
            version_max,
        });
    }
    mods.sort_by(|a, b| a.name.cmp(&b.name));
    mods
}

/// Order by numeric version key, then by string so equal keys (e.g.
/// "42.16" vs "42.16.0" declared in different manifests) resolve
/// deterministically regardless of directory read order.
fn cmp_candidates(a: &(Vec<u64>, String), b: &(Vec<u64>, String)) -> Ordering {
    cmp_versions(&a.0, &b.0).then_with(|| a.1.cmp(&b.1))
}

/// Parse the versionMin/versionMax fields from a mod.info file. The file
/// is key=value lines; unknown keys are ignored.
fn parse_mod_info(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(text) = fs::read_to_string(path) else {
        return (None, None);
    };
    let mut version_min = None;
    let mut version_max = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "versionMin" => version_min = Some(value.trim().to_string()),
            "versionMax" => version_max = Some(value.trim().to_string()),
            _ => {}
        }
    }
    (version_min, version_max)
}

/// Numeric component-wise comparison key for dotted versions like
/// "42.22.0". Non-numeric components count as 0.
fn version_key(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|part| part.trim().parse().unwrap_or(0))
        .collect()
}

/// Compare two version keys component-wise; missing components count as 0,
/// so [42, 22] equals [42, 22, 0].
fn cmp_versions(a: &[u64], b: &[u64]) -> Ordering {
    for index in 0..a.len().max(b.len()) {
        let left = a.get(index).copied().unwrap_or(0);
        let right = b.get(index).copied().unwrap_or(0);
        if left != right {
            return left.cmp(&right);
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::steam::SteamPaths;

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
        fs::write(content.join("123456789/mods/Hybrid Mod/mod.info"), "name=Hybrid\n").unwrap();
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
        assert_eq!(items[0].mods[0].version_min, None);
        assert_eq!(items[0].mods[0].version_max, None);

        assert_eq!(items[1].id, 123456789);
        assert_eq!(items[1].time_updated, 1_700_000_000);
        assert_eq!(items[1].mods.len(), 3);

        assert_eq!(items[1].mods[0].name, "B42 Mod");
        assert_eq!(items[1].mods[0].version_min.as_deref(), Some("42.16.0"));
        assert_eq!(items[1].mods[0].version_max.as_deref(), Some("42.99.0"));

        assert_eq!(items[1].mods[1].name, "Hybrid Mod");
        assert_eq!(items[1].mods[1].version_min.as_deref(), Some("42.5.0"));
        assert_eq!(items[1].mods[1].version_max, None);

        assert_eq!(items[1].mods[2].name, "Test Mod");
        assert_eq!(items[1].mods[2].version_min, None);
        assert_eq!(items[1].mods[2].version_max, None);
    }

    #[test]
    fn compares_versions_component_wise() {
        assert_eq!(cmp_versions(&[42, 22], &[42, 22, 0]), Ordering::Equal);
        assert_eq!(cmp_versions(&[42, 16], &[42, 22]), Ordering::Less);
        assert_eq!(cmp_versions(&[43, 0], &[42, 99]), Ordering::Greater);
    }
}
