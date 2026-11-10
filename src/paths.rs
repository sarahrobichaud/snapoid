use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Steam App ID for Project Zomboid
pub const PZ_APP_ID: &str = "108600";

#[derive(Debug)]
pub struct DetectedPaths {
    pub steam_roots: Vec<PathBuf>,
    pub game: Option<PathBuf>,
    pub workshop: Option<PathBuf>,
    pub userdata: Option<PathBuf>,
}

pub fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(windows)]
fn steam_root_from_registry() -> Option<PathBuf> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let steam = hkcu.open_subkey("Software\\Valve\\Steam").ok()?;
    let path: String = steam.get_value("SteamPath").ok()?;
    Some(PathBuf::from(path))
}

#[cfg(not(windows))]
fn steam_root_from_registry() -> Option<PathBuf> {
    None
}

fn steam_root_candidates() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(root) = steam_root_from_registry() {
        roots.push(root);
    }

    if let Some(home) = home_dir() {
        match env::consts::OS {
            "windows" => {
                roots.push(PathBuf::from("C:\\Program Files (x86)\\Steam"));
                roots.push(PathBuf::from("C:\\Program Files\\Steam"));
            }
            "macos" => {
                roots.push(home.join("Library/Application Support/Steam"));
            }
            _ => {
                roots.push(home.join(".steam/steam"));
                roots.push(home.join(".steam/debian-installation"));
                roots.push(home.join(".local/share/Steam"));
                // WSL: Windows Steam installs are mounted under /mnt/<drive>
                if let Ok(entries) = fs::read_dir("/mnt") {
                    for entry in entries.flatten() {
                        let mnt = entry.path();
                        roots.push(mnt.join("Program Files (x86)/Steam"));
                        roots.push(mnt.join("Program Files/Steam"));
                    }
                }
            }
        }
    }

    let mut found: Vec<PathBuf> = Vec::new();
    for root in roots {
        if root.is_dir() && !found.contains(&root) {
            found.push(root);
        }
    }
    found
}

/// Extract the values of a string key (e.g. "path") from a Valve KeyValues file.
fn parse_vdf_string_values(content: &str, key: &str) -> Vec<PathBuf> {
    let needle = format!("\"{key}\"");
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(&needle) {
            let rest = rest.trim_start();
            if let Some(value) = rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
                // VDF escapes backslashes on Windows: C:\\Program Files\\Steam
                out.push(PathBuf::from(value.replace("\\\\", "\\")));
            }
        }
    }
    out
}

/// All Steam libraries known to a Steam root, from libraryfolders.vdf.
fn steam_libraries(steam_root: &Path) -> Vec<PathBuf> {
    let mut libs = vec![steam_root.to_path_buf()];
    let vdf = steam_root.join("steamapps").join("libraryfolders.vdf");
    if let Ok(content) = fs::read_to_string(&vdf) {
        for path in parse_vdf_string_values(&content, "path") {
            if !libs.contains(&path) {
                libs.push(path);
            }
        }
    }
    libs
}

pub fn detect() -> DetectedPaths {
    let mut steam_roots = Vec::new();
    let mut game = None;
    let mut workshop = None;

    for root in steam_root_candidates() {
        steam_roots.push(root.clone());
        for lib in steam_libraries(&root) {
            if game.is_none() {
                let candidate = lib.join("steamapps").join("common").join("ProjectZomboid");
                if candidate.is_dir() {
                    game = Some(candidate);
                }
            }
            if workshop.is_none() {
                let candidate = lib
                    .join("steamapps")
                    .join("workshop")
                    .join("content")
                    .join(PZ_APP_ID);
                if candidate.is_dir() {
                    workshop = Some(candidate);
                }
            }
        }
    }

    let userdata = detect_userdata();

    DetectedPaths {
        steam_roots,
        game,
        workshop,
        userdata,
    }
}

fn detect_userdata() -> Option<PathBuf> {
    if let Some(home) = home_dir() {
        let candidate = home.join("Zomboid");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }

    // WSL: Project Zomboid on Windows stores user data in C:\Users\<name>\Zomboid
    if let Ok(drives) = fs::read_dir("/mnt") {
        for drive in drives.flatten() {
            let users = drive.path().join("Users");
            if let Ok(profiles) = fs::read_dir(&users) {
                for profile in profiles.flatten() {
                    let candidate = profile.path().join("Zomboid");
                    if candidate.is_dir() {
                        return Some(candidate);
                    }
                }
            }
        }
    }

    None
}

pub fn run(
    game_override: Option<PathBuf>,
    workshop_override: Option<PathBuf>,
    userdata_override: Option<PathBuf>,
) {
    let detected = detect();

    let game = game_override.or(detected.game);
    let workshop = workshop_override.or(detected.workshop);
    let userdata = userdata_override.or(detected.userdata);

    println!("Project Zomboid paths:");
    println!();
    print_path("Game install", game.as_deref());
    print_path("Workshop mods", workshop.as_deref());
    print_path("User data", userdata.as_deref());

    if (game.is_none() || workshop.is_none()) && !detected.steam_roots.is_empty() {
        println!();
        println!("Steam libraries searched:");
        for root in &detected.steam_roots {
            println!("  {}", root.display());
        }
        println!();
        println!("Use --game / --workshop to point snapoid at the right directories.");
    }
}

fn print_path(label: &str, path: Option<&Path>) {
    match path {
        Some(p) => {
            let status = if p.is_dir() { "found" } else { "missing" };
            println!("{:<14} [{}] {}", label, status, p.display();
        }
        None => {
            println!("{:<14} [not found]", label);
        }
    }
}
