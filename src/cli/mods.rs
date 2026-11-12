use std::path::Path;

use clap::Subcommand;

use snapoid::auth;
use snapoid::remote;
use snapoid::scanner::workshop;
use snapoid::steam;
use snapoid::steamcmd;
use snapoid::store::{self, Store};
use snapoid::workshop as workshop_dl;

use crate::report;
use crate::ui;

#[derive(Subcommand)]
pub enum ModsCommand {
    /// Workshop items: list what is installed, save copies into the store
    Workshop {
        #[command(subcommand)]
        command: Option<WorkshopCommand>,
    },
}

#[derive(Subcommand)]
pub enum WorkshopCommand {
    /// Save every installed workshop item into the store
    Save {
        /// Copy from the local workshop dir instead of downloading the
        /// latest versions with steamcmd
        #[arg(long)]
        disk: bool,
    },
}

pub fn run(command: ModsCommand) -> anyhow::Result<()> {
    match command {
        ModsCommand::Workshop { command } => match command {
            None => list(),
            Some(WorkshopCommand::Save { disk }) => save(disk),
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
    snapoid::info!("found {} workshop item(s)", items.len());
    report::print_scan(&items);
    Ok(())
}

/// Save every installed workshop item into the store: `--disk` copies
/// the local workshop dirs, otherwise steamcmd downloads the latest
/// version of each item.
fn save(disk: bool) -> anyhow::Result<()> {
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
    if disk {
        for item in &items {
            // Steam's timeupdated is the freshness marker: a stored copy
            // at least as recent as the workshop dir is up to date.
            if let Some(stored) = store.stored_mod(item.id)?
                && item.time_updated > 0
                && stored.time_updated >= item.time_updated
            {
                snapoid::info!("mod {} already stored; skipping", item.id);
                skipped += 1;
                continue;
            }
            let from = paths.workshop_content.join(item.id.to_string());
            let stored =
                store.save_mod_copy(&from, item.id, item.time_updated, ui::progress_factory)?;
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
                snapoid::warn!(
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
        let times = acf_dir.map(workshop_dl::time_updated_all).unwrap_or_default();
        for id in &stale {
            let from = content.join(id.to_string());
            if !dir_has_files(&from) {
                snapoid::warn!("steamcmd did not download item {}; skipping", id);
                continue;
            }
            let stored = store.adopt_mod_dir(&from, *id, times.get(id).copied().unwrap_or(0))?;
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
