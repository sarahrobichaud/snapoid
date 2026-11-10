use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

use crate::paths;

/// Parsed key=value fields from a `mod.info` file.
#[derive(Debug, Default)]
pub struct ModInfo {
    pub name: Option<String>,
    pub id: Option<String>,
    pub description: Option<String>,
}

/// One build of a mod: either the top-level (unversioned) folder or a
/// game-version folder like `42` / `42.13`.
#[derive(Debug)]
pub struct Build {
    pub version: Option<String>,
    pub info: ModInfo,
}

/// A single mod folder inside a workshop item.
#[derive(Debug)]
pub struct Mod {
    pub folder: String,
    pub builds: Vec<Build>,
}

/// A workshop item (one folder under `workshop/content/108600`), which may
/// bundle several mods.
#[derive(Debug)]
pub struct WorkshopItem {
    pub id: String,
    pub mods: Vec<Mod>,
}

fn parse_mod_info(path: &Path) -> ModInfo {
    let mut info = ModInfo::default();
    let Ok(content) = fs::read_to_string(path) else {
        return info;
    };
    for line in content.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "name" => {
                info.name.get_or_insert(value.to_string());
            }
            "id" => {
                info.id.get_or_insert(value.to_string());
            }
            "description" => {
                info.description.get_or_insert(value.to_string());
            }
            _ => {}
        }
    }
    info
}

/// Numeric-aware comparison so `42.13` sorts before `42.22`.
fn version_key(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

fn scan_mod_folder(mod_dir: &Path) -> Mod {
    let folder = mod_dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut builds = Vec::new();

    let top_info = mod_dir.join("mod.info");
    if top_info.is_file() {
        builds.push(Build {
            version: None,
            info: parse_mod_info(&top_info),
        });
    }

    if let Ok(entries) = fs::read_dir(mod_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let info_path = path.join("mod.info");
            if info_path.is_file() {
                builds.push(Build {
                    version: Some(entry.file_name().to_string_lossy().into_owned()),
                    info: parse_mod_info(&info_path),
                });
            }
        }
    }

    builds.sort_by(|a, b| match (&a.version, &b.version) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(x), Some(y)) => version_key(x).cmp(&version_key(y)),
    });

    Mod { folder, builds }
}

pub fn scan(workshop: &Path) -> Vec<WorkshopItem> {
    let mut items = Vec::new();
    let Ok(entries) = fs::read_dir(workshop) else {
        return items;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        let mut mods = Vec::new();
        if let Ok(mod_dirs) = fs::read_dir(path.join("mods")) {
            for mod_dir in mod_dirs.flatten() {
                let mod_path = mod_dir.path();
                if mod_path.is_dir() {
                    mods.push(scan_mod_folder(&mod_path));
                }
            }
        }
        mods.sort_by(|a, b| a.folder.cmp(&b.folder));
        items.push(WorkshopItem { id, mods });
    }
    items.sort_by(|a, b| {
        let ka = a.id.parse::<u64>().unwrap_or(0);
        let kb = b.id.parse::<u64>().unwrap_or(0);
        ka.cmp(&kb).then_with(|| a.id.cmp(&b.id))
    });
    items
}

impl Mod {
    fn display_name(&self) -> &str {
        self.builds
            .iter()
            .find(|b| b.version.is_none())
            .or_else(|| self.builds.first())
            .and_then(|b| b.info.name.as_deref())
            .unwrap_or(&self.folder)
    }

    fn display_id(&self) -> &str {
        self.builds
            .iter()
            .find(|b| b.version.is_none())
            .or_else(|| self.builds.first())
            .and_then(|b| b.info.id.as_deref())
            .unwrap_or(&self.folder)
    }

    fn versions_label(&self) -> String {
        let versions: Vec<&str> = self
            .builds
            .iter()
            .filter_map(|b| b.version.as_deref())
            .collect();
        if versions.is_empty() {
            if self.builds.iter().any(|b| b.version.is_none()) {
                "unversioned".to_string()
            } else {
                "-".to_string()
            }
        } else {
            versions.join(", ")
        }
    }
}

pub fn run(workshop_override: Option<PathBuf>) {
    let resolved = paths::resolve(None, workshop_override, None);
    let Some(workshop) = resolved.workshop else {
        eprintln!("Workshop mods directory not found.");
        eprintln!("Use --workshop to point snapoid at steamapps/workshop/content/108600.");
        std::process::exit(1);
    };

    let items = scan(&workshop);
    if items.is_empty() {
        println!("No workshop mods found in {}", workshop.display());
        return;
    }

    let mut rows: Vec<[String; 4]> = Vec::new();
    for item in &items {
        for m in &item.mods {
            rows.push([
                item.id.clone(),
                m.display_name().to_string(),
                m.display_id().to_string(),
                m.versions_label(),
            ]);
        }
    }

    let headers = ["Workshop ID", "Mod name", "Mod ID", "Versions"];
    let widths: Vec<usize> = (0..4)
        .map(|i| {
            headers[i]
                .len()
                .max(rows.iter().map(|r| r[i].len()).max().unwrap_or(0))
        })
        .collect();

    for row in std::iter::once(headers.map(|h| h.to_string())).chain(rows.iter().cloned()) {
        println!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {}",
            row[0], row[1], row[2], row[3],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
        );
    }

    let total_mods: usize = items.iter().map(|i| i.mods.len()).sum();
    println!();
    println!("{} workshop items, {} mods", items.len(), total_mods);
}
