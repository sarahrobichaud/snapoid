//! The snapshot store: the stable territory snapoid owns.
//!
//! Steam owns the workshop folder and the game install; this store holds
//! captured copies Steam will never touch.
//!
//! Layout:
//!   <root>/game/<version>/     stored game files, one copy per version,
//!                              with a game.json metadata sidecar
//!   <root>/instance/<name>/    a named instance whose game/ folder is a
//!                              link into ../../game/<version>, with an
//!                              instance.json sidecar
//!   <root>/.store/mods/<id>/   stored workshop items (with the mods
//!                              they bundle), with a mod.json sidecar
//!
//! Multiple instances can share one version. Under WSL the store moves to
//! the Windows side, on the same drive as the game install
//! (`/mnt/<letter>/Users/<profile>/AppData/Roaming/snapoid`), so both
//! Windows and WSL invocations share one store on a native filesystem.

pub mod games;
pub mod install;
pub mod instances;
pub mod meta;
pub mod mods;
pub mod snapshots;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::fsops::sanitize_name;
use crate::platform;
use crate::{info, warn};

pub use games::StoredGame;
pub use install::{Activation, InstallOutcome, InstalledMod};
pub use instances::GameInstance;
pub use meta::Source;
pub use mods::StoredMod;
pub use snapshots::SnapshotRef;

/// Store errors callers may want to match on.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "game version {version} is not stored; see `snapoid version list` \
         or download it with `snapoid version download`"
    )]
    VersionNotStored { version: String },
    #[error("instance {name} already exists at {dir}")]
    InstanceExists { name: String, dir: PathBuf },
}

pub struct Store {
    root: PathBuf,
}

/// Resolve the store root: under WSL the Windows profile on the same
/// drive as `game_dir`, else the platform data dir.
pub fn locate_root(game_dir: Option<&Path>) -> Result<PathBuf> {
    if let Some(root) = store_root(game_dir) {
        return Ok(root);
    }
    let dir = dirs::data_dir().context("could not resolve the user data directory")?;
    Ok(dir.join("snapoid"))
}

/// Store root under WSL: the Windows profile on the same drive as
/// `game_dir`. None when not on WSL or when the game install is not on a
/// /mnt drive (e.g. a Linux-side Steam), in which case the caller falls
/// back to the platform data dir.
fn store_root(game_dir: Option<&Path>) -> Option<PathBuf> {
    if !platform::is_wsl() {
        return None;
    }
    let drive = platform::wsl_drive_root(game_dir?)?;
    Some(match platform::windows_profile_on(&drive) {
        Some(profile) => profile.join("AppData/Roaming").join("snapoid"),
        None => {
            warn!(
                "no Windows user profile found on {}; placing the store at the drive root",
                drive.display()
            );
            drive.join("snapoid")
        }
    })
}

impl Store {
    /// Open (creating if needed) the store at `root`.
    pub fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root)
            .with_context(|| format!("creating store root {}", root.display()))?;
        for dir in [
            Self::games_dir_in(root),
            Self::instances_dir_in(root),
            Self::mods_dir_in(root),
        ] {
            fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        info!("store at {}", root.display());
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    #[cfg(test)]
    pub fn open_at(root: &Path) -> Result<Self> {
        Self::open(root)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn games_dir_in(root: &Path) -> PathBuf {
        root.join("game")
    }

    fn instances_dir_in(root: &Path) -> PathBuf {
        root.join("instance")
    }

    fn mods_dir_in(root: &Path) -> PathBuf {
        root.join(".store/mods")
    }

    /// Directory holding all stored game versions.
    pub fn games_dir(&self) -> PathBuf {
        Self::games_dir_in(&self.root)
    }

    /// Directory holding all game instances.
    pub fn instances_dir(&self) -> PathBuf {
        Self::instances_dir_in(&self.root)
    }

    /// Directory holding all stored workshop mods.
    pub fn mods_dir(&self) -> PathBuf {
        Self::mods_dir_in(&self.root)
    }

    /// Directory of one stored game version: `game/<version>/`.
    pub fn game_dir(&self, version: &str) -> PathBuf {
        self.games_dir().join(sanitize_name(version))
    }

    /// Directory of one game instance: `instance/<name>/`.
    pub fn instance_dir(&self, name: &str) -> PathBuf {
        self.instances_dir().join(sanitize_name(name))
    }

    /// Directory of one stored workshop mod: `mods/<id>/`.
    pub fn mod_dir(&self, id: u64) -> PathBuf {
        self.mods_dir().join(id.to_string())
    }
}
