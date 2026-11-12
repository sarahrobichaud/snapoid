use clap::Subcommand;

use snapoid::fmt;
use snapoid::pz::compat::{self, Compatibility, Incompatibility};
use snapoid::store::StoredMod;

use super::open_store;
use crate::report;
use crate::ui;

#[derive(Subcommand)]
pub enum InstanceModsCommand {
    /// Pick stored mods to install into the instance
    Install,
}

pub fn run(name: Option<String>, command: InstanceModsCommand) -> anyhow::Result<()> {
    match command {
        InstanceModsCommand::Install => install(name),
    }
}

/// Show the compatible stored mods as a checklist and confirm the
/// selection. The install step itself is not wired up yet.
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
    let mut compatible: Vec<&StoredMod> = Vec::new();
    let mut excluded: Vec<(&StoredMod, Incompatibility)> = Vec::new();
    let mut unknown = 0usize;
    for item in &mods {
        match compat::item_compatibility(&item.mods, &instance.version) {
            Compatibility::Compatible => compatible.push(item),
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

    if compatible.is_empty() {
        println!(
            "No stored mods are compatible with version {}.",
            instance.version
        );
        return Ok(());
    }

    let items: Vec<String> = compatible.iter().map(|m| describe_mod(m)).collect();
    let picked = ui::multi_pick(
        &items,
        &format!(
            "Install mods into instance {} (version {}) — space: toggle, enter: confirm",
            instance.name, instance.version
        ),
    )?;

    if picked.is_empty() {
        println!("\nNo mods selected; nothing to install.");
        return Ok(());
    }

    let picked_items: Vec<&StoredMod> = picked.iter().map(|&index| compatible[index]).collect();
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
