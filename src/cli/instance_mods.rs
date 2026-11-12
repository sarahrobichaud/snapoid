use std::fs;
use std::path::Path;

use clap::Subcommand;

use snapoid::fmt;
use snapoid::store::StoredMod;

use super::open_store;
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

/// Show the stored mods as a checklist and confirm the selection. The
/// install step itself is not wired up yet.
fn install(name: Option<String>) -> anyhow::Result<()> {
    let store = open_store()?;
    let name = super::instance::resolve_name(&store, name)?;
    let instance = store.instance(&name)?;
    let mods = store.list_mods()?;
    if mods.is_empty() {
        println!("No mods stored; save some with `snapoid mods workshop save`.");
        return Ok(());
    }

    let items: Vec<String> = mods.iter().map(describe_mod).collect();
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
    println!(
        "\n{} mod{} selected for instance {}:",
        picked.len(),
        if picked.len() == 1 { "" } else { "s" },
        instance.name
    );
    for index in picked {
        println!("  {}", items[index]);
    }
    Ok(())
}

/// One checklist row: workshop item id, the mods it bundles, and when
/// the stored copy was captured.
fn describe_mod(mod_entry: &StoredMod) -> String {
    let names = mod_names(&mod_entry.dir);
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

/// Mod folder names bundled in a stored workshop item.
fn mod_names(item_dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(item_dir.join("mods")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}
