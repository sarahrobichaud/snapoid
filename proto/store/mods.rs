//! Stored workshop mods: capture from the local workshop dir, adopt
//! steamcmd downloads, and read back.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context as _, Result};

use crate::fmt;
use crate::fsops::{ProgressFactory, copy_tree, measure_tree, move_dir, swap_into_place, unix_now};
use crate::pz::item::{self, Layout, ModEntry};
use crate::store::Store;
use crate::store::meta::{self, ModMeta, Source};
use crate::{debug, info};

/// A stored copy of one workshop item, with all the mods it bundles.
#[derive(Debug)]
pub struct StoredMod {
    pub id: u64,
    /// Steam's timeupdated for the item at capture time; 0 when unknown.
    pub time_updated: u64,
    pub dir: PathBuf,
    pub created: u64,
    /// Workshop item title from the Steam Web API, when known.
    pub title: Option<String>,
    /// How the copy was obtained.
    pub source: Option<Source>,
    /// Mods bundled in this item, captured at save time.
    pub mods: Vec<ModEntry>,
    /// Layout of the bundled mods, when they share one.
    pub layout: Option<Layout>,
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
        source: Source,
        title: Option<&str>,
        progress: ProgressFactory,
    ) -> Result<StoredMod> {
        let created = unix_now();
        let dir = self.mod_dir(id);
        let staging = self.mods_dir().join(format!(".staging-{id}"));

        // Capture the bundled mods' manifests before copying; the copy
        // is identical, so either side would do.
        let mods = item::inspect_item(from);

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

        let layout = shared_layout(&mods);
        meta::write(
            &staging.join("mod.json"),
            &ModMeta {
                id,
                time_updated,
                created,
                title: title.map(str::to_owned),
                source: Some(source),
                mods: mods.clone(),
                layout,
            },
        )?;

        swap_into_place(&staging, &dir)?;

        Ok(StoredMod {
            id,
            time_updated,
            dir,
            created,
            title: title.map(str::to_owned),
            source: Some(source),
            mods,
            layout,
        })
    }

    /// Move an externally populated workshop item dir (a steamcmd
    /// download) into `mods/<id>/` and write its meta file. `from` is
    /// consumed: renamed when the filesystems match, copied and removed
    /// otherwise. Re-adopting an existing mod replaces it, with the old
    /// copy kept as a dot-prefixed sibling until the new one is in place.
    pub fn adopt_mod_dir(
        &self,
        from: &Path,
        id: u64,
        time_updated: u64,
        source: Source,
        title: Option<&str>,
    ) -> Result<StoredMod> {
        let created = unix_now();
        let dir = self.mod_dir(id);

        // Inspect before the move consumes `from`.
        let mods = item::inspect_item(from);

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

        let layout = shared_layout(&mods);
        meta::write(
            &dir.join("mod.json"),
            &ModMeta {
                id,
                time_updated,
                created,
                title: title.map(str::to_owned),
                source: Some(source),
                mods: mods.clone(),
                layout,
            },
        )?;

        Ok(StoredMod {
            id,
            time_updated,
            dir,
            created,
            title: title.map(str::to_owned),
            source: Some(source),
            mods,
            layout,
        })
    }

    /// All stored workshop items, sorted by id. An entry with a missing
    /// or unreadable meta is still listed (times 0), so a damaged
    /// mod.json does not hide the mod.
    pub fn list_mods(&self) -> Result<Vec<StoredMod>> {
        let dir = self.mods_dir();
        if !dir.is_dir() {
            return Ok(Vec::new());
        }

        let mut mods = Vec::new();
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
            let Ok(id) = entry.file_name().to_string_lossy().parse::<u64>() else {
                continue;
            };
            let mod_meta = meta::read::<ModMeta>(&path.join("mod.json")).ok().flatten();
            mods.push(StoredMod {
                id,
                time_updated: mod_meta.as_ref().map(|m| m.time_updated).unwrap_or(0),
                dir: path,
                created: mod_meta.as_ref().map(|m| m.created).unwrap_or(0),
                title: mod_meta.as_ref().and_then(|m| m.title.clone()),
                source: mod_meta.as_ref().and_then(|m| m.source),
                mods: mod_meta
                    .as_ref()
                    .map(|m| m.mods.clone())
                    .unwrap_or_default(),
                layout: mod_meta.as_ref().and_then(|m| m.layout),
            });
        }

        mods.sort_by_key(|m| m.id);
        Ok(mods)
    }

    /// The stored copy of one workshop item, if any. A missing or
    /// unreadable meta counts as not stored, so a re-save can repair it.
    pub fn stored_mod(&self, id: u64) -> Result<Option<StoredMod>> {
        let dir = self.mod_dir(id);
        let mod_meta = meta::read::<ModMeta>(&dir.join("mod.json")).ok().flatten();
        Ok(mod_meta.map(|m| StoredMod {
            id: m.id,
            time_updated: m.time_updated,
            dir,
            created: m.created,
            title: m.title,
            source: m.source,
            mods: m.mods,
            layout: m.layout,
        }))
    }

    /// Rebuild the metadata sidecar of every stored workshop item from
    /// the files already in the store — no downloads. Capture-time
    /// fields (id, time_updated, created, title, source) are kept as
    /// recorded; everything derivable from the stored files (bundled
    /// mods, layout, inferred ranges) is re-derived. Idempotent.
    /// Returns the repaired items.
    pub fn repair_mod_meta(&self) -> Result<Vec<StoredMod>> {
        let mut repaired = Vec::new();
        for stored in self.list_mods()? {
            let mods = item::inspect_item(&stored.dir);
            let layout = shared_layout(&mods);
            meta::write(
                &stored.dir.join("mod.json"),
                &ModMeta {
                    id: stored.id,
                    time_updated: stored.time_updated,
                    created: stored.created,
                    title: stored.title.clone(),
                    source: stored.source,
                    mods: mods.clone(),
                    layout,
                },
            )?;
            repaired.push(StoredMod {
                mods,
                layout,
                ..stored
            });
        }
        Ok(repaired)
    }
}

/// The one layout all bundled mods share, or None when they differ (or
/// there are no mods).
fn shared_layout(mods: &[ModEntry]) -> Option<Layout> {
    let first = mods.first()?.layout;
    mods.iter().all(|m| m.layout == first).then_some(first)
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
        fs::write(
            from.join("mods/Test Mod/mod.info"),
            "name=Test Mod\nid=TM\nversionMin=42.16\nrequire=\\Lib, Other\n",
        )?;

        let stored = store.save_mod_copy(
            &from,
            123,
            1_700_000_000,
            Source::WorkshopDisk,
            Some("A Mod Pack"),
            noop_progress,
        )?;
        assert_eq!(stored.id, 123);
        assert_eq!(stored.time_updated, 1_700_000_000);
        assert_eq!(stored.dir, store.mod_dir(123));
        assert_eq!(stored.title.as_deref(), Some("A Mod Pack"));
        assert_eq!(stored.source, Some(Source::WorkshopDisk));
        assert_eq!(stored.mods.len(), 1);
        assert_eq!(stored.mods[0].mod_id.as_deref(), Some("TM"));
        // The manifest sits at the mod root (B41 shape), so its range
        // is the root manifest's.
        assert_eq!(
            stored.mods[0].root.as_ref().unwrap().min.as_deref(),
            Some("42.16")
        );
        assert_eq!(stored.mods[0].requires, vec!["Lib", "Other"]);
        assert_eq!(stored.layout, Some(Layout::B41));
        assert!(stored.dir.join("mods/Test Mod/mod.info").is_file());
        assert!(stored.dir.join("mod.json").is_file());
        assert!(from.exists(), "disk source must be kept");

        // Re-saving replaces the stored copy.
        fs::write(from.join("mods/Test Mod/mod.info"), "name=Test Mod v2\n")?;
        store.save_mod_copy(
            &from,
            123,
            1_700_000_100,
            Source::WorkshopDisk,
            None,
            noop_progress,
        )?;
        assert_eq!(
            fs::read_to_string(store.mod_dir(123).join("mods/Test Mod/mod.info"))?,
            "name=Test Mod v2\n"
        );
        let stored = store.stored_mod(123)?.unwrap();
        assert_eq!(stored.time_updated, 1_700_000_100);
        assert_eq!(stored.title, None);
        assert!(store.stored_mod(456)?.is_none());

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn lists_stored_mods() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-modlist-{}", std::process::id()));
        let store = Store::open_at(&base)?;
        assert!(store.list_mods()?.is_empty());

        let from = base.join("workshop/456");
        fs::create_dir_all(from.join("mods/Test Mod"))?;
        fs::write(from.join("mods/Test Mod/mod.info"), "mod")?;
        store.save_mod_copy(
            &from,
            456,
            1_700_000_000,
            Source::WorkshopDisk,
            None,
            noop_progress,
        )?;
        fs::create_dir_all(base.join("cache/123"))?;
        store.adopt_mod_dir(
            &base.join("cache/123"),
            123,
            1_700_000_100,
            Source::Steamcmd,
            None,
        )?;

        // Staging and retired dirs are dot-prefixed and never listed.
        fs::create_dir_all(store.mods_dir().join(".staging-789"))?;

        let mods = store.list_mods()?;
        let ids: Vec<u64> = mods.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![123, 456]);
        assert_eq!(mods[0].time_updated, 1_700_000_100);
        assert_eq!(mods[0].source, Some(Source::Steamcmd));
        assert_eq!(mods[1].time_updated, 1_700_000_000);

        // A damaged meta still lists the mod, with unknown times.
        fs::write(store.mod_dir(123).join("mod.json"), "not json")?;
        let mods = store.list_mods()?;
        assert_eq!(mods[0].id, 123);
        assert_eq!(mods[0].time_updated, 0);
        assert!(mods[0].mods.is_empty());

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn adopts_downloaded_dir() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-modadopt-{}", std::process::id()));
        let store = Store::open_at(&base)?;

        let from = base.join("steamcmd-cache/123");
        fs::create_dir_all(from.join("mods/Test Mod/42.16"))?;
        fs::write(
            from.join("mods/Test Mod/42.16/mod.info"),
            "name=Test Mod\nid=TM\n",
        )?;

        let stored = store.adopt_mod_dir(
            &from,
            123,
            1_700_000_000,
            Source::Steamcmd,
            Some("Test Pack"),
        )?;
        assert_eq!(stored.dir, store.mod_dir(123));
        assert_eq!(stored.title.as_deref(), Some("Test Pack"));
        assert_eq!(stored.source, Some(Source::Steamcmd));
        assert_eq!(stored.mods.len(), 1);
        assert_eq!(stored.mods[0].layout, Layout::B42);
        assert!(stored.dir.join("mods/Test Mod/42.16/mod.info").is_file());
        assert!(!from.exists(), "source must be consumed");

        // Re-adopting replaces the stored copy.
        fs::create_dir_all(&from)?;
        fs::write(from.join("marker"), "new")?;
        store.adopt_mod_dir(&from, 123, 1_700_000_100, Source::Steamcmd, None)?;
        assert_eq!(
            fs::read_to_string(store.mod_dir(123).join("marker"))?,
            "new"
        );
        assert_eq!(store.stored_mod(123)?.unwrap().time_updated, 1_700_000_100);

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    /// A sidecar written before the enriched metadata still lists.
    #[test]
    fn lists_old_sidecar_without_new_fields() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-modold-{}", std::process::id()));
        let store = Store::open_at(&base)?;
        fs::create_dir_all(store.mod_dir(77))?;
        fs::write(
            store.mod_dir(77).join("mod.json"),
            r#"{ "id": 77, "time_updated": 5, "created": 6 }"#,
        )?;

        let stored = store.stored_mod(77)?.unwrap();
        assert_eq!(stored.time_updated, 5);
        assert_eq!(stored.created, 6);
        assert_eq!(stored.title, None);
        assert_eq!(stored.source, None);
        assert!(stored.mods.is_empty());

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    /// Repair rebuilds a pre-enrichment sidecar from the stored files,
    /// keeping the capture-time fields.
    #[test]
    fn repairs_meta_from_stored_files() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-modrepair-{}", std::process::id()));
        let store = Store::open_at(&base)?;

        let dir = store.mod_dir(88);
        fs::create_dir_all(dir.join("mods/Test Mod/42.16"))?;
        fs::write(
            dir.join("mods/Test Mod/42.16/mod.info"),
            "name=Test Mod\nid=TM\n",
        )?;
        fs::write(
            dir.join("mod.json"),
            r#"{ "id": 88, "time_updated": 7, "created": 8 }"#,
        )?;

        let repaired = store.repair_mod_meta()?;
        assert_eq!(repaired.len(), 1);
        assert_eq!(repaired[0].id, 88);
        assert_eq!(repaired[0].time_updated, 7);
        assert_eq!(repaired[0].created, 8);
        assert_eq!(repaired[0].title, None);
        assert_eq!(repaired[0].source, None);
        assert_eq!(repaired[0].mods.len(), 1);
        assert_eq!(repaired[0].mods[0].mod_id.as_deref(), Some("TM"));
        assert_eq!(repaired[0].mods[0].folders[0].name, "42.16");
        assert_eq!(repaired[0].mods[0].folders[0].version, 42_016);
        assert_eq!(repaired[0].layout, Some(Layout::B42));

        // The sidecar on disk now carries the derived data too.
        assert_eq!(store.stored_mod(88)?.unwrap().mods.len(), 1);

        // Idempotent.
        assert_eq!(store.repair_mod_meta()?.len(), 1);

        fs::remove_dir_all(&base)?;
        Ok(())
    }
}
