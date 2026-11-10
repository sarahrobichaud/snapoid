//! Component 5: Snapshot store.
//!
//! The stable territory snapoid owns. Steam owns the workshop folder and
//! the game install; this store holds captured copies Steam will never
//! touch.
//!
//! Game instances are the first-class entity: a full copy of the game
//! files for one specific version, laid out as `<root>/instance/<version>/`
//! with an `instance.json` metadata sidecar. Workshop item captures
//! (`<root>/store/<pfid>/<timeupdated>/`) are scaffolded below but not
//! implemented yet.
//!
//! Under WSL the store moves to the Windows side, on the same drive as
//! the game install (`/mnt/<letter>/Users/<profile>/AppData/Roaming/snapoid`),
//! so both Windows and WSL invocations share one store on a native
//! filesystem instead of straddling the 9p boundary.

use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{debug, info, warn};

/// Top-level entries of the game install that never belong to an instance:
/// standalone mods, and home-folder data the game may have dropped into
/// its working directory.
const EXCLUDED_TOP_LEVEL: &[&str] = &["mods", "Zomboid"];

/// Entries of C:\Users that are not user profiles.
const NON_PROFILE_USERS: &[&str] = &["public", "default", "default user", "all users"];

/// Metadata sidecar written into every instance directory.
#[derive(Debug, Serialize, Deserialize)]
struct InstanceMeta {
    version: String,
    #[serde(default)]
    revision: Option<String>,
    /// Unix seconds at capture time.
    created: u64,
}

/// A stored copy of the game files for one specific version.
#[derive(Debug)]
pub struct GameInstance {
    pub version: String,
    pub revision: Option<String>,
    pub dir: PathBuf,
    /// Unix seconds at capture time; 0 when unknown.
    pub created: u64,
}

pub struct Store {
    root: PathBuf,
}

#[derive(Debug)]
pub struct SnapshotRef {
    pub item_id: u64,
    pub time_updated: u64,
}

impl Store {
    /// Open (creating if needed) the store root: %APPDATA%\snapoid on
    /// Windows, ~/.local/share/snapoid on Linux, and under WSL the Windows
    /// profile on the same drive as `game_dir` (pass the located game
    /// install when available).
    pub fn open(game_dir: Option<&Path>) -> Result<Self> {
        let root = match store_root(game_dir) {
            Some(root) => root,
            None => dirs::data_dir()
                .context("could not resolve the user data directory")?
                .join("snapoid"),
        };
        fs::create_dir_all(&root)
            .with_context(|| format!("creating store root {}", root.display()))?;
        let instances = Self::instances_dir_in(&root);
        fs::create_dir_all(&instances)
            .with_context(|| format!("creating instance dir {}", instances.display()))?;
        info!("store at {}", root.display());
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn instances_dir_in(root: &Path) -> PathBuf {
        root.join("instance")
    }

    /// Directory holding all game instances.
    pub fn instances_dir(&self) -> PathBuf {
        Self::instances_dir_in(&self.root)
    }

    /// Directory for one game instance: `instance/<version>/`.
    pub fn instance_dir(&self, version: &str) -> PathBuf {
        self.instances_dir().join(sanitize_version(version))
    }

    /// Copy the current game files into `instance/<version>/`.
    ///
    /// Only the game install itself is captured: mods and home-folder data
    /// are excluded. Re-snapshotting an existing version replaces it
    /// atomically via a staging directory.
    pub fn snapshot_game(
        &self,
        game_dir: &Path,
        version: &str,
        revision: Option<&str>,
    ) -> Result<GameInstance> {
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let safe = sanitize_version(version);
        let dir = self.instances_dir().join(&safe);
        let staging = self.instances_dir().join(format!(".staging-{safe}"));

        if staging.exists() {
            fs::remove_dir_all(&staging)
                .with_context(|| format!("clearing {}", staging.display()))?;
        }
        fs::create_dir_all(&staging)
            .with_context(|| format!("creating staging dir {}", staging.display()))?;

        let started = Instant::now();
        let (total_files, total_bytes) = measure_tree(game_dir, true)?;
        debug!(
            "snapshot: {total_files} file(s), {} to copy",
            crate::fmt::format_bytes(total_bytes)
        );
        let mut progress = CopyProgress::new(total_bytes);
        let files = copy_tree(game_dir, &staging, true, &mut progress)?;
        progress.finish();
        info!(
            "copied {files} file(s) ({}) in {:.1}s",
            crate::fmt::format_bytes(total_bytes),
            started.elapsed().as_secs_f32()
        );

        let meta = InstanceMeta {
            version: version.to_owned(),
            revision: revision.map(str::to_owned),
            created,
        };
        let meta_path = staging.join("instance.json");
        serde_json::to_writer_pretty(
            fs::File::create(&meta_path).context("writing instance.json")?,
            &meta,
        )
        .context("writing instance.json")?;

        swap_into_place(&staging, &dir)?;

        Ok(GameInstance {
            version: version.to_owned(),
            revision: revision.map(str::to_owned),
            dir,
            created,
        })
    }

    /// List stored game instances, sorted by version.
    pub fn list_instances(&self) -> Result<Vec<GameInstance>> {
        let dir = self.instances_dir();
        if !dir.is_dir() {
            return Ok(Vec::new());
        }

        let mut instances = Vec::new();
        for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
            let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
            let path = entry.path();
            if !path.is_dir()
                || path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            let meta = fs::read_to_string(path.join("instance.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<InstanceMeta>(&text).ok());
            instances.push(GameInstance {
                version: meta
                    .as_ref()
                    .map(|m| m.version.clone())
                    .unwrap_or_else(|| entry.file_name().to_string_lossy().into_owned()),
                revision: meta.as_ref().and_then(|m| m.revision.clone()),
                dir: path,
                created: meta.as_ref().map(|m| m.created).unwrap_or(0),
            });
        }

        instances.sort_by(|a, b| a.version.cmp(&b.version));
        Ok(instances)
    }

    /// Copy the current on-disk state of one workshop item into the store,
    /// keyed by its current timeupdated from the ACF.
    pub fn snapshot(&self, _item_id: u64) -> anyhow::Result<SnapshotRef> {
        todo!("snapshot capture")
    }

    /// List stored snapshots for one item, or all items if None.
    pub fn list(&self, _item_id: Option<u64>) -> anyhow::Result<Vec<SnapshotRef>> {
        todo!("store listing")
    }

    /// Prune old snapshots, keeping the newest `keep` per item.
    pub fn prune(&self, _keep: usize) -> anyhow::Result<()> {
        todo!("store pruning")
    }
}

/// Resolve the store root for WSL: the Windows profile on the same drive
/// as `game_dir`. Returns None when not on WSL or when the game install
/// is not on a /mnt drive (e.g. a Linux-side Steam), in which case the
/// caller falls back to the platform data dir.
fn store_root(game_dir: Option<&Path>) -> Option<PathBuf> {
    if !is_wsl() {
        return None;
    }
    let drive = wsl_drive_root(game_dir?)?;
    Some(match windows_profile_on(&drive) {
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

/// True when running under WSL (kernel release reports Microsoft/WSL).
fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|release| {
            let release = release.to_ascii_lowercase();
            release.contains("microsoft") || release.contains("wsl")
        })
        .unwrap_or(false)
}

/// For a path under a WSL drive automount (/mnt/<letter>/...), return the
/// drive root /mnt/<letter>. Returns None for anything else, so a
/// Linux-side game install keeps the store on the Linux side.
fn wsl_drive_root(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    match components.next()? {
        std::path::Component::RootDir => {}
        _ => return None,
    }
    match components.next()? {
        std::path::Component::Normal(name) if name == "mnt" => {}
        _ => return None,
    }
    match components.next()? {
        std::path::Component::Normal(name) => {
            let letter = name.to_string_lossy();
            if letter.len() == 1 && letter.chars().all(|c| c.is_ascii_alphabetic()) {
                Some(PathBuf::from("/mnt").join(letter.into_owned()))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Find a Windows user profile on the given drive: entries of
/// <drive>/Users, skipping non-profile dirs, that have an
/// AppData/Roaming folder. Prefers the profile that owns the Zomboid
/// user dir, since that is the user snapoid is managing; otherwise the
/// first (sorted) candidate.
fn windows_profile_on(drive: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = fs::read_dir(drive.join("Users"))
        .ok()?
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|path| {
            let Some(name) = path.file_name() else {
                return false;
            };
            let name = name.to_string_lossy().to_lowercase();
            !NON_PROFILE_USERS.contains(&name.as_str()) && path.join("AppData/Roaming").is_dir()
        })
        .collect();
    candidates.sort();
    candidates
        .iter()
        .find(|profile| profile.join("Zomboid").is_dir())
        .or_else(|| candidates.first())
        .cloned()
}

/// Replace `dir` with `staging`, keeping the old copy until the swap
/// succeeds so a failed rename cannot lose an existing instance.
fn swap_into_place(staging: &Path, dir: &Path) -> Result<()> {
    let parent = dir
        .parent()
        .context("instance dir has no parent")?
        .to_path_buf();
    let retired = parent.join(format!(
        ".retired-{}",
        dir.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));

    if dir.exists() {
        if retired.exists() {
            fs::remove_dir_all(&retired)
                .with_context(|| format!("clearing {}", retired.display()))?;
        }
        fs::rename(dir, &retired)
            .with_context(|| format!("retiring {}", dir.display()))?;
        if let Err(error) = fs::rename(staging, dir) {
            fs::rename(&retired, dir)
                .with_context(|| format!("restoring {}", dir.display()))?;
            return Err(error).with_context(|| format!("activating {}", dir.display()));
        }
        fs::remove_dir_all(&retired)
            .with_context(|| format!("removing {}", retired.display()))?;
    } else {
        fs::rename(staging, dir)
            .with_context(|| format!("activating {}", dir.display()))?;
    }
    Ok(())
}

/// Live copy progress rendered on stderr as `X / Y (p%)  N files`,
/// updated in place with `\r`. Silent when stderr is redirected.
struct CopyProgress {
    total_bytes: u64,
    copied_bytes: u64,
    copied_files: u64,
    last_render: Option<Instant>,
    enabled: bool,
}

const RENDER_INTERVAL: Duration = Duration::from_millis(100);

impl CopyProgress {
    /// Live progress on a terminal; silent when stderr is redirected.
    fn new(total_bytes: u64) -> Self {
        Self {
            total_bytes,
            copied_bytes: 0,
            copied_files: 0,
            last_render: None,
            enabled: std::io::stderr().is_terminal(),
        }
    }

    fn disabled() -> Self {
        Self {
            total_bytes: 0,
            copied_bytes: 0,
            copied_files: 0,
            last_render: None,
            enabled: false,
        }
    }

    fn step(&mut self, bytes: u64) {
        self.copied_files += 1;
        self.copied_bytes += bytes;
        let now = Instant::now();
        if self.enabled
            && self
                .last_render
                .map_or(true, |last| now.duration_since(last) >= RENDER_INTERVAL)
        {
            self.last_render = Some(now);
            self.render();
        }
    }

    fn finish(&mut self) {
        if self.enabled {
            self.render();
            eprintln!();
        }
    }

    fn render(&self) {
        let percent = if self.total_bytes == 0 {
            100
        } else {
            (self.copied_bytes * 100 / self.total_bytes).min(100)
        };
        eprint!(
            "\r  {} / {} ({percent}%)  {} files   ",
            crate::fmt::format_bytes(self.copied_bytes),
            crate::fmt::format_bytes(self.total_bytes),
            self.copied_files
        );
    }
}

/// Recursively measure `src`: (file count, total bytes), using the same
/// exclusions as [`copy_tree`] so progress totals match what gets copied.
fn measure_tree(src: &Path, top_level: bool) -> Result<(u64, u64)> {
    let mut files = 0u64;
    let mut bytes = 0u64;
    for entry in fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry.with_context(|| format!("reading {}", src.display()))?;
        let name = entry.file_name();
        if top_level
            && EXCLUDED_TOP_LEVEL
                .iter()
                .any(|x| x.eq_ignore_ascii_case(&name.to_string_lossy()))
        {
            continue;
        }

        let file_type = entry
            .file_type()
            .with_context(|| format!("reading {}", entry.path().display()))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let (dir_files, dir_bytes) = measure_tree(&entry.path(), false)?;
            files += dir_files;
            bytes += dir_bytes;
        } else {
            files += 1;
            bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    Ok((files, bytes))
}

/// Recursively copy `src` into `dst`, reporting each file to `progress`.
/// At the top level, entries in [`EXCLUDED_TOP_LEVEL`] are skipped.
/// Symlinks are skipped with a warning: the game install is not expected
/// to contain any, and copying them as links risks dangling or
/// privileged-link errors.
fn copy_tree(src: &Path, dst: &Path, top_level: bool, progress: &mut CopyProgress) -> Result<usize> {
    fs::create_dir_all(dst).with_context(|| format!("creating {}", dst.display()))?;
    let mut files = 0usize;
    for entry in fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry.with_context(|| format!("reading {}", src.display()))?;
        let name = entry.file_name();
        if top_level
            && EXCLUDED_TOP_LEVEL
                .iter()
                .any(|x| x.eq_ignore_ascii_case(&name.to_string_lossy()))
        {
            debug!("skipping {name:?} (excluded)");
            continue;
        }

        let from = entry.path();
        let to = dst.join(&name);
        let file_type = entry
            .file_type()
            .with_context(|| format!("reading {}", from.display()))?;

        if file_type.is_symlink() {
            warn!("skipping symlink {}", from.display());
        } else if file_type.is_dir() {
            fs::create_dir_all(&to)
                .with_context(|| format!("creating {}", to.display()))?;
            files += copy_tree(&from, &to, false, progress)?;
        } else {
            let copied = fs::copy(&from, &to)
                .with_context(|| format!("copying {}", from.display()))?;
            progress.step(copied);
            files += 1;
            if files % 2_000 == 0 {
                debug!("{files} files copied");
            }
        }
    }
    Ok(files)
}

/// Reduce a version string to safe path characters.
fn sanitize_version(version: &str) -> String {
    let safe: String = version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        "unknown".to_owned()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_drive_root_from_mnt_paths() {
        assert_eq!(
            wsl_drive_root(Path::new("/mnt/c/Program Files (x86)/Steam/game")),
            Some(PathBuf::from("/mnt/c"))
        );
        assert_eq!(wsl_drive_root(Path::new("/mnt/d")), Some(PathBuf::from("/mnt/d")));
        // Linux-side installs keep the store on the Linux side.
        assert_eq!(wsl_drive_root(Path::new("/home/user/.steam/game")), None);
        // Non-drive automounts are not drives.
        assert_eq!(wsl_drive_root(Path::new("/mnt/cdrom/game")), None);
        assert_eq!(wsl_drive_root(Path::new("/mnt")), None);
        assert_eq!(wsl_drive_root(Path::new("relative/path")), None);
    }

    #[test]
    fn finds_windows_profile_on_drive() -> anyhow::Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-prof-{}", std::process::id()));
        let users = base.join("Users");
        fs::create_dir_all(users.join("Public"))?;
        fs::create_dir_all(users.join("Alpha/AppData/Roaming"))?;
        fs::create_dir_all(users.join("Beta/AppData/Roaming"))?;
        fs::create_dir_all(users.join("Gamma"))?; // profile without AppData

        // No Zomboid owner yet: first sorted profile wins.
        assert_eq!(windows_profile_on(&base), Some(users.join("Alpha")));

        // The Zomboid owner is preferred over sort order.
        fs::create_dir_all(users.join("Beta/Zomboid"))?;
        assert_eq!(windows_profile_on(&base), Some(users.join("Beta")));

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn sanitizes_version_strings() {
        assert_eq!(sanitize_version("42.22.2"), "42.22.2");
        assert_eq!(sanitize_version("42.10 beta"), "42.10_beta");
        assert_eq!(sanitize_version(""), "unknown");
    }

    #[test]
    fn copies_tree_excluding_top_level_entries() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-test-{}", std::process::id()));
        let src = base.join("game");
        let dst = base.join("out");
        for rel in [
            "media/lua",
            "mods/standalone",
            "Zomboid/saves",
            "jre64/bin",
        ] {
            fs::create_dir_all(src.join(rel))?;
        }
        fs::write(src.join("projectzomboid.jar"), "jar")?;
        fs::write(src.join("media/lua/a.lua"), "lua")?;
        fs::write(src.join("mods/standalone/mod.info"), "mod")?;
        fs::write(src.join("Zomboid/saves/save.sav"), "save")?;
        fs::write(src.join("jre64/bin/java.exe"), "jvm")?;

        let files = copy_tree(&src, &dst, true, &mut CopyProgress::disabled())?;
        assert_eq!(files, 3, "mods and Zomboid contents must be excluded");
        assert!(dst.join("projectzomboid.jar").is_file());
        assert!(dst.join("media/lua/a.lua").is_file());
        assert!(dst.join("jre64/bin/java.exe").is_file());
        assert!(!dst.join("mods").exists());
        assert!(!dst.join("Zomboid").exists());

        // Measurement mirrors the copy exclusions (3+3+3 bytes).
        assert_eq!(measure_tree(&src, true)?, (3, 9));

        fs::remove_dir_all(&base)?;
        Ok(())
    }
}
