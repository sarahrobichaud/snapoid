//! Component 5: Snapshot store.
//!
//! The stable territory snapoid owns. Steam owns the workshop folder and
//! the game install; this store holds captured copies Steam will never
//! touch.
//!
//! Layout:
//!   <root>/game/<version>/     stored game files, one copy per version,
//!                              with a game.json metadata sidecar
//!   <root>/instance/<name>/    a named instance whose game/ folder is a
//!                              link into ../../game/<version>, with an
//!                              instance.json sidecar
//!
//! Multiple instances can share one version. Under WSL the store moves to
//! the Windows side, on the same drive as the game install
//! (`/mnt/<letter>/Users/<profile>/AppData/Roaming/snapoid`), so both
//! Windows and WSL invocations share one store on a native filesystem.
//!
//! Submodules:
//! - [`download`] fetches game builds straight into the store's `game/`
//!   dirs, so the multi-gigabyte files never get copied twice.

pub mod download;

use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{debug, info, warn};

/// Top-level entries of the game install that never belong to a stored
/// version: standalone mods, and home-folder data the game may have
/// dropped into its working directory.
const EXCLUDED_TOP_LEVEL: &[&str] = &["mods", "Zomboid"];

/// Entries of C:\Users that are not user profiles.
const NON_PROFILE_USERS: &[&str] = &["public", "default", "default user", "all users"];

/// Metadata sidecar (game.json / instance.json).
#[derive(Debug, Serialize, Deserialize)]
struct Meta {
    version: String,
    #[serde(default)]
    revision: Option<String>,
    #[serde(default)]
    buildid: Option<u64>,
    /// Unix seconds at creation time.
    created: u64,
}

/// A stored copy of the game files for one version.
#[derive(Debug)]
pub struct StoredGame {
    pub version: String,
    pub revision: Option<String>,
    /// Steam build ID, when known (latest downloads record it from the
    /// appmanifest; depot downloads don't have one). Used to detect that
    /// a `latest` download is already stored.
    pub buildid: Option<u64>,
    pub dir: PathBuf,
    pub created: u64,
}

/// A game instance: a named slot whose game folder links to a version.
#[derive(Debug)]
pub struct GameInstance {
    /// Instance name; also the name of its folder.
    pub name: String,
    /// Game version the instance is linked to.
    pub version: String,
    pub revision: Option<String>,
    pub dir: PathBuf,
    /// The instance's linked game folder (instance/<name>/game).
    pub game_dir: PathBuf,
    /// Unix seconds at creation time; 0 when unknown.
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
        Self::create(root)
    }

    #[cfg(test)]
    fn open_at(root: &Path) -> Result<Self> {
        Self::create(root.to_path_buf())
    }

    fn create(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)
            .with_context(|| format!("creating store root {}", root.display()))?;
        for dir in [Self::games_dir_in(&root), Self::instances_dir_in(&root)] {
            fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        info!("store at {}", root.display());
        Ok(Self { root })
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

    /// Directory holding all stored game versions.
    pub fn games_dir(&self) -> PathBuf {
        Self::games_dir_in(&self.root)
    }

    /// Directory holding all game instances.
    pub fn instances_dir(&self) -> PathBuf {
        Self::instances_dir_in(&self.root)
    }

    /// Directory of one stored game version: `game/<version>/`.
    pub fn game_dir(&self, version: &str) -> PathBuf {
        self.games_dir().join(sanitize_name(version))
    }

    /// Directory of one game instance: `instance/<name>/`.
    pub fn instance_dir(&self, name: &str) -> PathBuf {
        self.instances_dir().join(sanitize_name(name))
    }

    /// Copy the current game files into `game/<version>/`.
    ///
    /// Only the game install itself is captured: mods and home-folder data
    /// are excluded. Re-snapshotting an existing version replaces it
    /// atomically via a staging directory.
    pub fn snapshot_version(
        &self,
        game_dir: &Path,
        version: &str,
        revision: Option<&str>,
    ) -> Result<StoredGame> {
        let created = unix_now();
        let safe = sanitize_name(version);
        let dir = self.games_dir().join(&safe);
        let staging = self.games_dir().join(format!(".staging-{safe}"));

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

        write_meta(
            &staging.join("game.json"),
            &Meta {
                version: version.to_owned(),
                revision: revision.map(str::to_owned),
                buildid: None,
                created,
            },
        )?;

        swap_into_place(&staging, &dir)?;

        Ok(StoredGame {
            version: version.to_owned(),
            revision: revision.map(str::to_owned),
            buildid: None,
            dir,
            created,
        })
    }

    /// Move an externally populated game directory into `game/<version>/`
    /// and write its meta file.
    ///
    /// Used by the downloader, which installs builds straight into the
    /// store instead of copying them from the live install. `from` is
    /// consumed: renamed when the filesystems match, copied and removed
    /// otherwise. Re-adopting an existing version replaces it, with the
    /// old copy kept as a dot-prefixed sibling until the new one is in
    /// place.
    pub fn adopt_game_dir(
        &self,
        from: &Path,
        version: &str,
        revision: Option<&str>,
        buildid: Option<u64>,
    ) -> Result<StoredGame> {
        let created = unix_now();
        let safe = sanitize_name(version);
        let dir = self.games_dir().join(&safe);

        if from != dir {
            if dir.exists() {
                let retired = self.games_dir().join(format!(".retired-{safe}"));
                if retired.exists() {
                    fs::remove_dir_all(&retired)
                        .with_context(|| format!("clearing {}", retired.display()))?;
                }
                fs::rename(&dir, &retired)
                    .with_context(|| format!("retiring {}", dir.display()))?;
                if let Err(error) = move_dir(from, &dir) {
                    fs::rename(&retired, &dir)
                        .with_context(|| format!("restoring {}", dir.display()))?;
                    return Err(error);
                }
                fs::remove_dir_all(&retired)
                    .with_context(|| format!("removing {}", retired.display()))?;
            } else {
                move_dir(from, &dir)?;
            }
        }

        write_meta(
            &dir.join("game.json"),
            &Meta {
                version: version.to_owned(),
                revision: revision.map(str::to_owned),
                buildid,
                created,
            },
        )?;

        Ok(StoredGame {
            version: version.to_owned(),
            revision: revision.map(str::to_owned),
            buildid,
            dir,
            created,
        })
    }

    /// Record the Steam buildid for an already-stored version, creating
    /// the meta when it does not exist yet (versions stored before meta
    /// writing). `revision` fills a missing revision fingerprint.
    pub fn set_game_buildid(
        &self,
        version: &str,
        revision: Option<&str>,
        buildid: u64,
    ) -> Result<()> {
        let path = self.game_dir(version).join("game.json");
        let meta = match read_meta(&path)? {
            Some(mut meta) => {
                meta.buildid = Some(buildid);
                if meta.revision.is_none() {
                    meta.revision = revision.map(str::to_owned);
                }
                meta
            }
            None => Meta {
                version: version.to_owned(),
                revision: revision.map(str::to_owned),
                buildid: Some(buildid),
                created: unix_now(),
            },
        };
        write_meta(&path, &meta)
    }

    pub fn list_games(&self) -> Result<Vec<StoredGame>> {
        let dir = self.games_dir();
        if !dir.is_dir() {
            return Ok(Vec::new());
        }

        let mut games = Vec::new();
        for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
            let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
            let path = entry.path();
            if !path.is_dir() || path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            let meta = read_meta(&path.join("game.json")).ok().flatten();
            games.push(StoredGame {
                version: meta
                    .as_ref()
                    .map(|m| m.version.clone())
                    .unwrap_or_else(|| entry.file_name().to_string_lossy().into_owned()),
                revision: meta.as_ref().and_then(|m| m.revision.clone()),
                buildid: meta.as_ref().and_then(|m| m.buildid),
                dir: path,
                created: meta.as_ref().map(|m| m.created).unwrap_or(0),
            });
        }

        games.sort_by(|a, b| a.version.cmp(&b.version));
        Ok(games)
    }

    /// Create `instance/<name>` whose game folder links to
    /// `game/<version>`.
    pub fn instance_new(&self, name: &str, version: &str) -> Result<GameInstance> {
        let game = self.game_dir(version);
        if !game.is_dir() {
            anyhow::bail!("game version {version} is not stored; run `snapoid version snapshot` first");
        }
        let revision = read_meta(&game.join("game.json"))
            .ok()
            .flatten()
            .and_then(|m| m.revision);

        let dir = self.instance_dir(name);
        if dir.exists() {
            if dir.join("instance.json").is_file() {
                anyhow::bail!("instance {name} already exists at {}", dir.display());
            }
            // No metadata: a leftover from a failed creation. snapoid owns
            // this folder, so clear it and start fresh.
            warn!("removing leftover instance dir {}", dir.display());
            fs::remove_dir_all(&dir)
                .with_context(|| format!("clearing {}", dir.display()))?;
        }
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

        let link = dir.join("game");
        if let Err(error) =
            create_link(&link, &PathBuf::from("../../game").join(sanitize_name(version)))
        {
            // Don't leave a half-created instance behind.
            let _ = fs::remove_dir_all(&dir);
            return Err(error).with_context(|| format!("linking instance {name} to version {version}"));
        }

        let created = unix_now();
        write_meta(
            &dir.join("instance.json"),
            &Meta {
                version: version.to_owned(),
                revision: revision.clone(),
                buildid: None,
                created,
            },
        )?;

        Ok(GameInstance {
            name: name.to_owned(),
            version: version.to_owned(),
            revision,
            dir,
            game_dir: link,
            created,
        })
    }

    /// List stored game instances, sorted by name.
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
            let meta = read_meta(&path.join("instance.json")).ok().flatten();
            instances.push(GameInstance {
                name: entry.file_name().to_string_lossy().into_owned(),
                version: meta
                    .as_ref()
                    .map(|m| m.version.clone())
                    .unwrap_or_else(|| "unknown".to_owned()),
                revision: meta.as_ref().and_then(|m| m.revision.clone()),
                game_dir: path.join("game"),
                dir: path,
                created: meta.as_ref().map(|m| m.created).unwrap_or(0),
            });
        }

        instances.sort_by(|a, b| a.name.cmp(&b.name));
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
pub(crate) fn is_wsl() -> bool {
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

/// Create a directory link at `link` pointing at `target` (a path
/// relative to the link's directory).
///
/// Under WSL with the link on a Windows drive, the link is created as a
/// junction through cmd.exe interop: junctions need no privileges and are
/// followed by Windows processes (the game). Native Linux uses a
/// relative symlink; native Windows uses mklink /J, which needs an
/// absolute backslash target (its parser reads `/..` as a switch).
fn create_link(link: &Path, target: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        if is_wsl() && wsl_drive_root(link).is_some() {
            let absolute = link.parent().map(|base| base.join(target));
            let absolute = normalize_path(&absolute.unwrap_or_else(|| target.to_path_buf()));
            if junction_via_interop(link, &absolute)? {
                return Ok(());
            }
        }
        std::os::unix::fs::symlink(target, link)
            .with_context(|| format!("linking {} -> {}", link.display(), target.display()))?;
        Ok(())
    }
    #[cfg(windows)]
    {
        let absolute = link.parent().map(|base| base.join(target));
        let absolute = normalize_path(&absolute.unwrap_or_else(|| target.to_path_buf()));
        let absolute = absolute.to_string_lossy().replace('/', "\\");
        let status = Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(&absolute)
            .status()
            .context("running mklink")?;
        if !status.success() {
            anyhow::bail!("mklink /J {} {} failed", link.display(), absolute);
        }
        Ok(())
    }
}

/// Resolve `..` components lexically, without touching the filesystem.
fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            component => out.push(component.as_os_str()),
        }
    }
    out
}

/// Try to create a junction through WSL↔Windows interop, converting both
/// paths to their Windows form with wslpath. Returns Ok(false) when
/// interop is unavailable (no wslpath or cmd.exe).
#[cfg(unix)]
fn junction_via_interop(link: &Path, target: &Path) -> Result<bool> {
    let (Some(win_link), Some(win_target)) = (wslpath(link), wslpath(target)) else {
        return Ok(false);
    };
    match Command::new("cmd.exe")
        .args(["/c", "mklink", "/J"])
        .arg(&win_link)
        .arg(&win_target)
        .status()
    {
        Ok(status) if status.success() => {
            debug!("junction {win_link} -> {win_target}");
            Ok(true)
        }
        Ok(status) => anyhow::bail!("mklink /J failed ({status})"),
        Err(_) => Ok(false),
    }
}

/// Convert a path to its Windows form via wslpath; None when unavailable.
#[cfg(unix)]
pub(crate) fn wslpath(path: &Path) -> Option<String> {
    let output = Command::new("wslpath").arg("-w").arg(path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// Replace `dir` with `staging`, keeping the old copy until the swap
/// succeeds so a failed rename cannot lose an existing instance.
/// Move a directory: rename when the filesystems match, copy and remove
/// the source when they do not (e.g. steamcmd's download cache on another
/// drive than the store).
fn move_dir(from: &Path, to: &Path) -> Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    let mut progress = CopyProgress::disabled();
    copy_tree(from, to, true, &mut progress)?;
    fs::remove_dir_all(from).with_context(|| format!("removing {}", from.display()))?;
    Ok(())
}

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

/// Reduce an instance name to safe path characters.
fn sanitize_name(name: &str) -> String {
    let safe: String = name
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

/// Read a metadata sidecar; None when it does not exist.
fn read_meta(path: &Path) -> Result<Option<Meta>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn write_meta(path: &Path, meta: &Meta) -> Result<()> {
    serde_json::to_writer_pretty(
        fs::File::create(path).with_context(|| format!("writing {}", path.display()))?,
        meta,
    )
    .with_context(|| format!("writing {}", path.display()))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
        assert_eq!(
            wsl_drive_root(Path::new("/mnt/d")),
            Some(PathBuf::from("/mnt/d"))
        );
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
    fn sanitizes_instance_names() {
        assert_eq!(sanitize_name("42.22.2"), "42.22.2");
        assert_eq!(sanitize_name("42.10 beta"), "42.10_beta");
        assert_eq!(sanitize_name(""), "unknown");
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

    #[test]
    fn adopts_downloaded_dir_into_place() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-adopt-{}", std::process::id()));
        let store = Store::open_at(&base)?;

        let from = base.join("incoming");
        fs::create_dir_all(from.join("jre64/bin"))?;
        fs::write(from.join("projectzomboid.jar"), "jar")?;
        fs::write(from.join("jre64/bin/java.exe"), "jvm")?;

        let game = store.adopt_game_dir(&from, "42.22.2", Some("3d39148846"), Some(17_774_562))?;
        assert_eq!(game.version, "42.22.2");
        assert_eq!(game.revision.as_deref(), Some("3d39148846"));
        assert_eq!(game.buildid, Some(17_774_562));
        assert_eq!(game.dir, store.game_dir("42.22.2"));
        assert!(game.dir.join("projectzomboid.jar").is_file());
        assert!(game.dir.join("game.json").is_file());
        assert!(!from.exists(), "source must be consumed");
        assert!(store.list_games()?.iter().any(|g| g.version == "42.22.2"));

        // Re-adopting the same version replaces it.
        fs::create_dir_all(&from)?;
        fs::write(from.join("projectzomboid.jar"), "new jar")?;
        store.adopt_game_dir(&from, "42.22.2", None, None)?;
        assert_eq!(
            fs::read_to_string(store.game_dir("42.22.2").join("projectzomboid.jar"))?,
            "new jar"
        );

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn creates_instance_linked_to_version() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-inst-{}", std::process::id()));
        let store = Store::open_at(&base)?;

        let game = store.game_dir("42.22.2");
        fs::create_dir_all(&game)?;
        fs::write(game.join("projectzomboid.jar"), "jar")?;

        let instance = store.instance_new("survivor", "42.22.2")?;
        assert_eq!(instance.version, "42.22.2");
        assert!(instance.game_dir.is_dir(), "game link should resolve");
        assert!(instance.game_dir.join("projectzomboid.jar").is_file());
        assert!(instance.dir.join("instance.json").is_file());

        // Unknown versions and duplicate names are rejected.
        assert!(store.instance_new("bad", "0.0").is_err());
        assert!(store.instance_new("survivor", "42.22.2").is_err());

        fs::remove_dir_all(&base)?;
        Ok(())
    }
}
