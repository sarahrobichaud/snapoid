//! Installing stored workshop mods into an instance's profile: copying
//! only what the game would load on the instance's game version.
//!
//! On a B42 game the loader selects the highest version folder ≤ the
//! running version and reads it plus `common/` — so that is all that
//! gets copied: never the B41 root layout, never the version folders
//! the game would not select. On a B41 game the root layout is what
//! loads, so the copy is the mod dir minus the version folders and
//! `common/` (which B41 ignores entirely).
//!
//! Staging happens in the instance dir, never inside `profile/mods/`:
//! the game scans that folder and would recognize a staging dir
//! containing `common/mod.info` as a mod.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context as _, Result};

use crate::fmt;
use crate::fsops::{copy_tree, measure_tree, Progress, ProgressFactory};
use crate::launch;
use crate::pz::compat::{self, Compatibility};
use crate::pz::item::ModEntry;
use crate::pz::version;
use crate::store::{GameInstance, StoredMod, Store};
use crate::{debug, info, warn};

/// One bundled mod: installed into the profile, or skipped with the
/// game's reason.
#[derive(Debug)]
pub struct InstalledMod {
    /// Workshop item the mod came from.
    pub item_id: u64,
    /// Mod folder name under the item's mods/.
    pub name: String,
    /// The Mod ID, when the manifest declares one.
    pub mod_id: Option<String>,
    /// The installed folder in the profile, when installed.
    pub dir: Option<PathBuf>,
    /// Why the mod was skipped, when it was.
    pub skipped: Option<compat::Incompatibility>,
}

/// The activation artifacts written for an install: the client's
/// mods/default.txt and the server config's Mods= line, in dependency-
/// aware load order.
#[derive(Debug)]
pub struct Activation {
    /// Mod IDs in load order.
    pub mod_ids: Vec<String>,
    /// Warnings: unresolved dependencies, load-order cycles, mods
    /// without a Mod ID.
    pub warnings: Vec<String>,
    /// The client activation list that was written.
    pub default_txt: PathBuf,
    /// The server config whose Mods= line was written.
    pub server_ini: PathBuf,
}

/// The result of an install: what was copied or skipped, and the
/// activation artifacts written.
#[derive(Debug)]
pub struct InstallOutcome {
    pub mods: Vec<InstalledMod>,
    pub activation: Activation,
}

/// One copy operation reproducing part of what the game loads.
enum CopyOp {
    /// A subdirectory of the mod dir, copied under the same name.
    Subtree { name: String },
    /// The whole mod dir minus the named top-level entries.
    RootMinus { exclude: Vec<String> },
}

impl CopyOp {
    fn measure(&self, source: &Path) -> Result<(u64, u64)> {
        match self {
            CopyOp::Subtree { name } => measure_tree(&source.join(name), &[]),
            CopyOp::RootMinus { exclude } => {
                let exclude: Vec<&str> = exclude.iter().map(String::as_str).collect();
                measure_tree(source, &exclude)
            }
        }
    }

    fn copy(&self, source: &Path, staging: &Path, progress: &mut dyn Progress) -> Result<()> {
        match self {
            CopyOp::Subtree { name } => {
                copy_tree(&source.join(name), &staging.join(name), &[], progress)?;
            }
            CopyOp::RootMinus { exclude } => {
                let exclude: Vec<&str> = exclude.iter().map(String::as_str).collect();
                copy_tree(source, staging, &exclude, progress)?;
            }
        }
        Ok(())
    }
}

/// The copy operations that reproduce what the game would load from
/// the mod on a game version int.
fn copy_ops(mod_entry: &ModEntry, game: u64) -> Vec<CopyOp> {
    let floor = 42 * 1000;
    if game < floor {
        // B41: the root layout loads; version folders and common/ do
        // not (B41 ignores common/ entirely).
        let mut exclude: Vec<String> = mod_entry.folders.iter().map(|f| f.name.clone()).collect();
        exclude.push("common".to_owned());
        return vec![CopyOp::RootMinus { exclude }];
    }

    let mut ops = Vec::new();
    // The game selects the highest version folder ≤ the running
    // version; the others stay in the store.
    if let Some(selected) = mod_entry
        .folders
        .iter()
        .filter(|f| f.version >= floor && f.version <= game)
        .max_by_key(|f| f.version)
    {
        ops.push(CopyOp::Subtree {
            name: selected.name.clone(),
        });
    }
    // common/ loads first on B42, overwritten by the version folder —
    // its content loads even when it ships no mod.info.
    if mod_entry.common_dir {
        ops.push(CopyOp::Subtree {
            name: "common".to_owned(),
        });
    }
    ops
}

impl Store {
    /// Install the bundled mods of `items` into the instance's profile
    /// mods folder. Only what the game would load on the instance's
    /// game version is copied; mods the game would not make available
    /// are skipped with their reason. Re-installing a mod replaces the
    /// previous copy.
    ///
    /// The installed set is then activated: the client's
    /// mods/default.txt and the server config's Mods= line are written
    /// in dependency-aware load order (`require=` first, then
    /// `loadModAfter=`/`loadModBefore=`), since installing files alone
    /// activates nothing — the server advertises only its Mods= line
    /// and the client loads only the default.txt set.
    pub fn instance_install_mods(
        &self,
        instance: &GameInstance,
        items: &[&StoredMod],
        progress: ProgressFactory,
    ) -> Result<InstallOutcome> {
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

        let game = version::game_int(&instance.version);
        let mut installed = Vec::new();
        let mut activated: Vec<(String, &ModEntry)> = Vec::new();
        let mut no_id: Vec<String> = Vec::new();
        for item in items {
            for mod_entry in &item.mods {
                let skipped = match compat::mod_compatibility(mod_entry, &instance.version) {
                    Compatibility::Compatible => None,
                    Compatibility::Incompatible(reason) => Some(reason),
                    Compatibility::Unknown => {
                        warn!(
                            "mod {} has no compatibility signal; skipping",
                            mod_entry.name
                        );
                        continue;
                    }
                };
                let outcome = match skipped {
                    Some(reason) => InstalledMod {
                        item_id: item.id,
                        name: mod_entry.name.clone(),
                        mod_id: mod_entry.mod_id.clone(),
                        dir: None,
                        skipped: Some(reason),
                    },
                    None => {
                        let dir = self.install_one(
                            &staging_root,
                            &mods_dir,
                            item,
                            mod_entry,
                            game,
                            progress,
                        )?;
                        match &mod_entry.mod_id {
                            Some(id) => activated.push((id.clone(), mod_entry)),
                            None => no_id.push(format!(
                                "mod {} has no Mod ID and cannot be activated",
                                mod_entry.name
                            )),
                        }
                        InstalledMod {
                            item_id: item.id,
                            name: mod_entry.name.clone(),
                            mod_id: mod_entry.mod_id.clone(),
                            dir: Some(dir),
                            skipped: None,
                        }
                    }
                };
                installed.push(outcome);
            }
        }

        let (mod_ids, mut warnings) = activation_order(&activated);
        warnings.extend(no_id);
        ensure_reset_sentinel(&instance.profile_dir, &instance.version)?;
        let default_txt = instance.profile_dir.join("mods").join("default.txt");
        let maps_block = existing_maps_block(&default_txt);
        fs::write(&default_txt, default_txt_text(&mod_ids, &maps_block))
            .with_context(|| format!("writing {}", default_txt.display()))?;
        let server_ini = instance
            .profile_dir
            .join("Server")
            .join(format!("{}.ini", launch::server_name(instance)));
        write_server_ini(&server_ini, &mod_ids, &instance.name)?;

        fs::remove_dir_all(&staging_root)
            .with_context(|| format!("clearing {}", staging_root.display()))?;
        Ok(InstallOutcome {
            mods: installed,
            activation: Activation {
                mod_ids,
                warnings,
                default_txt,
                server_ini,
            },
        })
    }

    fn install_one(
        &self,
        staging_root: &Path,
        mods_dir: &Path,
        item: &StoredMod,
        mod_entry: &ModEntry,
        game: u64,
        progress: ProgressFactory,
    ) -> Result<PathBuf> {
        let source = item.dir.join("mods").join(&mod_entry.name);
        let target = mods_dir.join(&mod_entry.name);
        let staging = staging_root.join(&mod_entry.name);
        if staging.exists() {
            fs::remove_dir_all(&staging)
                .with_context(|| format!("clearing {}", staging.display()))?;
        }
        fs::create_dir_all(&staging)
            .with_context(|| format!("creating staging dir {}", staging.display()))?;

        let ops = copy_ops(mod_entry, game);
        if ops.is_empty() {
            anyhow::bail!(
                "mod {} has nothing loadable on version {}",
                mod_entry.name,
                game
            );
        }

        let started = Instant::now();
        let mut total_files = 0u64;
        let mut total_bytes = 0u64;
        for op in &ops {
            let (files, bytes) = op.measure(&source)?;
            total_files += files;
            total_bytes += bytes;
        }
        debug!(
            "mod install: {total_files} file(s), {} to copy",
            fmt::format_bytes(total_bytes)
        );
        let mut progress = progress(total_bytes);
        for op in &ops {
            op.copy(&source, &staging, progress.as_mut())?;
        }
        progress.finish();

        // The old copy comes out only once the new one is fully
        // staged; the rename is same-filesystem, so it cannot fail
        // half-way.
        if target.exists() {
            warn!("replacing existing mod folder {}", target.display());
            fs::remove_dir_all(&target)
                .with_context(|| format!("removing {}", target.display()))?;
        }
        fs::rename(&staging, &target)
            .with_context(|| format!("activating {}", target.display()))?;
        info!(
            "installed mod {} ({total_files} file(s), {}) in {:.1}s",
            mod_entry.name,
            fmt::format_bytes(total_bytes),
            started.elapsed().as_secs_f32()
        );
        Ok(target)
    }
}

/// Order the installed mods for activation: `require=` dependencies
/// first, then `loadModAfter=`/`loadModBefore=` constraints, with the
/// install order breaking ties. Unresolvable references and cycles are
/// warned about, never fatal — the game drops what it cannot resolve.
fn activation_order(installed: &[(String, &ModEntry)]) -> (Vec<String>, Vec<String>) {
    let mut warnings = Vec::new();
    let position: HashMap<&String, usize> = installed
        .iter()
        .enumerate()
        .map(|(index, (id, _))| (id, index))
        .collect();

    // Edge a → b: a must load before b.
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    let mut indegree: HashMap<String, usize> = installed
        .iter()
        .map(|(id, _)| (id.clone(), 0))
        .collect();
    for (id, entry) in installed {
        for dep in entry.requires.iter().chain(entry.load_after.iter()) {
            if position.contains_key(dep) {
                add_edge(&mut edges, &mut indegree, dep, id);
            } else if dep != id {
                warnings.push(format!("{id} needs {dep}, which is not installed"));
            }
        }
        for before in &entry.load_before {
            if position.contains_key(before) {
                add_edge(&mut edges, &mut indegree, id, before);
            } else {
                warnings.push(format!(
                    "{id} loads before {before}, which is not installed"
                ));
            }
        }
    }

    // Kahn's algorithm, lowest install position first among the ready.
    let mut order = Vec::new();
    let mut remaining: Vec<&(String, &ModEntry)> = installed.iter().collect();
    while let Some(index) = remaining
        .iter()
        .enumerate()
        .filter(|(_, (id, _))| indegree[id] == 0)
        .map(|(index, _)| index)
        .min()
    {
        let (id, _) = remaining.remove(index);
        order.push(id.clone());
        if let Some(afters) = edges.get(id) {
            for after in afters {
                *indegree.get_mut(after).expect("edge targets are installed") -= 1;
            }
        }
    }
    if !remaining.is_empty() {
        let cycle = remaining
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push(format!("load-order cycle among: {cycle}"));
        for (id, _) in remaining {
            order.push(id.clone());
        }
    }
    (order, warnings)
}

fn add_edge(
    edges: &mut HashMap<String, Vec<String>>,
    indegree: &mut HashMap<String, usize>,
    before: &str,
    after: &str,
) {
    let list = edges.entry(before.to_owned()).or_default();
    if !list.iter().any(|x| x == after) {
        list.push(after.to_owned());
        *indegree.get_mut(after).expect("edge targets are installed") += 1;
    }
}

/// The game clears default.txt on the first launch of a new game
/// release unless the reset sentinel exists
/// (`ZomboidFileSystem.resetDefaultModsForNewRelease`, verified in
/// bytecode: no sentinel → create it and write default.txt empty).
/// An install before the game's first launch must pre-create it, or
/// the activation written here is wiped on launch. The sentinel name
/// embeds the game version's major (`reset-mods-42_00.txt` for 42.x).
fn ensure_reset_sentinel(profile: &Path, game_version: &str) -> Result<()> {
    let major = game_version.split('.').next().unwrap_or("42");
    let sentinel = profile
        .join("mods")
        .join(format!("reset-mods-{major}_00.txt"));
    if sentinel.exists() {
        return Ok(());
    }
    fs::write(
        &sentinel,
        "If this file does not exist, default.txt will be reset to empty (no mods active).",
    )
    .with_context(|| format!("writing {}", sentinel.display()))
}

/// The client activation list in the game's exact format: ordered
/// `mod = <ModID>,` entries, with the maps block preserved from the
/// existing file.
fn default_txt_text(mod_ids: &[String], maps_block: &str) -> String {
    let mut text = String::from("VERSION = 1,\n\nmods\n{\n");
    for id in mod_ids {
        text.push_str(&format!("    mod = {id},\n"));
    }
    text.push_str("}\n\nmaps\n{\n");
    text.push_str(maps_block);
    text.push_str("}\n");
    text
}

/// The maps block of an existing default.txt, verbatim; empty when
/// absent.
fn existing_maps_block(path: &Path) -> String {
    let Ok(text) = fs::read_to_string(path) else {
        return String::new();
    };
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line.trim() == "maps" && lines.next().is_some_and(|l| l.trim() == "{") {
            let mut block = String::new();
            for line in lines.by_ref() {
                if line.trim() == "}" {
                    return block;
                }
                block.push_str(line);
                block.push('\n');
            }
            return block;
        }
    }
    String::new()
}

/// Write the server config's Mods= line: a minimal config (with the
/// instance name as the browser-visible PublicName) when absent, or
/// the existing config with only the Mods= line replaced.
fn write_server_ini(path: &Path, mod_ids: &[String], instance_name: &str) -> Result<()> {
    let mods_line = format!("Mods={}", mod_ids.join(";"));
    if path.exists() {
        let text =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut out = String::new();
        let mut replaced = false;
        for line in text.lines() {
            if line.trim_start().to_lowercase().starts_with("mods=") {
                out.push_str(&mods_line);
                replaced = true;
            } else {
                out.push_str(line);
            }
            out.push('\n');
        }
        if !replaced {
            out.push_str(&mods_line);
            out.push('\n');
        }
        fs::write(path, out).with_context(|| format!("writing {}", path.display()))
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = format!("PublicName={instance_name}\n{mods_line}\n");
        fs::write(path, text).with_context(|| format!("writing {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsops::noop_progress;
    use crate::pz::item::{inspect_item, Layout};
    use crate::store::meta::Source;

    /// Build a store with a game version and an instance, plus a
    /// workshop item bundling three mods:
    /// - "Hybrid Mod": root manifest, folders 42 (fieldless) and 42.20
    ///   (versionMin=42.20), common/ — on 42.22 the game loads 42.20
    ///   plus common/.
    /// - "Bounded Mod": folder 42.16 declaring 42.16 - 42.19 — skipped.
    /// - "B41 Only": root manifest only — skipped on B42.
    fn fixture(name: &str) -> Result<(Store, GameInstance, StoredMod, PathBuf)> {
        let base =
            std::env::temp_dir().join(format!("snapoid-install-{name}-{}", std::process::id()));
        let store = Store::open_at(&base)?;

        let game = store.game_dir("42.22.2");
        fs::create_dir_all(&game)?;
        fs::write(game.join("projectzomboid.jar"), "jar")?;
        let instance = store.instance_new("test", "42.22.2")?;

        let item_dir = base.join("item-123");
        let mods = item_dir.join("mods");
        fs::create_dir_all(mods.join("Hybrid Mod/42/media"))?;
        fs::create_dir_all(mods.join("Hybrid Mod/42.20/media"))?;
        fs::create_dir_all(mods.join("Hybrid Mod/common/media"))?;
        fs::create_dir_all(mods.join("Hybrid Mod/media"))?;
        fs::write(mods.join("Hybrid Mod/mod.info"), "name=Hybrid\nid=HM\n")?;
        fs::write(mods.join("Hybrid Mod/media/root.lua"), "root")?;
        fs::write(mods.join("Hybrid Mod/42/mod.info"), "name=Hybrid\nid=HM\n")?;
        fs::write(mods.join("Hybrid Mod/42/media/old.lua"), "old")?;
        fs::write(
            mods.join("Hybrid Mod/42.20/mod.info"),
            "name=Hybrid\nid=HM\nversionMin=42.20\n",
        )?;
        fs::write(mods.join("Hybrid Mod/42.20/media/new.lua"), "new")?;
        fs::write(mods.join("Hybrid Mod/common/mod.info"), "name=Hybrid\nid=HM\n")?;
        fs::write(mods.join("Hybrid Mod/common/media/shared.lua"), "shared")?;

        fs::create_dir_all(mods.join("Bounded Mod/42.16"))?;
        fs::write(
            mods.join("Bounded Mod/42.16/mod.info"),
            "name=Bounded\nid=BM\nversionMin=42.16\nversionMax=42.19\n",
        )?;

        fs::create_dir_all(mods.join("B41 Only"))?;
        fs::write(mods.join("B41 Only/mod.info"), "name=Old\nid=OM\n")?;

        // The Project Viewpoint shape: a media-only version folder
        // (no mod.info) recognized through common/.
        fs::create_dir_all(mods.join("Media Only/42/media"))?;
        fs::create_dir_all(mods.join("Media Only/common/media"))?;
        fs::write(mods.join("Media Only/42/media/m.lua"), "m")?;
        fs::write(mods.join("Media Only/common/mod.info"), "name=Media\nid=MO\n")?;
        fs::write(mods.join("Media Only/common/media/c.lua"), "c")?;

        // The workshop item 3810302175 shape: a patch-suffixed folder
        // holding only mod.info, with all the content in a manifest-
        // less common/.
        fs::create_dir_all(mods.join("Patch Mod/42.20.4"))?;
        fs::create_dir_all(mods.join("Patch Mod/common/media"))?;
        fs::write(
            mods.join("Patch Mod/42.20.4/mod.info"),
            "name=Patch\nid=PM\nversionMin=42.22.2\n",
        )?;
        fs::write(mods.join("Patch Mod/common/media/m.lua"), "m")?;

        let mods = inspect_item(&item_dir);
        assert_eq!(mods.len(), 5);
        let stored = StoredMod {
            id: 123,
            time_updated: 1,
            dir: item_dir.clone(),
            created: 1,
            title: None,
            source: Some(Source::WorkshopDisk),
            mods,
            layout: None,
        };
        Ok((store, instance, stored, base))
    }

    /// The outcome for a bundled mod, by folder name.
    fn mod_outcome<'a>(installed: &'a [InstalledMod], name: &str) -> &'a InstalledMod {
        installed.iter().find(|m| m.name == name).unwrap()
    }

    #[test]
    fn installs_only_what_the_game_loads() -> Result<()> {
        let (store, instance, item, base) = fixture("b42")?;
        let outcome = store.instance_install_mods(&instance, &[&item], noop_progress)?;
        let installed = &outcome.mods;
        assert_eq!(installed.len(), 5);

        // Hybrid Mod: the selected version folder plus common/, and
        // nothing else.
        let hybrid = mod_outcome(installed, "Hybrid Mod");
        let dir = hybrid.dir.as_ref().unwrap();
        assert!(dir.join("42.20/mod.info").is_file());
        assert!(dir.join("42.20/media/new.lua").is_file());
        assert!(dir.join("common/media/shared.lua").is_file());
        assert!(!dir.join("42").exists(), "unselected folder must stay behind");
        assert!(!dir.join("42/media/old.lua").exists());
        assert!(!dir.join("mod.info").exists(), "B41 root layout must stay behind");
        assert!(!dir.join("media/root.lua").exists());
        assert_eq!(hybrid.mod_id.as_deref(), Some("HM"));

        // Bounded Mod: the game would mark it unavailable on 42.22.
        let bounded = mod_outcome(installed, "Bounded Mod");
        assert!(bounded.dir.is_none());
        assert!(matches!(
            &bounded.skipped,
            Some(compat::Incompatibility::DeclaredRange { .. })
        ));
        assert!(!instance.profile_dir.join("mods/Bounded Mod").exists());

        // B41 Only: invisible to the B42 loader.
        let b41_only = mod_outcome(installed, "B41 Only");
        assert!(matches!(
            &b41_only.skipped,
            Some(compat::Incompatibility::LayoutOnly { .. })
        ));

        // Media Only: the game selects the media-only 42 folder by
        // name and reads common/'s manifest — both get copied.
        let media = mod_outcome(installed, "Media Only");
        let dir = media.dir.as_ref().unwrap();
        assert!(dir.join("42/media/m.lua").is_file());
        assert!(dir.join("common/mod.info").is_file());
        assert!(dir.join("common/media/c.lua").is_file());

        // Patch Mod: the manifest-less common/ carries all the content
        // and must be copied alongside the 1-file version folder.
        let patch = mod_outcome(installed, "Patch Mod");
        let dir = patch.dir.as_ref().unwrap();
        assert!(dir.join("42.20.4/mod.info").is_file());
        assert!(dir.join("common/media/m.lua").is_file());

        // No staging leftovers in the profile's mods folder — the game
        // scans it and would recognize them as mods.
        let mods_dir = instance.profile_dir.join("mods");
        let mut names: Vec<String> = fs::read_dir(&mods_dir)?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "Hybrid Mod",
                "Media Only",
                "Patch Mod",
                "default.txt",
                "reset-mods-42_00.txt"
            ]
        );

        // Activation: the installed set is written to both lists in
        // install order (no constraints in this fixture).
        assert_eq!(outcome.activation.mod_ids, vec!["HM", "MO", "PM"]);
        assert!(outcome.activation.warnings.is_empty());
        let default_txt = fs::read_to_string(instance.profile_dir.join("mods/default.txt"))?;
        assert_eq!(
            default_txt,
            "VERSION = 1,\n\nmods\n{\n    mod = HM,\n    mod = MO,\n    mod = PM,\n}\n\nmaps\n{\n}\n"
        );
        let server_ini = instance
            .profile_dir
            .join("Server")
            .join("snapoid_server_test.ini");
        assert_eq!(outcome.activation.server_ini, server_ini);
        let ini = fs::read_to_string(&server_ini)?;
        assert_eq!(ini, "PublicName=test\nMods=HM;MO;PM\n");

        // The reset sentinel is pre-created: without it the game wipes
        // default.txt on the profile's first launch.
        let sentinel = instance
            .profile_dir
            .join("mods/reset-mods-42_00.txt");
        assert_eq!(
            fs::read_to_string(&sentinel)?,
            "If this file does not exist, default.txt will be reset to empty (no mods active)."
        );

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn reinstall_replaces_the_previous_copy() -> Result<()> {
        let (store, instance, item, base) = fixture("reinstall")?;
        store.instance_install_mods(&instance, &[&item], noop_progress)?;
        let target = instance.profile_dir.join("mods/Hybrid Mod");
        fs::write(target.join("42.20/media/new.lua"), "stale")?;

        // Update the store copy and re-install.
        fs::write(
            item.dir.join("mods/Hybrid Mod/42.20/media/new.lua"),
            "fresh",
        )?;
        store.instance_install_mods(&instance, &[&item], noop_progress)?;
        assert_eq!(
            fs::read_to_string(target.join("42.20/media/new.lua"))?,
            "fresh"
        );

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn b41_game_gets_the_root_layout() -> Result<()> {
        let (store, _instance, item, base) = fixture("b41")?;
        let game = store.game_dir("41.5");
        fs::create_dir_all(&game)?;
        fs::write(game.join("projectzomboid.jar"), "jar")?;
        let instance = store.instance_new("legacy", "41.5")?;

        let outcome = store.instance_install_mods(&instance, &[&item], noop_progress)?;
        let installed = &outcome.mods;
        // On B41 the root layout loads: Hybrid Mod and B41 Only are
        // compatible, Bounded Mod declares a B42 range.
        let hybrid = mod_outcome(installed, "Hybrid Mod");
        let dir = hybrid.dir.as_ref().unwrap();
        assert!(dir.join("mod.info").is_file());
        assert!(dir.join("media/root.lua").is_file());
        assert!(!dir.join("42.20").exists(), "version folders stay behind");
        assert!(!dir.join("common").exists(), "B41 ignores common/");
        assert!(mod_outcome(installed, "B41 Only").dir.is_some(), "B41 Only loads on B41");
        assert!(mod_outcome(installed, "Bounded Mod").dir.is_none());

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    fn mod_entry(id: &str, requires: &[&str], before: &[&str], after: &[&str]) -> (String, ModEntry) {
        (
            id.to_owned(),
            ModEntry {
                name: format!("Mod {id}"),
                mod_id: Some(id.to_owned()),
                title: None,
                author: None,
                folders: Vec::new(),
                common_dir: false,
                common: None,
                root: None,
                requires: requires.iter().map(|s| s.to_string()).collect(),
                incompatible: Vec::new(),
                load_before: before.iter().map(|s| s.to_string()).collect(),
                load_after: after.iter().map(|s| s.to_string()).collect(),
                layout: Layout::B42,
            },
        )
    }

    #[test]
    fn orders_dependencies_first_then_load_overrides() {
        // B needs A (require), C loads after B, D loads before A.
        let entries = [
            mod_entry("B", &["A"], &[], &[]),
            mod_entry("A", &[], &[], &[]),
            mod_entry("C", &[], &[], &["B"]),
            mod_entry("D", &[], &["A"], &[]),
        ];
        let refs: Vec<(String, &ModEntry)> = entries
            .iter()
            .map(|(id, e)| (id.clone(), e))
            .collect();
        let (order, warnings) = activation_order(&refs);
        assert_eq!(order, vec!["D", "A", "B", "C"]);
        assert!(warnings.is_empty());
    }

    #[test]
    fn warns_on_missing_references_and_cycles() {
        let entries = [
            mod_entry("A", &["Missing"], &[], &[]),
            mod_entry("B", &[], &[], &["A"]),
            // A loads after B and B loads after A: a cycle.
            mod_entry("A2", &[], &[], &["B2"]),
            mod_entry("B2", &[], &[], &["A2"]),
        ];
        let refs: Vec<(String, &ModEntry)> = entries
            .iter()
            .map(|(id, e)| (id.clone(), e))
            .collect();
        let (order, warnings) = activation_order(&refs);
        // The cycle falls back to install order; everything is present.
        assert_eq!(order, vec!["A", "B", "A2", "B2"]);
        assert!(warnings
            .iter()
            .any(|w| w.contains("A needs Missing")));
        assert!(warnings.iter().any(|w| w.contains("cycle")));
    }

    #[test]
    fn default_txt_preserves_the_maps_block() {
        let text = default_txt_text(&["A".to_owned(), "B".to_owned()], "    map = X,\n");
        assert_eq!(
            text,
            "VERSION = 1,\n\nmods\n{\n    mod = A,\n    mod = B,\n}\n\nmaps\n{\n    map = X,\n}\n"
        );
    }

    #[test]
    fn server_ini_is_created_and_preserved() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-ini-{}", std::process::id()));
        let ini = base.join("Server/snapoid_server_test.ini");

        // Created fresh: browser-visible name plus the mod line.
        write_server_ini(&ini, &["A".to_owned(), "B".to_owned()], "home")?;
        assert_eq!(
            fs::read_to_string(&ini)?,
            "PublicName=home\nMods=A;B\n"
        );

        // Existing config: only the Mods= line is replaced.
        fs::write(
            &ini,
            "PublicName=My Server\nMods=OLD;STUFF\nPassword=secret\n",
        )?;
        write_server_ini(&ini, &["A".to_owned()], "home")?;
        let text = fs::read_to_string(&ini)?;
        assert!(text.contains("PublicName=My Server"));
        assert!(text.contains("Mods=A"));
        assert!(!text.contains("OLD"));
        assert!(text.contains("Password=secret"));

        fs::remove_dir_all(&base)?;
        Ok(())
    }
}
