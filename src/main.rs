//! Component 1: CLI layer.
//!
//! Subcommands map 1:1 onto the system components; each handler is a thin
//! composition of module entry points.

// Scaffolds are not wired up yet; remove as modules get implemented.
#![allow(dead_code)]

mod api;
mod deploy;
mod fmt;
mod logger;
mod parser;
mod scanner;
mod store;

use anyhow::Context;
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
    /// Manage stored game versions (game files per version)
    Version {
        #[command(subcommand)]
        command: VersionCommand,
    },
    /// Manage game instances
    Instance {
        #[command(subcommand)]
        command: InstanceCommand,
    },
    /// Check the Steam Web API for upstream updates
    Check,
    /// Show the resolved Steam and Project Zomboid paths
    Paths,
}

#[derive(Subcommand)]
enum VersionCommand {
    /// Detect the installed game version (briefly spawns the headless server)
    Detect,
    /// Capture the current game files into game/<version>/
    Snapshot,
    /// List stored game versions
    List,
}

#[derive(Subcommand)]
enum InstanceCommand {
    /// Create instance/<name> with a game folder linked to a stored version
    New {
        /// Instance name
        name: String,
        /// Game version to link (default: the currently installed version)
        #[arg(short, long)]
        version: Option<String>,
    },
    /// List stored game instances
    List,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    logger::init(logger::Level::Info);

    match cli.command {
        Commands::Scan => {
            let paths = scanner::steam::locate()?;
            let items = scanner::workshop::scan(&paths)?;
            if items.is_empty() {
                println!("No workshop items installed for Project Zomboid.");
                return Ok(());
            }

            info!("found {} workshop item(s)", items.len());

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
            let paths = scanner::steam::locate()?;
            let store = store::Store::open(paths.game_dir.as_deref())?;
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
            let store = store::Store::open(paths.game_dir.as_deref())?;
            deploy::restore(&paths, &store, item_id, time_updated)?;
        }
        Commands::Instance { command } => match command {
            InstanceCommand::New { name, version } => {
                let paths = scanner::steam::locate().ok();
                let version = match version {
                    Some(v) => v,
                    None => {
                        let paths = paths
                            .as_ref()
                            .context("Steam install not found; pass --version")?;
                        let info = scanner::game_version::detect(paths)?;
                        info.version.context(
                            "could not detect the installed game version; pass --version",
                        )?
                    }
                };

                let store = store::Store::open(paths.as_ref().and_then(|p| p.game_dir.as_deref()))?;
                let instance = store.instance_new(&name, &version)?;
                println!(
                    "instance {} created (game version {})",
                    instance.name, instance.version
                );
                println!("  path: {}", instance.dir.display());
                println!("  game: {}", instance.game_dir.display());
            }
            InstanceCommand::List => {
                // The store location depends on the game drive under WSL,
                // but listing should still work without a Steam install.
                let game_dir = scanner::steam::locate().ok().and_then(|p| p.game_dir);
                let store = store::Store::open(game_dir.as_deref())?;
                let instances = store.list_instances()?;
                if instances.is_empty() {
                    println!("No game instances stored.");
                    return Ok(());
                }
                for instance in &instances {
                    let captured = if instance.created == 0 {
                        "unknown".to_string()
                    } else {
                        fmt::format_date_utc(instance.created)
                    };
                    let game = if instance.game_dir.is_dir() {
                        "ok"
                    } else {
                        "BROKEN LINK"
                    };
                    println!(
                        "{}  (version {})  captured {}  game link {}",
                        instance.name, instance.version, captured, game
                    );
                    println!("  {}", instance.dir.display());
                }
            }
        },
        Commands::Check => {
            todo!("collect installed ids, query api::fetch_remote, diff against the store")
        }
        Commands::Version { command } => match command {
            VersionCommand::Detect => {
                let paths = scanner::steam::locate()?;
                let info = scanner::game_version::detect(&paths)?;
                println!("version:  {}", info.version.as_deref().unwrap_or("unknown"));
                println!(
                    "revision: {}",
                    info.revision.as_deref().unwrap_or("unknown")
                );
                println!(
                    "source:   {}",
                    info.source
                        .map(|source| format!("{source:?}"))
                        .unwrap_or_else(|| "none".to_owned())
                );
            }
            VersionCommand::Snapshot => {
                let paths = scanner::steam::locate()?;
                let Some(game_dir) = paths.game_dir.as_deref() else {
                    anyhow::bail!("Project Zomboid install not found; nothing to snapshot");
                };
                let version = scanner::game_version::detect(&paths)?;
                let Some(version_str) = version.version.as_deref() else {
                    anyhow::bail!("could not detect the installed game version");
                };

                let store = store::Store::open(paths.game_dir.as_deref())?;
                let game =
                    store.snapshot_version(game_dir, version_str, version.revision.as_deref())?;
                println!("game version {} captured", game.version);
                if let Some(revision) = &game.revision {
                    println!("  revision: {revision}");
                }
                println!("  path:     {}", game.dir.display());
            }
            VersionCommand::List => {
                // The store location depends on the game drive under WSL,
                // but listing should still work without a Steam install.
                let game_dir = scanner::steam::locate().ok().and_then(|p| p.game_dir);
                let store = store::Store::open(game_dir.as_deref())?;
                let games = store.list_games()?;
                if games.is_empty() {
                    println!("No game versions stored.");
                    return Ok(());
                }
                for game in &games {
                    let captured = if game.created == 0 {
                        "unknown".to_string()
                    } else {
                        fmt::format_date_utc(game.created)
                    };
                    println!(
                        "{}  captured {}  {}",
                        game.version,
                        captured,
                        game.dir.display()
                    );
                }
            }
        },
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
            let version = scanner::game_version::detect(&paths)?;
            println!("Game version:     {}", describe_version(&version));
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

/// Format detected game version info for display.
fn describe_version(info: &scanner::game_version::GameVersionInfo) -> String {
    let version = info.version.as_deref().unwrap_or("unknown");
    match (&info.revision, info.source) {
        (Some(revision), Some(source)) => {
            format!("{version} (revision {revision}, from {source:?})")
        }
        (Some(revision), None) => format!("{version} (revision {revision})"),
        _ => version.to_owned(),
    }
}
