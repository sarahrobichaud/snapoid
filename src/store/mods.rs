//! Stored workshop mods: capture from the local workshop dir, adopt
//! steamcmd downloads, and read back.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context as _, Result};

use crate::fmt;
use crate::fsops::{
    ProgressFactory, copy_tree, measure_tree, move_dir, swap_into_place, unix_now,
};
use crate::store::Store;
use crate::store::meta::{self, ModMeta};
use crate::{debug, info};

/// A stored copy of one workshop item, with all the mods it bundles.
#[derive(Debug)]
pub struct StoredMod {
    pub id: u64,
    /// Steam's timeupdated for the item at capture time; 0 when unknown.
    pub time_updated: u64,
    pub dir: PathBuf,
    pub created: u64,
}

impl Store {
    /// Copy a workshop item dir from the local workshop into
    /// `mods/<id>/`, replacing any stored copy atomically via a staging
    /// directory. The source is left in place.
    pub fn save_mod_copy(
        &self,
        from: &Path,
        id: u64,
        time_updated: u64,
        progress: ProgressFactory,
    ) -> Result<StoredMod> {
        let created = unix_now();
        let dir = self.mod_dir(id);
        let staging = self.mods_dir().join(format!(".staging-{id}"));

        if staging.exists() {
            fs::remove_dir_all(&staging)
                .with_context(|| format!("clearing {}", staging.display()))?;
        }
        fs::create_dir_all(&staging)
            .with_context(|| format!("creating staging dir {}", staging.display()))?;

        let started = Instant::now();
        let (total_files, total_bytes) = measure_tree(from, &[])?;
        debug!(
            "mod save: {total_files} file(s), {} to copy",
            fmt::format_bytes(total_bytes)
        );
        let mut progress = progress(total_bytes);
        let files = copy_tree(from, &staging, &[], progress.as_mut())?;
        progress.finish();
        info!(
            "copied {files} file(s) ({}) in {:.1}s",
            fmt::format_bytes(total_bytes),
            started.elapsed().as_secs_f32()
        );

        meta::write(
            &staging.join("mod.json"),
            &ModMeta {
                id,
                time_updated,
                created,
            },
        )?;

        swap_into_place(&staging, &dir)?;

        Ok(StoredMod {
            id,
            time_updated,
            dir,
            created,
        })
    }

    /// Move an externally populated workshop item dir (a steamcmd
    /// download) into `mods/<id>/` and write its meta file. `from` is
    /// consumed: renamed when the filesystems match, copied and removed
    /// otherwise. Re-adopting an existing mod replaces it, with the old
    /// copy kept as a dot-prefixed sibling until the new one is in place.
    pub fn adopt_mod_dir(&self, from: &Path, id: u64, time_updated: u64) -> Result<StoredMod> {
        let created = unix_now();
        let dir = self.mod_dir(id);

        if from != dir {
            if dir.exists() {
                let retired = self.mods_dir().join(format!(".retired-{id}"));
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
            &dir.join("mod.json"),
            &ModMeta {
                id,
                time_updated,
                created,
            },
        )?;

        Ok(StoredMod {
            id,
            time_updated,
            dir,
            created,
        })
    }

    /// The stored copy of one workshop item, if any. A missing or
    /// unreadable meta counts as not stored, so a re-save can repair it.
    pub fn stored_mod(&self, id: u64) -> Result<Option<StoredMod>> {
        let dir = self.mod_dir(id);
        let mod_meta = meta::read::<ModMeta>(&dir.join("mod.json"))
            .ok()
            .flatten();
        Ok(mod_meta.map(|m| StoredMod {
            id: m.id,
            time_updated: m.time_updated,
            dir,
            created: m.created,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsops::noop_progress;

    #[test]
    fn saves_and_replaces_copy() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-modsave-{}", std::process::id()));
        let store = Store::open_at(&base)?;

        let from = base.join("workshop/123");
        fs::create_dir_all(from.join("mods/Test Mod"))?;
        fs::write(from.join("mods/Test Mod/mod.info"), "name=Test Mod\n")?;

        let stored = store.save_mod_copy(&from, 123, 1_700_000_000, noop_progress)?;
        assert_eq!(stored.id, 123);
        assert_eq!(stored.time_updated, 1_700_000_000);
        assert_eq!(stored.dir, store.mod_dir(123));
        assert!(stored.dir.join("mods/Test Mod/mod.info").is_file());
        assert!(stored.dir.join("mod.json").is_file());
        assert!(from.exists(), "disk source must be kept");

        // Re-saving replaces the stored copy.
        fs::write(from.join("mods/Test Mod/mod.info"), "name=Test Mod v2\n")?;
        store.save_mod_copy(&from, 123, 1_700_000_100, noop_progress)?;
        assert_eq!(
            fs::read_to_string(store.mod_dir(123).join("mods/Test Mod/mod.info"))?,
            "name=Test Mod v2\n"
        );
        assert_eq!(store.stored_mod(123)?.unwrap().time_updated, 1_700_000_100);
        assert!(store.stored_mod(456)?.is_none());

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn adopts_downloaded_dir() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-modadopt-{}", std::process::id()));
        let store = Store::open_at(&base)?;

        let from = base.join("steamcmd-cache/123");
        fs::create_dir_all(from.join("mods/Test Mod"))?;
        fs::write(from.join("mods/Test Mod/mod.info"), "mod")?;

        let stored = store.adopt_mod_dir(&from, 123, 1_700_000_000)?;
        assert_eq!(stored.dir, store.mod_dir(123));
        assert!(stored.dir.join("mods/Test Mod/mod.info").is_file());
        assert!(!from.exists(), "source must be consumed");

        // Re-adopting replaces the stored copy.
        fs::create_dir_all(&from)?;
        fs::write(from.join("marker"), "new")?;
        store.adopt_mod_dir(&from, 123, 1_700_000_100)?;
        assert_eq!(fs::read_to_string(store.mod_dir(123).join("marker"))?, "new");
        assert_eq!(store.stored_mod(123)?.unwrap().time_updated, 1_700_000_100);

        fs::remove_dir_all(&base)?;
        Ok(())
    }
}
