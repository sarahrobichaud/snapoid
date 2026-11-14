use std::path::{Path, PathBuf};

use clap::Subcommand;

use yapzm_lib::fmt;
use yapzm_lib::pz::compat::{self, Compatibility, Incompatibility};
use yapzm_lib::pz::item::ModEntry;
use yapzm_lib::store::{ModStatus, SelectedMod, StoredMod, mod_install_status};

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
    let output = output.unwrap_or_else(|| PathBuf::from(format!("{}.modpack", instance.name)));
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

/// One list row: a bundled mod of a stored item, with what the
/// install manifest says about it. Items bundle several mods; the
/// checklist offers them individually.
struct Row<'a> {
    item: &'a StoredMod,
    entry: &'a ModEntry,
    status: ModStatus,
}

/// Pick compatible stored mods to install, one bundled mod at a time.
/// The instance's install manifest remembers what is already
/// installed: up-to-date mods are reported and left alone, mods the
/// store has newer captures of are asked about (update or keep), and
/// the rest make the checklist.
fn install(name: Option<String>) -> anyhow::Result<()> {
    let store = open_store()?;
    let name = super::instance::resolve_name(&store, name)?;
    let instance = store.instance(&name)?;
    let mods = store.list_mods()?;
    if mods.is_empty() {
        println!("No mods stored; update some with `snapoid mods update --workshop`.");
        return Ok(());
    }

    // Only bundled mods that load on the instance's game version make
    // the checklist; items with none are reported with reasons, or
    // offered mod-by-mod for an unsafe install.
    let manifest = store.instance_installed_manifest(&instance);
    let mut rows: Vec<Row> = Vec::new();
    let mut excluded: Vec<(&StoredMod, Incompatibility)> = Vec::new();
    let mut unsafe_rows: Vec<Row> = Vec::new();
    let mut unknown = 0usize;
    for item in &mods {
        match compat::item_compatibility(&item.mods, &instance.version) {
            Compatibility::Compatible => {
                for entry in &item.mods {
                    if matches!(
                        compat::mod_compatibility(entry, &instance.version),
                        Compatibility::Compatible
                    ) {
                        let status = mod_install_status(item, entry, &manifest);
                        rows.push(Row {
                            item,
                            entry,
                            status,
                        });
                    }
                }
            }
            Compatibility::Incompatible(reason) => {
                let fixable = unsafe_rows.len();
                for entry in &item.mods {
                    if matches!(
                        compat::mod_compatibility(entry, &instance.version),
                        Compatibility::Incompatible(Incompatibility::DeclaredRange { .. })
                    ) {
                        unsafe_rows.push(Row {
                            item,
                            entry,
                            status: ModStatus::New,
                        });
                    }
                }
                if unsafe_rows.len() == fixable {
                    excluded.push((item, reason));
                }
            }
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
        println!("{unknown} stored item(s) have unknown compatibility and are not shown.");
    }
    if !excluded.is_empty() || unknown > 0 {
        println!();
    }

    let up_to_date: Vec<&Row> = rows
        .iter()
        .filter(|r| r.status == ModStatus::UpToDate)
        .collect();
    if !up_to_date.is_empty() {
        println!("Already installed at the stored version:");
        for row in &up_to_date {
            println!("  {}", describe_row(row.item, row.entry));
        }
        println!();
    }

    let mut selected: Vec<SelectedMod> = Vec::new();

    // Newer captures in the store: ask per mod whether to update or
    // keep the installed version.
    for row in rows
        .iter()
        .filter(|r| matches!(r.status, ModStatus::UpdateAvailable { .. }))
    {
        let ModStatus::UpdateAvailable { installed } = row.status else {
            unreachable!("filtered above")
        };
        let options = [
            format!(
                "Update to the stored version (updated {})",
                fmt::format_date_utc(row.item.time_updated)
            ),
            format!(
                "Keep the installed version (installed {})",
                fmt::format_date_utc(installed)
            ),
        ];
        let choice = ui::pick(&options, &describe_row(row.item, row.entry), 0)?;
        if choice == 0 {
            selected.push(SelectedMod {
                item: row.item,
                entry: row.entry,
                unsafe_install: false,
            });
        }
    }

    // Never-installed mods make the checklist, one row per mod.
    let installable: Vec<&Row> = rows.iter().filter(|r| r.status == ModStatus::New).collect();
    if !installable.is_empty() {
        let items: Vec<String> = installable
            .iter()
            .map(|row| describe_row(row.item, row.entry))
            .collect();
        let picked = ui::multi_pick(
            &items,
            &format!(
                "Install mods into instance {} (version {}) — space: toggle, enter: confirm",
                instance.name, instance.version
            ),
        )?;
        selected.extend(picked.iter().map(|&index| SelectedMod {
            item: installable[index].item,
            entry: installable[index].entry,
            unsafe_install: false,
        }));
    }

    // Mods the game refuses only for their declared version range can
    // be forced in: their manifest is rewritten to the instance's
    // version and the display name tagged.
    if !unsafe_rows.is_empty() {
        println!(
            "These mods declare a version range that excludes version {}. \
             Unsafe mode installs them anyway: the manifest the game reads is \
             rewritten to that version and the name tagged [unsafe]:",
            instance.version
        );
        let items: Vec<String> = unsafe_rows
            .iter()
            .map(|row| describe_row(row.item, row.entry))
            .collect();
        let picked = ui::multi_pick(
            &items,
            "Install in unsafe mode — space: toggle, enter: confirm",
        )?;
        selected.extend(picked.iter().map(|&index| SelectedMod {
            item: unsafe_rows[index].item,
            entry: unsafe_rows[index].entry,
            unsafe_install: true,
        }));
        println!();
    }

    if selected.is_empty() {
        println!("\nNothing to install.");
        return Ok(());
    }

    let outcome = store.instance_install_selected(&instance, &selected, ui::progress_factory)?;
    report::print_installed_mods(&instance.name, &outcome);
    Ok(())
}

/// One checklist row: the item a mod is bundled in, the mod itself,
/// its Mod ID, and when the stored copy was captured.
fn describe_row(item: &StoredMod, entry: &ModEntry) -> String {
    let name = entry.title.clone().unwrap_or_else(|| entry.name.clone());
    let updated = if item.time_updated == 0 {
        "unknown".to_owned()
    } else {
        fmt::format_date_utc(item.time_updated)
    };
    match &entry.mod_id {
        Some(id) => format!("{}  {}  (id={id})  updated {}", item.id, name, updated),
        None => format!("{}  {}  updated {}", item.id, name, updated),
    }
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
