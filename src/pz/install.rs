//! The PZ game install: the facts of an install directory, whatever put
//! the files there.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::platform;

use super::{appmanifest, probe, APP_ID};

/// The facts of one game install directory, wherever the files came
/// from.
pub struct GameInstall {
    /// The directory holding the game files.
    pub dir: PathBuf,
    /// Version the game reports for itself.
    pub version: probe::GameVersionInfo,
    /// Steam build id from the appmanifest, when present.
    pub buildid: Option<u64>,
}

/// Probe a game install directory for its version and buildid; briefly
/// spawns the headless server found there.
pub fn probe_install(dir: &Path) -> Result<GameInstall> {
    Ok(GameInstall {
        dir: dir.to_path_buf(),
        version: probe::detect_dir(dir)?,
        buildid: appmanifest::buildid_for_install(dir, APP_ID),
    })
}

/// The Zomboid user data dir for the current user, when one exists.
pub fn user_dir() -> Option<PathBuf> {
    if platform::is_windows() {
        if let Ok(profile) = std::env::var("USERPROFILE") {
            let dir = PathBuf::from(profile).join("Zomboid");
            if dir.is_dir() {
                return Some(platform::normalize_path(dir));
            }
        }
    }
    if platform::is_wsl() {
        if let Ok(entries) = std::fs::read_dir("/mnt/c/Users") {
            for entry in entries.flatten() {
                let dir = entry.path().join("Zomboid");
                if dir.is_dir() {
                    return Some(platform::normalize_path(dir));
                }
            }
        }
    }
    None
}
