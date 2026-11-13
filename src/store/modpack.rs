//! Modpacks: a zip archive of an instance's installed mods plus a
//! manifest, shareable and installable into another instance.
//!
//! The archive holds `manifest.json` at the root and the mod folders
//! under `mods/`, exactly as the source instance's profile has them —
//! already filtered to what the game loads on the source's version.
//! The manifest carries the mods' captured `mod.info` metadata, so an
//! install can run the same compatibility check, dependency-aware
//! activation, and ZombieBuddy agent setup as a store install.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;

use crate::fmt;
use crate::fsops::{measure_tree, unix_now, Progress, ProgressFactory};
use crate::pz::compat::{self, Compatibility};
use crate::pz::item::{self, ModEntry};
use crate::pz::zombie_buddy;
use crate::store::install::{
    activate_install, read_installed_manifest, InstallOutcome, InstalledMod, ModRecord,
};
use crate::store::{GameInstance, Store};
use crate::{debug, info, warn};

/// The manifest file name inside a modpack.
pub const MANIFEST: &str = "manifest.json";

/// Zip prefix holding the mod folders, mirroring the profile layout.
const MODS_PREFIX: &str = "mods/";

/// Format version of the manifest this build writes and accepts.
const FORMAT: u32 = 1;

/// The manifest at the root of a modpack.
#[derive(Debug, Serialize, Deserialize)]
pub struct ModpackManifest {
    pub format: u32,
    /// The packing instance's name.
    pub name: String,
    /// The game version the mods were installed on.
    pub game_version: String,
    /// Unix seconds at pack time.
    pub created: u64,
    /// The packed mods, with the metadata captured from their mod.info
    /// manifests at pack time.
    pub mods: Vec<ModEntry>,
    /// Install provenance per packed mod, when the source instance
    /// tracked it; empty in packs from untracked installs. Keyed by
    /// mod folder name, so an install of this pack can record where
    /// each mod came from.
    #[serde(default)]
    pub records: Vec<ModRecord>,
}

/// The result of packing: where the modpack landed and what it holds.
#[derive(Debug)]
pub struct PackedModpack {
    pub path: PathBuf,
    /// Packed mod folder names.
    pub mods: Vec<String>,
    /// Uncompressed size of the packed content.
    pub bytes: u64,
}

impl Store {
    /// Pack the instance's installed mods into a `.modpack` zip at
    /// `output`: the manifest plus the mod folders exactly as
    /// installed. Replaces an existing modpack file.
    pub fn instance_pack_mods(
        &self,
        instance: &GameInstance,
        output: &Path,
        progress: ProgressFactory,
    ) -> Result<PackedModpack> {
        let mods_dir = instance.profile_dir.join("mods");
        let entries = item::inspect_item(&instance.profile_dir);
        if entries.is_empty() {
            anyhow::bail!("no mods installed in {}", mods_dir.display());
        }

        // Folders the game would not recognize (no manifest anywhere)
        // stay out: they cannot load, so packing them would only
        // promise content an install would skip.
        for entry in fs::read_dir(&mods_dir)
            .with_context(|| format!("reading {}", mods_dir.display()))?
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir()
                && !entries.iter().any(|m| m.name == name)
            {
                warn!("skipping {name}, which ships no manifest the game would read");
            }
        }

        let mut total_bytes = 0u64;
        for entry in &entries {
            let (_, bytes) = measure_tree(&mods_dir.join(&entry.name), &[])?;
            total_bytes += bytes;
        }

        let staging = output.with_extension("modpack-partial");
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        if output.is_dir() {
            anyhow::bail!("{} is a directory", output.display());
        }
        if staging.exists() {
            fs::remove_file(&staging)
                .with_context(|| format!("clearing {}", staging.display()))?;
        }

        let started = Instant::now();
        // Carry the install provenance along, so installing this pack
        // elsewhere records where each mod came from — a later store
        // install there can then offer updates.
        let installed = read_installed_manifest(&mods_dir);
        let records: Vec<ModRecord> = entries
            .iter()
            .filter_map(|entry| installed.record(&entry.name).cloned())
            .collect();
        let manifest = ModpackManifest {
            format: FORMAT,
            name: instance.name.clone(),
            game_version: instance.version.clone(),
            created: unix_now(),
            mods: entries.clone(),
            records,
        };
        let mut zip = zip::ZipWriter::new(fs::File::create(&staging).with_context(|| {
            format!("creating {}", staging.display())
        })?);
        zip.start_file(MANIFEST, SimpleFileOptions::default())
            .with_context(|| format!("writing {MANIFEST}"))?;
        zip.write_all(&serde_json::to_vec_pretty(&manifest)?)?;

        let mut progress = progress(total_bytes);
        let mut packed_bytes = 0u64;
        for entry in &entries {
            add_mod_dir(&mut zip, &mods_dir, &entry.name, &mut packed_bytes, progress.as_mut())?;
        }
        progress.finish();
        let file = zip
            .finish()
            .with_context(|| format!("finishing {}", staging.display()))?;
        debug!(
            "modpack: {} file(s), {}",
            entries.len(),
            fmt::format_bytes(packed_bytes)
        );
        drop(file);

        // Replace an existing modpack only once the new one is fully
        // written; the rename is same-filesystem, so it cannot fail
        // half-way.
        if output.exists() {
            warn!("replacing existing {}", output.display());
            fs::remove_file(output)
                .with_context(|| format!("removing {}", output.display()))?;
        }
        fs::rename(&staging, output)
            .with_context(|| format!("activating {}", output.display()))?;
        info!(
            "packed {} mod(s), {}, in {:.1}s",
            entries.len(),
            fmt::format_bytes(packed_bytes),
            started.elapsed().as_secs_f32()
        );
        Ok(PackedModpack {
            path: output.to_path_buf(),
            mods: entries.iter().map(|m| m.name.clone()).collect(),
            bytes: packed_bytes,
        })
    }

    /// Install a `.modpack` file into the instance's profile mods
    /// folder: every mod the manifest lists and the instance's game
    /// version accepts, activated in dependency-aware load order — the
    /// same end state as a store install, ZombieBuddy's agent setup
    /// included. Re-installing a mod replaces the previous copy.
    pub fn instance_install_modpack(
        &self,
        instance: &GameInstance,
        path: &Path,
        progress: ProgressFactory,
    ) -> Result<InstallOutcome> {
        let file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let mut archive =
            zip::ZipArchive::new(file).with_context(|| format!("reading {}", path.display()))?;
        let manifest: ModpackManifest = {
            let mut entry = archive
                .by_name(MANIFEST)
                .with_context(|| format!("reading {MANIFEST} from {}", path.display()))?;
            serde_json::from_reader(&mut entry)
                .with_context(|| format!("parsing {MANIFEST} from {}", path.display()))?
        };
        if manifest.format != FORMAT {
            anyhow::bail!(
                "modpack format {} is not supported; this build reads format {FORMAT}",
                manifest.format
            );
        }
        if manifest.mods.is_empty() {
            anyhow::bail!("the modpack holds no mods");
        }

        let mods_dir = instance.profile_dir.join("mods");
        fs::create_dir_all(&mods_dir)
            .with_context(|| format!("creating {}", mods_dir.display()))?;
        let staging_root = instance.dir.join(".install-staging");
        if staging_root.exists() {
            fs::remove_dir_all(&staging_root)
                .with_context(|| format!("clearing {}", staging_root.display()))?;
        }
        fs::create_dir_all(&staging_root)
            .with_context(|| format!("creating {}", staging_root.display()))?;

        // The uncompressed size of the mods being installed, for the
        // progress bar.
        let mut total_bytes = 0u64;
        for entry in &manifest.mods {
            total_bytes += entry_bytes(&mut archive, &entry.name)?;
        }

        let started = Instant::now();
        let mut progress = progress(total_bytes);
        let mut installed = Vec::new();
        let mut activated: Vec<(String, &ModEntry)> = Vec::new();
        let mut no_id: Vec<String> = Vec::new();
        let mut zombie_buddy_dir: Option<PathBuf> = None;
        for entry in &manifest.mods {
            let provenance = manifest.records.iter().find(|r| r.name == entry.name);
            let skipped = match compat::mod_compatibility(entry, &instance.version) {
                Compatibility::Compatible => None,
                Compatibility::Incompatible(reason) => Some(reason),
                Compatibility::Unknown => {
                    warn!(
                        "mod {} has no compatibility signal; skipping",
                        entry.name
                    );
                    continue;
                }
            };
            let outcome = match skipped {
                Some(reason) => InstalledMod {
                    item_id: provenance.map_or(0, |r| r.item_id),
                    name: entry.name.clone(),
                    mod_id: entry.mod_id.clone(),
                    title: entry.title.clone(),
                    time_updated: provenance.map_or(0, |r| r.time_updated),
                    dir: None,
                    skipped: Some(reason),
                    unsafe_install: false,
                },
                None => {
                    let dir = extract_one(
                        &mut archive,
                        entry,
                        &staging_root,
                        &mods_dir,
                        progress.as_mut(),
                    )?;
                    match &entry.mod_id {
                        Some(id) => activated.push((id.clone(), entry)),
                        None => no_id.push(format!(
                            "mod {} has no Mod ID and cannot be activated",
                            entry.name
                        )),
                    }
                    if zombie_buddy::is_mod_name(&entry.name)
                        || entry
                            .mod_id
                            .as_deref()
                            .is_some_and(zombie_buddy::is_mod_name)
                    {
                        zombie_buddy_dir = Some(dir.clone());
                    }
                    InstalledMod {
                        item_id: provenance.map_or(0, |r| r.item_id),
                        name: entry.name.clone(),
                        mod_id: entry.mod_id.clone(),
                        title: entry.title.clone(),
                        time_updated: provenance.map_or(0, |r| r.time_updated),
                        dir: Some(dir),
                        skipped: None,
                        unsafe_install: false,
                    }
                }
            };
            installed.push(outcome);
        }

        let (activation, agent_files) = activate_install(
            instance,
            &installed,
            &activated,
            no_id,
            zombie_buddy_dir.as_deref(),
        )?;

        fs::remove_dir_all(&staging_root)
            .with_context(|| format!("clearing {}", staging_root.display()))?;
        info!(
            "installed modpack {} in {:.1}s",
            path.display(),
            started.elapsed().as_secs_f32()
        );
        Ok(InstallOutcome {
            mods: installed,
            activation,
            agent_files,
        })
    }
}

/// The uncompressed size of one mod's files in the archive.
fn entry_bytes(archive: &mut zip::ZipArchive<fs::File>, name: &str) -> Result<u64> {
    let prefix = format!("{MODS_PREFIX}{name}/");
    let mut total = 0u64;
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index)?;
        let name = entry.name().context("reading an archive entry name")?;
        if name.starts_with(&prefix) && !entry.is_dir() {
            total += entry.size();
        }
    }
    Ok(total)
}

/// Extract one mod's files from the archive into the staging dir, then
/// move the staged copy into the profile's mods folder. Returns the
/// installed folder.
fn extract_one(
    archive: &mut zip::ZipArchive<fs::File>,
    mod_entry: &ModEntry,
    staging_root: &Path,
    mods_dir: &Path,
    progress: &mut dyn Progress,
) -> Result<PathBuf> {
    let prefix = format!("{MODS_PREFIX}{}/", mod_entry.name);
    let staging = staging_root.join(&mod_entry.name);
    if staging.exists() {
        fs::remove_dir_all(&staging)
            .with_context(|| format!("clearing {}", staging.display()))?;
    }
    fs::create_dir_all(&staging)
        .with_context(|| format!("creating staging dir {}", staging.display()))?;

    let mut files = 0usize;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry
            .name()
            .context("reading an archive entry name")?
            .into_owned();
        if !name.starts_with(&prefix) {
            continue;
        }
        // Refuse paths that escape the staging dir (zip-slip).
        let relative = Path::new(&name)
            .strip_prefix(&prefix)
            .with_context(|| format!("the archive contains an unsafe path: {name}"))?;
        let target = staging.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&target)
                .with_context(|| format!("creating {}", target.display()))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let mut out = fs::File::create(&target)
            .with_context(|| format!("creating {}", target.display()))?;
        let bytes = io::copy(&mut entry, &mut out)
            .with_context(|| format!("extracting {name}"))?;
        progress.step(bytes);
        files += 1;
    }
    if files == 0 {
        anyhow::bail!("the modpack holds no files for mod {}", mod_entry.name);
    }

    // The old copy comes out only once the new one is fully staged;
    // the rename is same-filesystem, so it cannot fail half-way.
    let target = mods_dir.join(&mod_entry.name);
    if target.exists() {
        warn!("replacing existing mod folder {}", target.display());
        fs::remove_dir_all(&target)
            .with_context(|| format!("removing {}", target.display()))?;
    }
    fs::rename(&staging, &target)
        .with_context(|| format!("activating {}", target.display()))?;
    Ok(target)
}

/// Add one mod folder to the archive under `mods/<name>/`.
fn add_mod_dir(
    zip: &mut zip::ZipWriter<fs::File>,
    mods_dir: &Path,
    name: &str,
    packed: &mut u64,
    progress: &mut dyn Progress,
) -> Result<()> {
    add_dir_rec(
        zip,
        &mods_dir.join(name),
        &format!("{MODS_PREFIX}{name}"),
        packed,
        progress,
    )
}

fn add_dir_rec(
    zip: &mut zip::ZipWriter<fs::File>,
    dir: &Path,
    zip_path: &str,
    packed: &mut u64,
    progress: &mut dyn Progress,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        let path = entry.path();
        let child = format!(
            "{zip_path}/{}",
            entry.file_name().to_string_lossy()
        );
        if path.is_dir() {
            add_dir_rec(zip, &path, &child, packed, progress)?;
        } else {
            let mut source =
                fs::File::open(&path).with_context(|| format!("reading {}", path.display()))?;
            zip.start_file(&child, SimpleFileOptions::default())
                .with_context(|| format!("writing {child}"))?;
            let bytes = io::copy(&mut source, zip)
                .with_context(|| format!("writing {child}"))?;
            *packed += bytes;
            progress.step(bytes);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsops::noop_progress;
    use crate::pz::item::inspect_item;
    use crate::store::meta::Source;
    use crate::store::mods::StoredMod;

    /// Build a store with two instances of one game version, and a
    /// workshop item bundling a plain B42 mod plus ZombieBuddy (with
    /// its loader and agent jar in libs/).
    fn fixture(name: &str) -> Result<(Store, GameInstance, GameInstance, StoredMod, PathBuf)> {
        let base =
            std::env::temp_dir().join(format!("snapoid-modpack-{name}-{}", std::process::id()));
        let store = Store::open_at(&base)?;

        let game = store.game_dir("42.22.2");
        fs::create_dir_all(&game)?;
        fs::write(game.join("projectzomboid.jar"), "jar")?;
        let source = store.instance_new("source", "42.22.2")?;
        let target = store.instance_new("target", "42.22.2")?;

        let item_dir = base.join("item-123");
        let mods = item_dir.join("mods");
        fs::create_dir_all(mods.join("Plain Mod/42/media"))?;
        fs::write(mods.join("Plain Mod/42/mod.info"), "name=Plain\nid=PM\n")?;
        fs::write(mods.join("Plain Mod/42/media/plain.lua"), "lua")?;
        fs::create_dir_all(mods.join("ZombieBuddy/42/media"))?;
        fs::create_dir_all(mods.join("ZombieBuddy/libs"))?;
        fs::write(
            mods.join("ZombieBuddy/42/mod.info"),
            "name=ZombieBuddy\nid=ZombieBuddy\n",
        )?;
        fs::write(mods.join("ZombieBuddy/42/media/buddy.lua"), "lua")?;
        fs::write(mods.join("ZombieBuddy/libs/zbNative.dll"), "loader")?;
        fs::write(mods.join("ZombieBuddy/libs/ZombieBuddy.jar"), "agent jar")?;
        fs::write(
            mods.join("ZombieBuddy/libs/ZombieBuddy.jar.zbs"),
            "signature",
        )?;

        let stored = StoredMod {
            id: 123,
            time_updated: 1,
            dir: item_dir.clone(),
            created: 1,
            title: None,
            source: Some(Source::WorkshopDisk),
            mods: inspect_item(&item_dir),
            layout: None,
        };
        Ok((store, source, target, stored, base))
    }

    #[test]
    fn packs_the_installed_mods_with_a_manifest() -> Result<()> {
        let (store, instance, _other, item, base) = fixture("pack")?;
        store.instance_install_mods(&instance, &[&item], noop_progress)?;

        let output = base.join("share/test.modpack");
        let packed = store.instance_pack_mods(&instance, &output, noop_progress)?;
        assert_eq!(packed.mods, vec!["Plain Mod", "ZombieBuddy"]);
        assert!(packed.bytes > 0);
        assert!(output.is_file());

        let file = fs::File::open(&output)?;
        let mut archive = zip::ZipArchive::new(file)?;
        let manifest: ModpackManifest = {
            let mut entry = archive.by_name(MANIFEST)?;
            serde_json::from_reader(&mut entry)?
        };
        assert_eq!(manifest.format, FORMAT);
        assert_eq!(manifest.name, "source");
        assert_eq!(manifest.game_version, "42.22.2");
        assert_eq!(manifest.mods.len(), 2);
        assert!(manifest.mods.iter().any(|m| m.name == "ZombieBuddy"));

        // The mod folders are in the archive, contents included.
        let mut names: Vec<String> = (0..archive.len())
            .filter_map(|i| {
                archive
                    .by_index(i)
                    .ok()?
                    .name()
                    .ok()?
                    .into_owned()
                    .into()
            })
            .collect();
        names.sort();
        assert!(names.contains(&"mods/Plain Mod/42/media/plain.lua".to_owned()));
        assert!(names.contains(&"mods/ZombieBuddy/libs/zbNative.dll".to_owned()));

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn packing_with_no_mods_installed_fails() -> Result<()> {
        let (store, instance, _other, _item, base) = fixture("empty")?;
        let output = base.join("empty.modpack");
        assert!(store.instance_pack_mods(&instance, &output, noop_progress).is_err());
        assert!(!output.exists());
        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn installs_a_modpack_into_another_instance() -> Result<()> {
        let (store, source, target, item, base) = fixture("install")?;
        store.instance_install_mods(&source, &[&item], noop_progress)?;
        let modpack = base.join("share/test.modpack");
        store.instance_pack_mods(&source, &modpack, noop_progress)?;

        let outcome = store.instance_install_modpack(&target, &modpack, noop_progress)?;

        // The mod files land in the target's profile, ZombieBuddy
        // whole (its libs/ included).
        assert!(target
            .profile_dir
            .join("mods/Plain Mod/42/media/plain.lua")
            .is_file());
        assert!(target
            .profile_dir
            .join("mods/ZombieBuddy/libs/zbNative.dll")
            .is_file());

        // The agent install ran: loader and jar in the target's own
        // agent dir, the jar also in the game folder for the Windows
        // loader flow.
        assert_eq!(
            outcome.agent_files,
            vec!["zbNative.dll", "ZombieBuddy.jar", "ZombieBuddy.jar.zbs"]
        );
        assert_eq!(
            fs::read(target.dir.join("agent/zbNative.dll"))?,
            b"loader"
        );
        assert_eq!(
            fs::read(target.game_dir.join("ZombieBuddy.jar"))?,
            b"agent jar"
        );

        // Activation: ZombieBuddy first, then the plain mod, in both
        // the client list and the server config.
        assert_eq!(outcome.activation.mod_ids, vec!["ZombieBuddy", "PM"]);
        let default_txt =
            fs::read_to_string(target.profile_dir.join("mods/default.txt"))?;
        assert!(default_txt.contains("mod = ZombieBuddy,\n    mod = PM,"));
        let ini = fs::read_to_string(
            target
                .profile_dir
                .join("Server")
                .join("snapoid_server_target.ini"),
        )?;
        assert_eq!(ini, "PublicName=target\nMods=ZombieBuddy;PM\n");

        // No staging left behind.
        assert!(!target.dir.join(".install-staging").exists());

        // The pack carried the source's install provenance, so the
        // target's manifest records where each mod came from — a
        // later store install there can offer updates.
        let manifest = store.instance_installed_manifest(&target);
        let plain = manifest.record("Plain Mod").unwrap();
        assert_eq!(plain.item_id, 123);
        assert_eq!(plain.time_updated, 1);
        assert_eq!(manifest.record("ZombieBuddy").unwrap().item_id, 123);

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn reinstalling_from_a_modpack_replaces_the_previous_copy() -> Result<()> {
        let (store, source, target, item, base) = fixture("reinstall")?;
        store.instance_install_mods(&source, &[&item], noop_progress)?;
        let modpack = base.join("share/test.modpack");
        store.instance_pack_mods(&source, &modpack, noop_progress)?;
        store.instance_install_modpack(&target, &modpack, noop_progress)?;

        // The mod updates in the source; re-packing and installing
        // must replace the target's copy.
        fs::write(
            source
                .profile_dir
                .join("mods/Plain Mod/42/media/plain.lua"),
            "lua v2",
        )?;
        store.instance_pack_mods(&source, &modpack, noop_progress)?;
        store.instance_install_modpack(&target, &modpack, noop_progress)?;
        assert_eq!(
            fs::read_to_string(
                target
                    .profile_dir
                    .join("mods/Plain Mod/42/media/plain.lua")
            )?,
            "lua v2"
        );

        fs::remove_dir_all(&base)?;
        Ok(())
    }
}
