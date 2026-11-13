//! The snapshot store: the stable territory yapzm owns.
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
//! (`/mnt/<letter>/Users/<profile>/AppData/Roaming/yapzm`), so both
//! Windows and WSL invocations share one store on a native filesystem.
//!
//! The directory name defaults to `yapzm` and can be swapped (e.g. to
//! `yapzm-dev` or `yapzm-test`) to keep development and testing
//! stores separate from the real one: set `YAPZM_APP_DIR`, or run the
//! binary under a variant name (`yapzm-dev`, `yapzm-test`, ...) — a
//! copy or a `cargo run --bin` target both work.

pub mod games;
pub mod install;
pub mod instances;
pub mod meta;
pub mod mods;
pub mod modpack;
pub mod snapshots;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::config;
use crate::fsops::sanitize_name;
use crate::platform;
use crate::info;

pub use games::StoredGame;
pub use install::{
    mod_install_status, Activation, InstallOutcome, InstalledManifest, InstalledMod, ModRecord,
    ModStatus, SelectedMod,
};
pub use instances::GameInstance;
pub use meta::Source;
pub use mods::StoredMod;
pub use modpack::{ModpackManifest, PackedModpack};
pub use snapshots::SnapshotRef;

/// Store errors callers may want to match on.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "game version {version} is not stored; see `yapzm version list` \
         or download it with `yapzm version download`"
    )]
    VersionNotStored { version: String },
    #[error("instance {name} already exists at {dir}")]
    InstanceExists { name: String, dir: PathBuf },
}

pub struct Store {
    root: PathBuf,
}

/// Resolve the store root.
///
/// Resolution must not depend on Steam being visible — Steam installs
/// come and go (moved libraries, leftover dirs, uninstalls), and a root
/// derived from the live install alone would silently move the store
/// and "lose" every stored version and instance. The order:
///
/// 1. `YAPZM_STORE`, when set — an explicit pin.
/// 2. An existing store among the candidate roots (under WSL: the
///    Windows profile on each automounted drive, then the platform data
///    dir). A root counts as a store when it actually holds content.
/// 3. The preferred location for a brand-new store: under WSL the
///    Windows profile on the drive of `game_dir` (the game install's
///    drive, so the store sits on a native filesystem next to it), else
///    the first automounted drive; elsewhere the platform data dir.
pub fn locate_root(game_dir: Option<&Path>) -> Result<PathBuf> {
    if let Some(root) = config::store_root_override() {
        return Ok(platform::normalize_path(root));
    }
    let candidates = candidate_roots(game_dir);
    if let Some(existing) = existing_store(&candidates) {
        return Ok(existing);
    }
    default_root(&candidates)
}

/// The first candidate that holds an actual store, if any.
fn existing_store(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|root| is_store(root)).cloned()
}

/// Places a store could live, best guess first: every WSL drive's
/// Windows profile (the live install's drive first), then the drive
/// roots as a legacy fallback, then the platform data dir.
fn candidate_roots(game_dir: Option<&Path>) -> Vec<PathBuf> {
    candidate_roots_named(&config::app_dir_name(), game_dir)
}

/// `candidate_roots` with the app data dir name supplied, so tests can
/// exercise the layout without touching process state.
fn candidate_roots_named(name: &str, game_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if platform::is_wsl() {
        let mut drives = Vec::new();
        if let Some(drive) = game_dir.and_then(platform::wsl_drive_root) {
            drives.push(drive);
        }
        for drive in platform::wsl_drives() {
            if !drives.contains(&drive) {
                drives.push(drive);
            }
        }
        for drive in drives {
            if let Some(profile) = platform::windows_profile_on(&drive) {
                candidates.push(profile.join("AppData/Roaming").join(&name));
            }
            candidates.push(drive.join(&name));
        }
    }
    if let Some(data_dir) = dirs::data_dir() {
        candidates.push(data_dir.join(&name));
    }
    candidates
}

/// Where a brand-new store goes: the first candidate, unless WSL has no
/// drives to anchor it on — then the platform data dir.
fn default_root(candidates: &[PathBuf]) -> Result<PathBuf> {
    if platform::is_wsl() {
        if let Some(root) = candidates.first() {
            return Ok(root.clone());
        }
    }
    let dir = dirs::data_dir().context("could not resolve the user data directory")?;
    Ok(dir.join(config::app_dir_name()))
}

/// Does `root` hold an actual store? Directories alone prove nothing:
/// `Store::open` creates them eagerly, so an empty store left behind by
/// a failed resolution must not anchor resolution to itself.
fn is_store(root: &Path) -> bool {
    let has_entries = |dir: &Path| {
        dir.read_dir()
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
    };
    has_entries(&root.join("game"))
        || has_entries(&root.join("instance"))
        || has_entries(&root.join(".store/mods"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_the_first_existing_store_over_the_fallback() {
        let base = std::env::temp_dir().join(format!("yapzm-root-{}", std::process::id()));
        let empty = base.join("empty");
        let real = base.join("real");
        fs::create_dir_all(real.join("game/42.22.2")).unwrap();
        fs::write(real.join("game/42.22.2/game.json"), "{}").unwrap();

        // The empty root is not a store (a leftover of Store::open); the
        // root holding versions wins; the fallback is never reached.
        assert!(!is_store(&empty));
        assert!(is_store(&real));
        assert_eq!(
            resolve_among(&[empty.clone(), real.clone()], base.join("fallback")),
            real
        );

        // No store anywhere: the fallback is used.
        assert_eq!(resolve_among(&[empty], base.join("fallback")), base.join("fallback"));

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_store_with_any_content_kind_anchors_resolution() {
        let base = std::env::temp_dir().join(format!("yapzm-root2-{}", std::process::id()));
        for content in ["game/v1", "instance/a", ".store/mods/108600"] {
            let root = base.join(format!("s-{}", content.replace('/', "_")));
            fs::create_dir_all(root.join(content)).unwrap();
            fs::write(root.join(content).join("marker"), "").unwrap();
            assert!(is_store(&root), "failed for {content}");
        }
        fs::remove_dir_all(&base).unwrap();
    }

    /// The decision core of locate_root, exercised through the real
    /// production selector.
    fn resolve_among(candidates: &[PathBuf], fallback: PathBuf) -> PathBuf {
        existing_store(candidates).unwrap_or(fallback)
    }

    #[test]
    fn app_dir_override_names_every_candidate_root() {
        let roots = candidate_roots_named("yapzm-test", None);
        assert!(!roots.is_empty(), "no candidate roots resolved");
        assert!(
            roots.iter().all(|root| root.ends_with("yapzm-test")),
            "candidates not named by the override: {roots:?}"
        );
    }
}
