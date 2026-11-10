//! Component 2: Steam locator.
//!
//! Finds the Steam root, the library holding PZ workshop content, the PZ
//! game install, and the Zomboid user data dir.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::parser::acf::{self, Acf};
use crate::warn;

/// Project Zomboid's Steam app ID.
pub const PZ_APP_ID: u32 = 108600;

const PZ_COMMON_DIR: &str = "ProjectZomboid";

#[derive(Debug)]
pub struct SteamPaths {
    /// Steam root (contains steamapps/).
    pub steam_dir: PathBuf,
    /// Library dir containing workshop content for PZ
    /// (.../steamapps/workshop/content/108600). May live in any library;
    /// falls back to the Steam root's path if no library has it yet.
    pub workshop_content: PathBuf,
    /// PZ game install directory, if found.
    pub game_dir: Option<PathBuf>,
    /// Zomboid user data dir (e.g. %UserProfile%\Zomboid), if found.
    pub zomboid_user_dir: Option<PathBuf>,
}

/// Locate Steam and derive all PZ-relevant paths.
pub fn locate() -> Result<SteamPaths> {
    let steam_dir = find_steam_dir()?;
    let libraries = find_libraries(&steam_dir)?;
    let workshop_content = find_workshop_content(&steam_dir, &libraries);
    let game_dir = find_game_dir(&libraries);
    let zomboid_user_dir = find_zomboid_user_dir();

    Ok(SteamPaths {
        steam_dir,
        workshop_content,
        game_dir,
        zomboid_user_dir,
    })
}

/// Find the Steam root. Checks, in order: the Windows registry (via
/// reg.exe, which also works under WSL interop), %ProgramFiles(x86)%\Steam,
/// the usual Linux/macOS locations, and /mnt/c for WSL installs. The first
/// candidate containing a steamapps/ directory wins. Under WSL the
/// registry's drive-letter path is not directly readable, so the /mnt/c
/// fallback takes over there.
fn find_steam_dir() -> Result<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(path) = steam_dir_from_registry() {
        candidates.push(path);
    }

    if let Ok(program_files) = std::env::var("ProgramFiles(x86)") {
        candidates.push(PathBuf::from(program_files).join("Steam"));
    }
    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join(".steam/steam"));
        candidates.push(home.join(".local/share/Steam"));
        candidates.push(home.join("Library/Application Support/Steam"));
        // WSL: Steam installed on the Windows host.
        candidates.push(PathBuf::from("/mnt/c/Program Files (x86)/Steam"));
    }

    candidates
        .into_iter()
        .find(|p| p.join("steamapps").is_dir())
        .with_context(|| "no Steam directory with a steamapps/ folder found")
}

/// Read SteamPath from the Windows registry by shelling out to reg.exe
/// instead of using a Windows-only crate, so the whole crate compiles (and
/// is analyzed by rust-analyzer) identically on every platform. On
/// non-Windows hosts reg does not exist and this returns None.
fn steam_dir_from_registry() -> Option<PathBuf> {
    let output = Command::new("reg")
        .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_reg_steam_path(&String::from_utf8_lossy(&output.stdout))
}

/// Parse `reg query` output. Values are tab-separated but paths may contain
/// spaces, so anchor on the REG_SZ type marker rather than splitting on
/// whitespace.
fn parse_reg_steam_path(stdout: &str) -> Option<PathBuf> {
    for line in stdout.lines() {
        if !line.contains("SteamPath") {
            continue;
        }
        let Some(marker) = line.find("REG_SZ") else {
            continue;
        };
        let value = line[marker + "REG_SZ".len()..].trim();
        if !value.is_empty() {
            return Some(PathBuf::from(value));
        }
    }
    None
}

/// Enumerate Steam library roots, Steam root first, then any extra
/// libraries from steamapps/libraryfolders.vdf in their declared order.
fn find_libraries(steam_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut libraries = vec![steam_dir.to_path_buf()];

    let vdf = steam_dir.join("steamapps/libraryfolders.vdf");
    if let Ok(text) = std::fs::read_to_string(&vdf) {
        match acf::parse(&text) {
            Ok(root) => {
                let mut extra: Vec<(u32, PathBuf)> = Vec::new();
                if let Acf::Map(folders) = &root {
                    for (key, folder) in folders {
                        let Some(index) = key.parse::<u32>().ok() else {
                            continue;
                        };
                        let Some(path) = folder.get("path").and_then(Acf::as_str) else {
                            continue;
                        };
                        extra.push((index, PathBuf::from(path)));
                    }
                }
                extra.sort_by_key(|(index, _)| *index);
                for (_, path) in extra {
                    if !libraries.contains(&path) {
                        libraries.push(path);
                    }
                }
            }
            Err(error) => {
                warn!("could not parse {}: {error}", vdf.display());
            }
        }
    }

    Ok(libraries)
}

/// Workshop content for an app can live in any library; pick the first
/// library that actually has content for PZ, else default to the Steam
/// root's path (nothing downloaded yet).
fn find_workshop_content(steam_dir: &Path, libraries: &[PathBuf]) -> PathBuf {
    let default = steam_dir.join(format!("steamapps/workshop/content/{PZ_APP_ID}"));
    libraries
        .iter()
        .map(|lib| lib.join(format!("steamapps/workshop/content/{PZ_APP_ID}")))
        .find(|p| p.is_dir())
        .unwrap_or(default)
}

fn find_game_dir(libraries: &[PathBuf]) -> Option<PathBuf> {
    libraries
        .iter()
        .map(|lib| lib.join("steamapps/common").join(PZ_COMMON_DIR))
        .find(|p| p.is_dir())
}

/// Buildid of the installed app, read from the appmanifest in the
/// library holding `game_dir` (.../steamapps/common/<name>). None when
/// the manifest is missing or unparsable.
pub fn installed_buildid(game_dir: &Path, app_id: u32) -> Option<u64> {
    let text = std::fs::read_to_string(
        game_dir
            .parent()?
            .parent()?
            .join(format!("appmanifest_{app_id}.acf")),
    )
    .ok()?;
    let Acf::Map(map) = acf::parse(&text).ok()? else {
        return None;
    };
    map.get("buildid")?.as_str()?.parse().ok()
}

/// The Zomboid user data dir sits next to the user's profile
/// (%UserProfile%\Zomboid on Windows). Under WSL, probe the Windows users
/// mounted at /mnt/c/Users.
fn find_zomboid_user_dir() -> Option<PathBuf> {
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let dir = PathBuf::from(profile).join("Zomboid");
        if dir.is_dir() {
            return Some(dir);
        }
    }
    if let Ok(entries) = std::fs::read_dir("/mnt/c/Users") {
        for entry in entries.flatten() {
            let dir = entry.path().join("Zomboid");
            if dir.is_dir() {
                return Some(dir);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reg_query_output() {
        let output = "\r\nHKEY_CURRENT_USER\\Software\\Valve\\Steam\r\n    SteamPath    REG_SZ    c:/program files (x86)/steam\r\n\r\n";
        assert_eq!(
            parse_reg_steam_path(output),
            Some(PathBuf::from("c:/program files (x86)/steam"))
        );
        assert_eq!(parse_reg_steam_path("no value here"), None);
    }

    /// Smoke test: when a Steam install is visible, locate() must succeed
    /// and the workshop content path must target PZ's app ID. Tolerates
    /// machines without Steam.
    #[test]
    fn locate_smoke() {
        let Ok(paths) = locate() else {
            eprintln!("no Steam install visible; skipping");
            return;
        };
        eprintln!("{paths:#?}");
        assert!(paths
            .workshop_content
            .to_string_lossy()
            .ends_with(&format!("steamapps/workshop/content/{PZ_APP_ID}")));
    }
}
