use std::path::{Path, PathBuf};

use clap::Subcommand;

use snapoid::fmt;
use snapoid::pz::compat::{self, Compatibility, Incompatibility};
use snapoid::store::{item_install_status, ItemStatus, StoredMod};

use super::open_store;
use crate::report;
use crate::ui;

#[derive(Subcommand)]
pub enum InstanceModsCommand {
    /// Install stored mods into the instance: pick from the store when
    /// no path is given, or install a .modpack file
    Install {
        /// Path to a .modpack file (as written by `mods pack`)
        path: Option<PathBuf>,
    },
    /// Pack the instance's installed mods into a .modpack file
    Pack {
        /// Where to write the modpack
        /// (default: <instance>.modpack in the current directory)
        output: Option<PathBuf>,
    },
}

pub fn run(name: Option<String>, command: InstanceModsCommand) -> anyhow::Result<()> {
    match command {
        InstanceModsCommand::Install { path } => match path {
            Some(path) => install_modpack(name, &path),
            None => install(name),
        },
        InstanceModsCommand::Pack { output } => pack(name, output),
    }
}

/// Pack the instance's installed mods into a .modpack zip.
fn pack(name: Option<String>, output: Option<PathBuf>) -> anyhow::Result<()> {
    let store = open_store()?;
    let name = super::instance::resolve_name(&store, name)?;
    let instance = store.instance(&name)?;
    let output =
        output.unwrap_or_else(|| PathBuf::from(format!("{}.modpack", instance.name)));
    let packed = store.instance_pack_mods(&instance, &output, ui::progress_factory)?;
    report::print_packed(&packed);
    Ok(())
}

/// Install a .modpack file into the instance's profile.
fn install_modpack(name: Option<String>, path: &Path) -> anyhow::Result<()> {
    let store = open_store()?;
    let name = super::instance::resolve_name(&store, name)?;
    let instance = store.instance(&name)?;
    let outcome = store.instance_install_modpack(&instance, path, ui::progress_factory)?;
    report::print_installed_mods(&instance.name, &outcome);
    Ok(())
}

/// Pick compatible stored mods to install. The instance's install
/// manifest remembers what is already installed: up-to-date mods are
/// reported and left alone, mods the store has newer captures of are
/// asked about (update or keep), and the rest make the checklist.
fn install(name: Option<String>) -> anyhow::Result<()> {
    let store = open_store()?;
    let name = super::instance::resolve_name(&store, name)?;
    let instance = store.instance(&name)?;
    let mods = store.list_mods()?;
    if mods.is_empty() {
        println!("No mods stored; update some with `snapoid mods update --workshop`.");
        return Ok(());
    }

    // Only items whose bundled mods load on the instance's game
    // version make the checklist; the rest are reported with reasons.
    let manifest = store.instance_installed_manifest(&instance);
    let mut compatible: Vec<(&StoredMod, ItemStatus)> = Vec::new();
    let mut excluded: Vec<(&StoredMod, Incompatibility)> = Vec::new();
    let mut unknown = 0usize;
    for item in &mods {
        match compat::item_compatibility(&item.mods, &instance.version) {
            Compatibility::Compatible => {
                let status = item_install_status(item, &instance.version, &manifest);
                compatible.push((item, status));
            }
            Compatibility::Incompatible(reason) => excluded.push((item, reason)),
            Compatibility::Unknown => unknown += 1,
        }
    }

    if !excluded.is_empty() {
        println!("Not compatible with version {}:", instance.version);
        for (item, reason) in &excluded {
            println!("  {}  {}  — {reason}", item.id, item_label(item));
        }
    }
    if unknown > 0 {
        println!(
            "{unknown} stored item(s) have unknown compatibility and are not shown."
        );
    }
    if !excluded.is_empty() || unknown > 0 {
        println!();
    }

    let up_to_date: Vec<&&StoredMod> = compatible
        .iter()
        .filter(|(_, status)| *status == ItemStatus::UpToDate)
        .map(|(item, _)| item)
        .collect();
    if !up_to_date.is_empty() {
        println!("Already installed at the stored version:");
        for item in &up_to_date {
            println!("  {}  {}", item.id, item_label(item));
        }
        println!();
    }

    // Newer captures in the store: ask per item whether to update or
    // keep the installed version.
    let mut picked_items: Vec<&StoredMod> = Vec::new();
    for (item, status) in &compatible {
        let ItemStatus::UpdateAvailable { installed } = status else {
            continue;
        };
        let options = [
            format!(
                "Update to the stored version (updated {})",
                fmt::format_date_utc(item.time_updated)
            ),
            format!(
                "Keep the installed version (installed {})",
                fmt::format_date_utc(*installed)
            ),
        ];
        let choice = ui::pick(
            &options,
            &format!(
                "{}  {} — a newer version is in the store",
                item.id,
                item_label(item)
            ),
            0,
        )?;
        if choice == 0 {
            picked_items.push(item);
        }
    }

    let installable: Vec<(&StoredMod, ItemStatus)> = compatible
        .iter()
        .filter(|(_, status)| matches!(status, ItemStatus::New | ItemStatus::Partial))
        .map(|(item, status)| (*item, *status))
        .collect();
    if !installable.is_empty() {
        let items: Vec<String> = installable
            .iter()
            .map(|(item, status)| match status {
                ItemStatus::Partial => format!("{}  (partially installed)", describe_mod(item)),
                _ => describe_mod(item),
            })
            .collect();
        let picked = ui::multi_pick(
            &items,
            &format!(
                "Install mods into instance {} (version {}) — space: toggle, enter: confirm",
                instance.name, instance.version
            ),
        )?;
        picked_items.extend(picked.iter().map(|&index| installable[index].0));
    }

    if picked_items.is_empty() {
        println!("\nNothing to install.");
        return Ok(());
    }

    let outcome = store.instance_install_mods(&instance, &picked_items, ui::progress_factory)?;
    report::print_installed_mods(&instance.name, &outcome);
    Ok(())
}

/// A short label for a stored item: its title, else its first bundled
/// mod's name.
fn item_label(item: &StoredMod) -> String {
    item.title
        .clone()
        .or_else(|| {
            item.mods
                .first()
                .map(|m| m.title.clone().unwrap_or_else(|| m.name.clone()))
        })
        .unwrap_or_else(|| "(no mods)".to_owned())
}

/// One checklist row: workshop item id, the mods it bundles (from the
/// stored metadata, not a fresh disk walk), and when the stored copy
/// was captured.
fn describe_mod(mod_entry: &StoredMod) -> String {
    let names: Vec<String> = mod_entry
        .mods
        .iter()
        .map(|m| m.title.clone().unwrap_or_else(|| m.name.clone()))
        .collect();
    let updated = if mod_entry.time_updated == 0 {
        "unknown".to_owned()
    } else {
        fmt::format_date_utc(mod_entry.time_updated)
    };
    format!(
        "{}  {}  updated {}",
        mod_entry.id,
        if names.is_empty() {
            "(no mods)".to_owned()
        } else {
            names.join(", ")
        },
        updated
    )
}
