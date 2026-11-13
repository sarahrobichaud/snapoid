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
//! Exception — ZombieBuddy copies whole: its loader and agent jar live
//! in `libs/`, outside anything the game's loader reads, and the
//! profile copy must be complete — the game's own dialog points users
//! at that folder, and the agent install reads the loader from it.
//!
//! Staging happens in the instance dir, never inside `profile/mods/`:
//! the game scans that folder and would recognize a staging dir
//! containing `common/mod.info` as a mod.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::fmt;
use crate::fsops::{copy_tree, measure_tree, Progress, ProgressFactory};
use crate::launch;
use crate::pz::compat::{self, Compatibility};
use crate::pz::item::ModEntry;
use crate::pz::mod_info;
use crate::pz::version;
use crate::pz::zombie_buddy;
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
    /// The mod's title, when a manifest or the workshop item declares
    /// one.
    pub title: Option<String>,
    /// Steam's timeupdated for the item at install time; 0 when the
    /// install carries no store provenance (a modpack).
    pub time_updated: u64,
    /// The installed folder in the profile, when installed.
    pub dir: Option<PathBuf>,
    /// Why the mod was skipped, when it was.
    pub skipped: Option<compat::Incompatibility>,
    /// True when the install forced a mod past its declared version
    /// range by rewriting its manifest (unsafe mode).
    pub unsafe_install: bool,
}

/// The install manifest written into the profile's mods folder: what
/// yapzm installed there, and from which store version, so a later
/// install can offer updates instead of blind reinstalls. A plain file
/// next to default.txt — the game only scans mod folders.
pub const INSTALLED_MANIFEST: &str = "yapzm-mods.json";

/// One installed mod's provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModRecord {
    /// Mod folder name in the profile's mods folder.
    pub name: String,
    /// The Mod ID, when the manifest declared one.
    pub mod_id: Option<String>,
    /// Workshop item the mod came from; 0 when the install carried no
    /// store provenance (a modpack).
    pub item_id: u64,
    /// Steam's timeupdated for the item at install time; 0 when
    /// unknown.
    pub time_updated: u64,
    /// The mod's title, when known.
    pub title: Option<String>,
    /// True when the install forced the mod past its declared version
    /// range by rewriting its manifest.
    #[serde(default)]
    pub unsafe_install: bool,
}

/// The install manifest itself.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstalledManifest {
    pub format: u32,
    pub mods: Vec<ModRecord>,
}

impl InstalledManifest {
    /// The record for a mod folder name, when one is kept.
    pub fn record(&self, name: &str) -> Option<&ModRecord> {
        self.mods.iter().find(|r| r.name == name)
    }

    /// Insert or replace the record for a mod folder name.
    fn upsert(&mut self, record: ModRecord) {
        match self.mods.iter_mut().find(|r| r.name == record.name) {
            Some(existing) => *existing = record,
            None => self.mods.push(record),
        }
    }
}

/// One mod picked for install: the stored item it came from and the
/// bundled mod to install from it. An item bundles several mods; the
/// CLI picks them individually.
#[derive(Debug)]
pub struct SelectedMod<'a> {
    /// The stored workshop item the mod is bundled in.
    pub item: &'a StoredMod,
    /// The bundled mod to install.
    pub entry: &'a ModEntry,
    /// Install in unsafe mode: force the mod past its declared
    /// version range by rewriting its manifest.
    pub unsafe_install: bool,
}

/// What the install manifest says about one bundled mod relative to
/// its stored version.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModStatus {
    /// Not installed — or installed without a manifest record, which
    /// cannot be told apart.
    New,
    /// Installed at the store's version.
    UpToDate,
    /// Installed, but the store holds a newer capture; `installed` is
    /// the recorded install time.
    UpdateAvailable { installed: u64 },
}

/// Classify one bundled mod against the instance's install manifest.
/// Callers pass mods that would install on the game version — for the
/// rest the status is meaningless.
pub fn mod_install_status(
    item: &StoredMod,
    entry: &ModEntry,
    manifest: &InstalledManifest,
) -> ModStatus {
    match manifest.record(&entry.name) {
        None => ModStatus::New,
        Some(record) if item.time_updated > record.time_updated => {
            ModStatus::UpdateAvailable {
                installed: record.time_updated,
            }
        }
        Some(_) => ModStatus::UpToDate,
    }
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
    /// ZombieBuddy agent files copied into the instance's agent dir
    /// (and the game folder, where the Windows loader needs them);
    /// empty when the mod is not among the installed ones.
    pub agent_files: Vec<String>,
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
    // ZombieBuddy copies whole: the loader and agent jar in libs/ are
    // outside anything the game's loader reads, and the profile copy
    // must be complete (the game's own dialog points users at it, and
    // the agent install reads the loader from it).
    if zombie_buddy::is_mod_name(&mod_entry.name)
        || mod_entry
            .mod_id
            .as_deref()
            .is_some_and(zombie_buddy::is_mod_name)
    {
        return vec![CopyOp::RootMinus { exclude: Vec::new() }];
    }

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
    /// The instance's install manifest: what yapzm has installed
    /// into its profile and from which store versions.
    pub fn instance_installed_manifest(&self, instance: &GameInstance) -> InstalledManifest {
        read_installed_manifest(&instance.profile_dir.join("mods"))
    }

    /// Install the bundled mods of `items` into the instance's profile
    /// mods folder. Only what the game would load on the instance's
    /// game version is copied (ZombieBuddy: its whole folder); mods the
    /// game would not make available are skipped with their reason.
    /// Re-installing a mod replaces the previous copy.
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
        self.instance_install_mods_with(instance, items, &[], progress)
    }

    /// The same install with a second set of items installed in unsafe
    /// mode: mods whose declared version range excludes the instance's
    /// game version are installed anyway, with the manifest the game
    /// would read rewritten to that version (`versionMin=` pinned,
    /// `versionMax=` dropped) and the display name tagged. Mods whose
    /// incompatibility a manifest rewrite cannot fix (layout-only,
    /// no selectable folder, no manifest at all) are still skipped
    /// with their reason.
    pub fn instance_install_mods_with(
        &self,
        instance: &GameInstance,
        items: &[&StoredMod],
        unsafe_items: &[&StoredMod],
        progress: ProgressFactory,
    ) -> Result<InstallOutcome> {
        let selected: Vec<SelectedMod> = items
            .iter()
            .flat_map(|item| {
                item.mods
                    .iter()
                    .map(|entry| SelectedMod {
                        item,
                        entry,
                        unsafe_install: false,
                    })
            })
            .chain(unsafe_items.iter().flat_map(|item| {
                item.mods
                    .iter()
                    .map(|entry| SelectedMod {
                        item,
                        entry,
                        unsafe_install: true,
                    })
            }))
            .collect();
        self.instance_install_selected(instance, &selected, progress)
    }

    /// Install individually picked mods: each [`SelectedMod`] is one
    /// bundled mod with its item, so an item's mods can be installed
    /// selectively instead of as a whole bundle.
    pub fn instance_install_selected(
        &self,
        instance: &GameInstance,
        selected: &[SelectedMod],
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
        let mut zombie_buddy_dir: Option<PathBuf> = None;
        for select in selected {
            let item = select.item;
            let unsafe_install = select.unsafe_install;
            let mod_entry = select.entry;
            {
                let verdict = compat::mod_compatibility(mod_entry, &instance.version);
                // An unsafe install fixes exactly one failure mode: a
                // declared version range excluding the running
                // version — the manifest the game would read is
                // rewritten. Everything else (layout-only, no
                // selectable folder, no manifest at all, no signal)
                // is skipped with its reason, unsafe request or not.
                let (skipped, rewrite) = match (&verdict, unsafe_install) {
                    (Compatibility::Compatible, _) => (None, None),
                    (Compatibility::Unknown, _) => {
                        warn!(
                            "mod {} has no compatibility signal; skipping",
                            mod_entry.name
                        );
                        continue;
                    }
                    (Compatibility::Incompatible(reason), false) => (Some(reason.clone()), None),
                    (
                        Compatibility::Incompatible(compat::Incompatibility::DeclaredRange {
                            manifest,
                            ..
                        }),
                        true,
                    ) => (None, Some((manifest.clone(), instance.version.clone()))),
                    (Compatibility::Incompatible(reason), true) => {
                        warn!(
                            "mod {} cannot be installed unsafely; skipping",
                            mod_entry.name
                        );
                        (Some(reason.clone()), None)
                    }
                };
                let outcome = match skipped {
                    Some(reason) => InstalledMod {
                        item_id: item.id,
                        name: mod_entry.name.clone(),
                        mod_id: mod_entry.mod_id.clone(),
                        title: mod_entry.title.clone().or_else(|| item.title.clone()),
                        time_updated: item.time_updated,
                        dir: None,
                        skipped: Some(reason),
                        unsafe_install: false,
                    },
                    None => {
                        let unsafe_installed = rewrite.is_some();
                        let dir = self.install_one(
                            &staging_root,
                            &mods_dir,
                            item,
                            mod_entry,
                            game,
                            rewrite,
                            progress,
                        )?;
                        match &mod_entry.mod_id {
                            Some(id) => activated.push((id.clone(), mod_entry)),
                            None => no_id.push(format!(
                                "mod {} has no Mod ID and cannot be activated",
                                mod_entry.name
                            )),
                        }
                        if zombie_buddy::is_mod_name(&mod_entry.name)
                            || mod_entry
                                .mod_id
                                .as_deref()
                                .is_some_and(zombie_buddy::is_mod_name)
                        {
                            zombie_buddy_dir = Some(dir.clone());
                        }
                        InstalledMod {
                            item_id: item.id,
                            name: mod_entry.name.clone(),
                            mod_id: mod_entry.mod_id.clone(),
                            title: mod_entry.title.clone().or_else(|| item.title.clone()),
                            time_updated: item.time_updated,
                            dir: Some(dir),
                            skipped: None,
                            unsafe_install: unsafe_installed,
                        }
                    }
                };
                installed.push(outcome);
            }
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
        Ok(InstallOutcome {
            mods: installed,
            activation,
            agent_files,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn install_one(
        &self,
        staging_root: &Path,
        mods_dir: &Path,
        item: &StoredMod,
        mod_entry: &ModEntry,
        game: u64,
        rewrite: Option<(compat::Manifest, String)>,
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

        let started = Instant::now();
        let ops = copy_ops(mod_entry, game);
        if ops.is_empty() {
            anyhow::bail!(
                "mod {} has nothing loadable on version {}",
                mod_entry.name,
                game
            );
        }
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

        // Unsafe install: rewrite the manifest the game would read,
        // inside the staged copy — the store copy stays untouched.
        if let Some((manifest, game_version)) = rewrite {
            rewrite_manifest_unsafe(&staging, manifest, &mod_entry.name, &game_version)?;
        }

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

/// Activate the installed mods and write the artifacts every install
/// path shares: the dependency-aware load order (ZombieBuddy promoted
/// first), the agent install, the reset sentinel, the client's
/// mods/default.txt and the server config's Mods= line, and the
/// install manifest recording what was installed from where. Returns
/// the activation record and the ZombieBuddy agent file names.
pub(crate) fn activate_install(
    instance: &GameInstance,
    installed: &[InstalledMod],
    activated: &[(String, &ModEntry)],
    extra_warnings: Vec<String>,
    zombie_buddy_dir: Option<&Path>,
) -> Result<(Activation, Vec<String>)> {
    write_installed_manifest(&instance.profile_dir.join("mods"), installed)?;
    let (mut mod_ids, mut warnings) = activation_order(activated);
    warnings.extend(extra_warnings);
    // The agent install needs the mod in place, so it runs after the
    // files land; the load-order promotion happens before anything is
    // written, so every artifact agrees on ZombieBuddy loading first.
    // The agent goes into the instance's own agent/ dir — never the
    // shared per-version game folder, which would leak the library
    // into every instance of that version.
    zombie_buddy::promote_first(&mut mod_ids);
    let agent_files = match zombie_buddy_dir {
        Some(dir) => zombie_buddy::install_agent(
            dir,
            &instance.dir.join(zombie_buddy::AGENT_DIR),
            &instance.game_dir,
        )?,
        None => Vec::new(),
    };
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
    Ok((
        Activation {
            mod_ids,
            warnings,
            default_txt,
            server_ini,
        },
        agent_files,
    ))
}

/// Rewrite the manifest the game would read on this version, inside a
/// staged mod copy, for an unsafe install: its declared range is
/// pinned to the running game version and its display name tagged.
/// The manifest sits where compat found it — the staged copy holds
/// exactly what the copy operations put there.
fn rewrite_manifest_unsafe(
    staging: &Path,
    manifest: compat::Manifest,
    mod_name: &str,
    game_version: &str,
) -> Result<()> {
    let path = match manifest {
        compat::Manifest::Root => staging.join("mod.info"),
        compat::Manifest::Common => staging.join("common").join("mod.info"),
        compat::Manifest::Folder(name) => staging.join(name).join("mod.info"),
    };
    let text =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let rewritten = mod_info::unsafe_text(&text, mod_name, game_version);
    fs::write(&path, rewritten).with_context(|| format!("writing {}", path.display()))?;
    warn!(
        "unsafe install: {}'s manifest rewritten to version {}",
        mod_name, game_version
    );
    Ok(())
}

/// Read the profile's install manifest; an empty one when absent or
/// unreadable (the manifest is yapzm's own bookkeeping — a broken
/// file must not block an install).
pub(crate) fn read_installed_manifest(mods_dir: &Path) -> InstalledManifest {
    let path = mods_dir.join(INSTALLED_MANIFEST);
    match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(manifest) => manifest,
            Err(error) => {
                warn!(
                    "ignoring unreadable install manifest {}: {error}",
                    path.display()
                );
                InstalledManifest::default()
            }
        },
        Err(_) => InstalledManifest::default(),
    }
}

/// Merge `installed` into the profile's install manifest and write it
/// back: records for mod folders that no longer exist are dropped, and
/// everything installed here is recorded with its provenance.
fn write_installed_manifest(mods_dir: &Path, installed: &[InstalledMod]) -> Result<()> {
    let mut manifest = read_installed_manifest(mods_dir);
    manifest.format = 1;
    manifest
        .mods
        .retain(|record| mods_dir.join(&record.name).is_dir());
    for outcome in installed.iter().filter(|m| m.dir.is_some()) {
        manifest.upsert(ModRecord {
            name: outcome.name.clone(),
            mod_id: outcome.mod_id.clone(),
            item_id: outcome.item_id,
            time_updated: outcome.time_updated,
            title: outcome.title.clone(),
            unsafe_install: outcome.unsafe_install,
        });
    }
    let path = mods_dir.join(INSTALLED_MANIFEST);
    let text = serde_json::to_vec_pretty(&manifest)?;
    fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
}

/// Order the installed mods for activation: `require=` dependencies
/// first, then `loadModAfter=`/`loadModBefore=` constraints, with the
/// install order breaking ties. Unresolvable references and cycles are
/// warned about, never fatal — the game drops what it cannot resolve.
/// (ZombieBuddy is promoted first by the caller.)
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
            std::env::temp_dir().join(format!("yapzm-install-{name}-{}", std::process::id()));
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
    fn zombie_buddy_gets_its_agent_installed_and_loads_first() -> Result<()> {
        let (store, instance, _item, base) = fixture("zb")?;

        // A second item bundling ZombieBuddy, shaped like the real
        // workshop mod: version folders plus a libs/ dir with the
        // native loader and the agent jar.
        let item_dir = base.join("item-456");
        let mods = item_dir.join("mods");
        fs::create_dir_all(mods.join("ZombieBuddy/42/media"))?;
        fs::create_dir_all(mods.join("ZombieBuddy/libs"))?;
        fs::write(
            mods.join("ZombieBuddy/42/mod.info"),
            "name=ZombieBuddy\nid=ZombieBuddy\n",
        )?;
        fs::write(mods.join("ZombieBuddy/42/media/buddy.lua"), "lua")?;
        fs::write(mods.join("ZombieBuddy/libs/zbNative.dll"), "loader")?;
        fs::write(mods.join("ZombieBuddy/libs/ZombieBuddy.jar"), "agent jar")?;
        fs::write(mods.join("ZombieBuddy/libs/ZombieBuddy.jar.zbs"), "signature")?;
        let zombie_buddy_item = StoredMod {
            id: 456,
            time_updated: 1,
            dir: item_dir.clone(),
            created: 1,
            title: None,
            source: Some(Source::WorkshopDisk),
            mods: inspect_item(&item_dir),
            layout: None,
        };

        // ZombieBuddy installs after another mod, yet must load first.
        let outcome = store.instance_install_mods(
            &instance,
            &[&_item, &zombie_buddy_item],
            noop_progress,
        )?;

        // The whole mod folder lands in the profile (the ZombieBuddy
        // exception): libs/ included, so the profile copy is complete.
        assert!(instance
            .profile_dir
            .join("mods/ZombieBuddy/libs/zbNative.dll")
            .is_file());

        // The agent files are copied into the instance's own agent/
        // dir; the jar (with its sidecar) also lands in the game
        // folder, where the Windows loader resolves it relative to the
        // game's working directory. The loader itself never does.
        assert_eq!(
            outcome.agent_files,
            vec!["zbNative.dll", "ZombieBuddy.jar", "ZombieBuddy.jar.zbs"]
        );
        assert_eq!(
            fs::read(instance.dir.join("agent/zbNative.dll"))?,
            b"loader"
        );
        assert_eq!(
            fs::read(instance.dir.join("agent/ZombieBuddy.jar"))?,
            b"agent jar"
        );
        assert_eq!(
            fs::read(instance.game_dir.join("ZombieBuddy.jar"))?,
            b"agent jar"
        );
        assert!(
            !instance.game_dir.join("zbNative.dll").exists(),
            "the loader must stay out of the shared game folder"
        );

        // Load order: ZombieBuddy first, everywhere (default.txt and
        // the server's Mods= line).
        assert_eq!(
            outcome.activation.mod_ids,
            vec!["ZombieBuddy", "HM", "MO", "PM"]
        );
        let default_txt = fs::read_to_string(instance.profile_dir.join("mods/default.txt"))?;
        assert!(default_txt.contains("mod = ZombieBuddy,\n    mod = HM,"));
        let ini = fs::read_to_string(
            instance
                .profile_dir
                .join("Server")
                .join("yapzm_server_test.ini"),
        )?;
        assert_eq!(ini, "PublicName=test\nMods=ZombieBuddy;HM;MO;PM\n");

        fs::remove_dir_all(&base)?;
        Ok(())
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
                "reset-mods-42_00.txt",
                "yapzm-mods.json"
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
            .join("yapzm_server_test.ini");
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
        let base = std::env::temp_dir().join(format!("yapzm-ini-{}", std::process::id()));
        let ini = base.join("Server/yapzm_server_test.ini");

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

    #[test]
    fn installing_writes_the_manifest() -> Result<()> {
        let (store, instance, item, base) = fixture("manifest")?;
        store.instance_install_mods(&instance, &[&item], noop_progress)?;

        let manifest = store.instance_installed_manifest(&instance);
        assert_eq!(manifest.format, 1);
        // Only the mods that installed are recorded; the skipped ones
        // (Bounded Mod, B41 Only) are not.
        let names: Vec<&str> = manifest.mods.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["Hybrid Mod", "Media Only", "Patch Mod"]);
        let hybrid = manifest.record("Hybrid Mod").unwrap();
        assert_eq!(hybrid.item_id, 123);
        assert_eq!(hybrid.time_updated, item.time_updated);
        assert_eq!(hybrid.mod_id.as_deref(), Some("HM"));
        assert!(manifest.record("Bounded Mod").is_none());

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn reinstalling_updates_the_manifest_record() -> Result<()> {
        let (store, instance, mut item, base) = fixture("manifest-update")?;
        store.instance_install_mods(&instance, &[&item], noop_progress)?;

        // The workshop item updates; re-installing must move the
        // record to the new version.
        item.time_updated = 999;
        store.instance_install_mods(&instance, &[&item], noop_progress)?;
        let manifest = store.instance_installed_manifest(&instance);
        assert_eq!(manifest.record("Hybrid Mod").unwrap().time_updated, 999);

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn the_manifest_keeps_records_of_untouched_mods() -> Result<()> {
        let (store, instance, item, base) = fixture("manifest-keep")?;
        store.instance_install_mods(&instance, &[&item], noop_progress)?;

        // A later install of a different item must keep the first
        // install's records, and drop records whose mod folder is
        // gone from the profile.
        fs::remove_dir_all(instance.profile_dir.join("mods/Patch Mod"))?;
        let other_dir = base.join("item-789");
        fs::create_dir_all(other_dir.join("mods/Other Mod/42"))?;
        fs::write(
            other_dir.join("mods/Other Mod/42/mod.info"),
            "name=Other\nid=OM2\n",
        )?;
        let other = StoredMod {
            id: 789,
            time_updated: 7,
            dir: other_dir.clone(),
            created: 1,
            title: None,
            source: Some(Source::WorkshopDisk),
            mods: inspect_item(&other_dir),
            layout: None,
        };
        store.instance_install_mods(&instance, &[&other], noop_progress)?;

        let manifest = store.instance_installed_manifest(&instance);
        assert!(manifest.record("Hybrid Mod").is_some());
        assert!(manifest.record("Media Only").is_some());
        assert!(manifest.record("Patch Mod").is_none(), "stale");
        assert_eq!(manifest.record("Other Mod").unwrap().item_id, 789);

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn unsafe_install_forces_a_declared_range_and_tags_the_name() -> Result<()> {
        let (store, instance, item, base) = fixture("unsafe")?;
        let outcome =
            store.instance_install_mods_with(&instance, &[], &[&item], noop_progress)?;

        // Bounded Mod declares 42.16-42.19; the unsafe install copies
        // the folder the game would select and rewrites its manifest
        // to the running version, tagging the display name.
        let bounded = mod_outcome(&outcome.mods, "Bounded Mod");
        assert!(bounded.unsafe_install);
        let dir = bounded.dir.as_ref().unwrap();
        let rewritten = fs::read_to_string(dir.join("42.16/mod.info"))?;
        assert!(rewritten.contains("name=Bounded [unsafe]"));
        assert!(rewritten.contains("versionMin=42.22.2"));
        assert!(!rewritten.contains("versionMax"));
        assert_eq!(bounded.skipped, None);

        // The compatible mods of the same item install normally.
        assert!(!mod_outcome(&outcome.mods, "Hybrid Mod").unsafe_install);
        assert!(mod_outcome(&outcome.mods, "Hybrid Mod").dir.is_some());

        // The forced mod is activated like any other.
        assert_eq!(
            outcome.activation.mod_ids,
            vec!["BM", "HM", "MO", "PM"]
        );

        // The manifest record remembers the unsafe install.
        let manifest = store.instance_installed_manifest(&instance);
        assert!(manifest.record("Bounded Mod").unwrap().unsafe_install);
        assert!(!manifest.record("Hybrid Mod").unwrap().unsafe_install);

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn unsafe_install_cannot_fix_a_layout_only_mod() -> Result<()> {
        let (store, instance, item, base) = fixture("unsafe-layout")?;
        let outcome =
            store.instance_install_mods_with(&instance, &[], &[&item], noop_progress)?;

        // B41 Only ships the B41 layout; no manifest rewrite can make
        // a B42 loader read it.
        let b41 = mod_outcome(&outcome.mods, "B41 Only");
        assert!(b41.dir.is_none());
        assert!(b41.skipped.is_some());
        assert!(!b41.unsafe_install);
        assert!(!instance.profile_dir.join("mods/B41 Only").exists());

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn mod_status_reflects_the_manifest() -> Result<()> {
        let base = std::env::temp_dir().join(format!("yapzm-status-{}", std::process::id()));
        let item_dir = base.join("item-1");
        fs::create_dir_all(item_dir.join("mods/Plain/42"))?;
        fs::write(item_dir.join("mods/Plain/42/mod.info"), "name=P\nid=P\n")?;
        let mut item = StoredMod {
            id: 1,
            time_updated: 10,
            dir: item_dir.clone(),
            created: 1,
            title: None,
            source: Some(Source::WorkshopDisk),
            mods: inspect_item(&item_dir),
            layout: None,
        };
        let entry = &item.mods[0];

        let installed = InstalledManifest::default();
        assert_eq!(
            mod_install_status(&item, entry, &installed),
            ModStatus::New
        );

        let manifest = |records| InstalledManifest {
            format: 1,
            mods: records,
        };
        let record = |time_updated| ModRecord {
            name: "Plain".to_owned(),
            mod_id: None,
            item_id: 1,
            time_updated,
            title: None,
            unsafe_install: false,
        };
        assert_eq!(
            mod_install_status(&item, entry, &manifest(vec![record(10)])),
            ModStatus::UpToDate
        );

        // The store captures a newer version.
        item.time_updated = 20;
        assert_eq!(
            mod_install_status(&item, entry, &manifest(vec![record(10)])),
            ModStatus::UpdateAvailable { installed: 10 }
        );

        // A record newer than the store (a downgrade in the store) is
        // not an update prompt.
        assert_eq!(
            mod_install_status(&item, entry, &manifest(vec![record(30)])),
            ModStatus::UpToDate
        );

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn installs_individually_selected_mods_of_an_item() -> Result<()> {
        let (store, instance, item, base) = fixture("select")?;

        // The item bundles five mods; pick two, one of them unsafely
        // (Bounded Mod's declared range excludes 42.22.2).
        let selected: Vec<SelectedMod> = item
            .mods
            .iter()
            .filter(|entry| entry.name == "Hybrid Mod" || entry.name == "Bounded Mod")
            .map(|entry| SelectedMod {
                item: &item,
                entry,
                unsafe_install: entry.name == "Bounded Mod",
            })
            .collect();
        let outcome = store.instance_install_selected(&instance, &selected, noop_progress)?;

        let names: Vec<&str> = outcome
            .mods
            .iter()
            .map(|m| m.name.as_str())
            .collect();
        assert_eq!(names, vec!["Bounded Mod", "Hybrid Mod"]);
        assert!(instance
            .profile_dir
            .join("mods/Hybrid Mod/42.20/media/new.lua")
            .is_file());
        let rewritten = fs::read_to_string(
            instance
                .profile_dir
                .join("mods/Bounded Mod/42.16/mod.info"),
        )?;
        assert!(rewritten.contains("versionMin=42.22.2"));
        assert_eq!(
            outcome.activation.mod_ids,
            vec!["BM", "HM"]
        );

        fs::remove_dir_all(&base)?;
        Ok(())
    }
}
