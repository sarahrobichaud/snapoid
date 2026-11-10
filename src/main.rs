//! Component 1: CLI layer.
//!
//! Subcommands map 1:1 onto the system components; each handler is a thin
//! composition of module entry points.

// Scaffolds are not wired up yet; remove as modules get implemented.
#![allow(dead_code)]

mod api;
mod deploy;
mod fmt;
mod parser;
mod scanner;
mod store;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "snapoid",
    version,
    about = "Version manager for Project Zomboid workshop mods",
    long_about = None,
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List installed workshop items and the mods they contain
    Scan,
    /// Capture the current state of workshop items into the local store
    Snapshot {
        /// Snapshot only this workshop item (default: all installed)
        item_id: Option<u64>,
    },
    /// Restore a stored snapshot into the workshop folder
    Restore {
        item_id: u64,
        /// Specific snapshot to restore (default: newest)
        time_updated: Option<u64>,
    },
    /// Check the Steam Web API for upstream updates
    Check,
    /// Show the resolved Steam and Project Zomboid paths
    Paths,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Scan => {
            let paths = scanner::steam::locate()?;
            let items = scanner::workshop::scan(&paths)?;
            if items.is_empty() {
                println!("No workshop items installed for Project Zomboid.");
                return Ok(());
            }

            let mut total_mods = 0;
            for item in &items {
                total_mods += item.mods.len();
                let updated = if item.time_updated == 0 {
                    "unknown".to_string()
                } else {
                    fmt::format_date_utc(item.time_updated)
                };
                println!("\n{}  updated {}", item.id, updated);

                let name_width = item
                    .mods
                    .iter()
                    .map(|m| m.name.chars().count())
                    .max()
                    .unwrap_or(0);
                if item.mods.is_empty() {
                    println!("  (no mods found)");
                }
                for m in &item.mods {
                    let range = match (&m.version_min, &m.version_max) {
                        (Some(min), Some(max)) if min == max => min.clone(),
                        (Some(min), Some(max)) => format!("{min} - {max}"),
                        (Some(min), None) => format!("{min}+"),
                        (None, Some(max)) => format!("- {max}"),
                        (None, None) => "-".to_string(),
                    };
                    println!(
                        "  {:name_width$}  {}",
                        m.name,
                        range,
                        name_width = name_width
                    );
                }
            }

            println!(
                "\n{} workshop item{}, {} mod{}",
                items.len(),
                if items.len() == 1 { "" } else { "s" },
                total_mods,
                if total_mods == 1 { "" } else { "s" }
            );
        }
        Commands::Snapshot { item_id } => {
            let store = store::Store::open()?;
            match item_id {
                Some(id) => {
                    store.snapshot(id)?;
                }
                None => todo!("snapshot every installed item"),
            }
        }
        Commands::Restore {
            item_id,
            time_updated,
        } => {
            let paths = scanner::steam::locate()?;
            let store = store::Store::open()?;
            deploy::restore(&paths, &store, item_id, time_updated)?;
        }
        Commands::Check => {
            todo!("collect installed ids, query api::fetch_remote, diff against the store")
        }
        Commands::Paths => {
            let paths = scanner::steam::locate()?;
            println!("Steam root:       {}", paths.steam_dir.display());
            println!(
                "Workshop content: {}{}",
                paths.workshop_content.display(),
                if paths.workshop_content.is_dir() {
                    ""
                } else {
                    " (no content downloaded)"
                }
            );
            println!(
                "Game install:     {}",
                optional_path(paths.game_dir.as_deref())
            );
            println!(
                "Zomboid user dir: {}",
                optional_path(paths.zomboid_user_dir.as_deref())
            );
        }
    }

    Ok(())
}

/// Format an optional path for display.
fn optional_path(path: Option<&std::path::Path>) -> String {
    path.map(|p| p.display().to_string())
        .unwrap_or_else(|| "(not found)".to_string())
}
