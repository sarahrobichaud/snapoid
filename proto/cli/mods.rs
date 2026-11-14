use std::collections::HashMap;
use std::path::Path;

use clap::{ArgGroup, Subcommand};

use yapzm_lib::auth;
use yapzm_lib::remote;
use yapzm_lib::scanner::workshop;
use yapzm_lib::steam;
use yapzm_lib::steamcmd;
use yapzm_lib::store::{self, Source, Store};
use yapzm_lib::workshop as workshop_dl;

use crate::report;
use crate::ui;

#[derive(Subcommand)]
pub enum ModsCommand {
    /// List mods from a source
    #[command(group = ArgGroup::new("list_source").required(true).multiple(false).args(&["workshop", "store"]))]
    List {
        /// The local Steam workshop content
        #[arg(long)]
        workshop: bool,
        /// The local snapoid store
        #[arg(long)]
        store: bool,
    },
    /// Update the store's copies from a source
    #[command(group = ArgGroup::new("update_source").required(true).multiple(false).args(&["workshop"]))]
    Update {
        /// The local Steam workshop content
        #[arg(long)]
        workshop: bool,
        /// Copy the local workshop dirs into the store instead of
        /// downloading the latest versions with steamcmd
        #[arg(long)]
        copy: bool,
    },
    /// Developer diagnostics for the mod store (dev builds only)
    #[cfg(debug_assertions)]
    Debug {
        #[command(subcommand)]
        command: DebugCommand,
    },
}

#[derive(Subcommand)]
#[cfg(debug_assertions)]
pub enum DebugCommand {
    /// Rebuild every stored mod's mod.json sidecar from the files
    /// already in the store, without downloading anything
    RepairMeta,
}

pub fn run(command: ModsCommand) -> anyhow::Result<()> {
    match command {
        // The source groups make a source flag required; workshop and
        // store are the only sources so far.
        ModsCommand::List { workshop: _, store } => {
            if store {
                list_store()
            } else {
                list()
            }
        }
        ModsCommand::Update { workshop: _, copy } => update(copy),
        #[cfg(debug_assertions)]
        ModsCommand::Debug { command } => match command {
            DebugCommand::RepairMeta => repair_meta(),
        },
    }
}

/// List installed workshop items and the mods they contain.
fn list() -> anyhow::Result<()> {
    let paths = steam::locate()?;
    let items = workshop::scan(&paths)?;
    if items.is_empty() {
        println!("No workshop items installed for Project Zomboid.");
        return Ok(());
    }
    yapzm_lib::info!("found {} workshop item(s)", items.len());
    report::print_scan(&items);
    Ok(())
}

/// List the workshop items in the local store, from the metadata
/// captured at save time.
fn list_store() -> anyhow::Result<()> {
    let store = super::open_store()?;
    let items = store.list_mods()?;
    if items.is_empty() {
        println!("No mods stored; update some with `snapoid mods update --workshop`.");
        return Ok(());
    }
    yapzm_lib::info!("{} workshop item(s) stored", items.len());
    report::print_stored_mods(&items);
    Ok(())
}

/// Rebuild the stored mods' sidecars from the stored files and show
/// the result.
#[cfg(debug_assertions)]
fn repair_meta() -> anyhow::Result<()> {
    let store = super::open_store()?;
    let repaired = store.repair_mod_meta()?;
    if repaired.is_empty() {
        println!("No mods stored; nothing to repair.");
        return Ok(());
    }
    yapzm_lib::info!("rebuilt {} mod manifest(s)", repaired.len());
    report::print_stored_mods(&repaired);
    Ok(())
}

/// Update the store's workshop copies: `--copy` copies the local
/// workshop dirs, otherwise steamcmd downloads the latest version of
/// each item.
fn update(copy: bool) -> anyhow::Result<()> {
    let paths = steam::locate()?;
    let items = workshop::scan(&paths)?;
    if items.is_empty() {
        println!("No workshop items installed for Project Zomboid.");
        return Ok(());
    }

    let root = store::locate_root(paths.game_dir.as_deref())?;
    let store = Store::open(&root)?;

    let mut saved = 0usize;
    let mut skipped = 0usize;
    if copy {
        for item in &items {
            // Steam's timeupdated is the freshness marker: a stored copy
            // at least as recent as the workshop dir is up to date.
            if let Some(stored) = store.stored_mod(item.id)?
                && item.time_updated > 0
                && stored.time_updated >= item.time_updated
            {
                yapzm_lib::info!("mod {} already stored; skipping", item.id);
                skipped += 1;
                continue;
            }
            let from = paths.workshop_content.join(item.id.to_string());
            let stored = store.save_mod_copy(
                &from,
                item.id,
                item.time_updated,
                Source::WorkshopDisk,
                None,
                ui::progress_factory,
            )?;
            report::print_saved_mod(&stored);
            saved += 1;
        }
    } else {
        let ids: Vec<u64> = items.iter().map(|item| item.id).collect();

        // Best-effort freshness check against the Steam Web API: items
        // already stored at the upstream version are skipped, so repeat
        // saves only download what changed (and launch no steamcmd at
        // all when everything is current).
        let remote = match remote::web::fetch_remote(&ids) {
            Ok(items) => items,
            Err(error) => {
                yapzm_lib::warn!(
                    "could not query the workshop ({error:#}); downloading everything"
                );
                Vec::new()
            }
        };
        let stale: Vec<u64> = items
            .iter()
            .filter(|item| {
                let Some(upstream) = remote.iter().find(|r| r.id == item.id) else {
                    return true;
                };
                if upstream.time_updated == 0 {
                    return true;
                }
                match store.stored_mod(item.id) {
                    Ok(Some(stored)) => stored.time_updated < upstream.time_updated,
                    _ => true,
                }
            })
            .map(|item| item.id)
            .collect();
        skipped = items.len() - stale.len();

        // Titles come from the same query as the freshness check; items
        // the API could not report save with an unknown title.
        let titles: HashMap<u64, String> = remote
            .iter()
            .filter_map(|item| item.title.clone().map(|title| (item.id, title)))
            .collect();

        if stale.is_empty() {
            println!("\n{} mod(s) saved, {} already up to date", saved, skipped);
            return Ok(());
        }

        let program = steamcmd::locate()?;
        let cmd = steamcmd::Process::new(program);
        // Workshop items download anonymously by default; the account
        // remembered by `snapoid auth login` is used when present, since
        // some items need a subscription.
        let login = auth::load(&root).unwrap_or_else(|| "anonymous".to_owned());
        let content = workshop_dl::download_items(&cmd, &login, &stale)?;
        // The workshop ACF steamcmd maintains sits next to the content
        // dir's parent (.../steamapps/workshop/).
        let acf_dir = content.parent().and_then(Path::parent);
        let times = acf_dir
            .map(workshop_dl::time_updated_all)
            .unwrap_or_default();
        for id in &stale {
            let from = content.join(id.to_string());
            if !dir_has_files(&from) {
                yapzm_lib::warn!("steamcmd did not download item {}; skipping", id);
                continue;
            }
            let stored = store.adopt_mod_dir(
                &from,
                *id,
                times.get(id).copied().unwrap_or(0),
                Source::Steamcmd,
                titles.get(id).map(String::as_str),
            )?;
            report::print_saved_mod(&stored);
            saved += 1;
        }
    }

    println!("\n{} mod(s) saved, {} already up to date", saved, skipped);
    Ok(())
}

/// True when `dir` exists and holds at least one entry.
fn dir_has_files(dir: &Path) -> bool {
    dir.read_dir()
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}
