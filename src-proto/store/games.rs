//! Stored game versions: capture, adopt, and list.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context as _, Result};

use crate::fmt;
use crate::fsops::{
    ProgressFactory, copy_tree, measure_tree, move_dir, sanitize_name, swap_into_place, unix_now,
};
use crate::pz;
use crate::store::Store;
use crate::store::meta::{self, GameMeta};
use crate::{debug, info};

/// A stored copy of the game files for one version.
#[derive(Debug)]
pub struct StoredGame {
    pub version: String,
    pub revision: Option<String>,
    /// Steam build ID, when known (latest downloads record it from the
    /// appmanifest; depot downloads don't have one). Used to detect that
    /// a `latest` download is already stored.
    pub buildid: Option<u64>,
    /// Depot manifest ID, when known (pinned downloads record the
    /// registry's manifest). Also used by the `latest` skip check — it
    /// covers branch reverts, where an old manifest becomes current
    /// again.
    pub manifest: Option<u64>,
    pub dir: PathBuf,
    pub created: u64,
}

impl Store {
    /// Copy the current game files into `game/<version>/`.
    ///
    /// Only the game install itself is captured: mods and home-folder
    /// data are excluded. Re-snapshotting an existing version replaces
    /// it atomically via a staging directory. `buildid` is the Steam
    /// build the install is at (from its appmanifest), when known.
    pub fn snapshot_version(
        &self,
        game_dir: &Path,
        version: &str,
        revision: Option<&str>,
        buildid: Option<u64>,
        progress: ProgressFactory,
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
        let (total_files, total_bytes) = measure_tree(game_dir, pz::EXCLUDED_INSTALL_ENTRIES)?;
        debug!(
            "snapshot: {total_files} file(s), {} to copy",
            fmt::format_bytes(total_bytes)
        );
        let mut progress = progress(total_bytes);
        let files = copy_tree(
            game_dir,
            &staging,
            pz::EXCLUDED_INSTALL_ENTRIES,
            progress.as_mut(),
        )?;
        progress.finish();
        info!(
            "copied {files} file(s) ({}) in {:.1}s",
            fmt::format_bytes(total_bytes),
            started.elapsed().as_secs_f32()
        );

        meta::write(
            &staging.join("game.json"),
            &GameMeta {
                version: version.to_owned(),
                revision: revision.map(str::to_owned),
                buildid,
                manifest: None,
                created,
            },
        )?;

        swap_into_place(&staging, &dir)?;

        Ok(StoredGame {
            version: version.to_owned(),
            revision: revision.map(str::to_owned),
            buildid,
            manifest: None,
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
        manifest: Option<u64>,
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

        meta::write(
            &dir.join("game.json"),
            &GameMeta {
                version: version.to_owned(),
                revision: revision.map(str::to_owned),
                buildid,
                manifest,
                created,
            },
        )?;

        Ok(StoredGame {
            version: version.to_owned(),
            revision: revision.map(str::to_owned),
            buildid,
            manifest,
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
        let game_meta = match meta::read::<GameMeta>(&path)? {
            Some(mut game_meta) => {
                game_meta.buildid = Some(buildid);
                if game_meta.revision.is_none() {
                    game_meta.revision = revision.map(str::to_owned);
                }
                game_meta
            }
            None => GameMeta {
                version: version.to_owned(),
                revision: revision.map(str::to_owned),
                buildid: Some(buildid),
                manifest: None,
                created: unix_now(),
            },
        };
        meta::write(&path, &game_meta)
    }

    /// List stored game versions, sorted by version string.
    pub fn list_games(&self) -> Result<Vec<StoredGame>> {
        let dir = self.games_dir();
        if !dir.is_dir() {
            return Ok(Vec::new());
        }

        let mut games = Vec::new();
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
            let game_meta = meta::read::<GameMeta>(&path.join("game.json"))
                .ok()
                .flatten();
            games.push(StoredGame {
                version: game_meta
                    .as_ref()
                    .map(|m| m.version.clone())
                    .unwrap_or_else(|| entry.file_name().to_string_lossy().into_owned()),
                revision: game_meta.as_ref().and_then(|m| m.revision.clone()),
                buildid: game_meta.as_ref().and_then(|m| m.buildid),
                manifest: game_meta.as_ref().and_then(|m| m.manifest),
                dir: path,
                created: game_meta.as_ref().map(|m| m.created).unwrap_or(0),
            });
        }

        games.sort_by(|a, b| a.version.cmp(&b.version));
        Ok(games)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adopts_downloaded_dir_into_place() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-adopt-{}", std::process::id()));
        let store = Store::open_at(&base)?;

        let from = base.join("incoming");
        fs::create_dir_all(from.join("jre64/bin"))?;
        fs::write(from.join("projectzomboid.jar"), "jar")?;
        fs::write(from.join("jre64/bin/java.exe"), "jvm")?;

        let game =
            store.adopt_game_dir(&from, "42.22.2", Some("3d39148846"), Some(17_774_562), None)?;
        assert_eq!(game.version, "42.22.2");
        assert_eq!(game.revision.as_deref(), Some("3d39148846"));
        assert_eq!(game.buildid, Some(17_774_562));
        assert_eq!(game.manifest, None);
        assert_eq!(game.dir, store.game_dir("42.22.2"));
        assert!(game.dir.join("projectzomboid.jar").is_file());
        assert!(game.dir.join("game.json").is_file());
        assert!(!from.exists(), "source must be consumed");
        assert!(store.list_games()?.iter().any(|g| g.version == "42.22.2"));

        // Re-adopting the same version replaces it.
        fs::create_dir_all(&from)?;
        fs::write(from.join("projectzomboid.jar"), "new jar")?;
        store.adopt_game_dir(&from, "42.22.2", None, None, Some(2378094061158458897))?;
        assert_eq!(
            fs::read_to_string(store.game_dir("42.22.2").join("projectzomboid.jar"))?,
            "new jar"
        );
        // The re-adopt recorded the manifest for the latest check.
        let games = store.list_games()?;
        assert_eq!(games[0].manifest, Some(2378094061158458897));

        fs::remove_dir_all(&base)?;
        Ok(())
    }
}
