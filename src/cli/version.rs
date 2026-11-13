use std::path::PathBuf;

use clap::Subcommand;

use crate::auth;
use crate::download::{self, DownloadRequest};
use crate::pz::{self, probe};
use crate::registry;
use crate::steam;
use crate::steamcmd;
use crate::store::{self, Store};

use super::open_store;
use crate::report;
use crate::ui;

#[derive(Subcommand)]
pub enum VersionCommand {
    /// Detect the installed game version (briefly spawns the headless server)
    Detect,
    /// Detect the game version in an arbitrary directory (briefly spawns
    /// the headless server found there)
    Probe {
        /// Directory containing the game files (e.g. a downloaded build)
        game_dir: PathBuf,
    },
    /// Capture the current game files into game/<version>/
    Snapshot,
    /// List stored game versions
    List,
    /// List game versions available for download
    Available,
    /// Download a game version into the store
    Download {
        /// Version to download (see `version available`), or "latest"
        version: String,
        /// Steam account name for steamcmd (default: the account
        /// remembered by `yapzm auth login`)
        #[arg(short, long)]
        login: Option<String>,
        /// Depot ID override (default: the registry entry's depot, else
        /// the Windows depot 108604)
        #[arg(long)]
        depot: Option<u64>,
        /// Download even if the version is already stored
        #[arg(long)]
        force: bool,
    },
    /// Re-download a stored version to repair missing or corrupted files
    Repair {
        /// Stored version to repair (see `version list`)
        version: String,
        /// Steam account name for steamcmd (default: the account
        /// remembered by `yapzm auth login`)
        #[arg(short, long)]
        login: Option<String>,
        /// Depot ID override (default: the registry entry's depot, else
        /// the Windows depot 108604)
        #[arg(long)]
        depot: Option<u64>,
    },
}

pub fn run(command: VersionCommand) -> anyhow::Result<()> {
    match command {
        VersionCommand::Detect => {
            let paths = steam::locate()?;
            let info = match paths.game_dir.as_deref() {
                Some(game_dir) => probe::detect_dir(game_dir)?,
                None => {
                    crate::warn!("no game install found");
                    probe::GameVersionInfo::default()
                }
            };
            report::print_version_info(&info);
        }
        VersionCommand::Probe { game_dir } => {
            let info = probe::detect_dir(&game_dir)?;
            report::print_version_info(&info);
        }
        VersionCommand::Snapshot => {
            let paths = steam::locate()?;
            let Some(game_dir) = paths.game_dir.as_deref() else {
                anyhow::bail!("Project Zomboid install not found; nothing to snapshot");
            };
            let version = probe::detect_dir(game_dir)?;
            let Some(version_str) = version.version.as_deref() else {
                anyhow::bail!("could not detect the installed game version");
            };

            let root = store::locate_root(paths.game_dir.as_deref())?;
            let store = Store::open(&root)?;
            let buildid = pz::appmanifest::buildid_for_install(game_dir, pz::APP_ID);
            let game = store.snapshot_version(
                game_dir,
                version_str,
                version.revision.as_deref(),
                buildid,
                ui::progress_factory,
            )?;
            report::print_stored_game(&game);
        }
        VersionCommand::List => {
            let store = open_store()?;
            let games = store.list_games()?;
            report::print_games(&games);
        }
        VersionCommand::Available => {
            let registry = registry::load()?;
            report::print_registry(&registry);
        }
        VersionCommand::Download {
            version,
            login,
            depot,
            force,
        } => {
            let program = steamcmd::locate()?;
            let paths = steam::locate().ok();
            let root = store::locate_root(paths.as_ref().and_then(|p| p.game_dir.as_deref()))?;
            let store = Store::open(&root)?;
            let cmd = steamcmd::Process::new(program);
            // Fall back to the account remembered by `yapzm auth login`.
            let login = login.or_else(|| auth::load(&root));
            let downloaded = download::download(
                &store,
                &cmd,
                &DownloadRequest {
                    version,
                    login,
                    depot,
                    force,
                    live_game_dir: paths.and_then(|p| p.game_dir),
                },
            )?;
            report::print_downloaded(&downloaded);
        }
        VersionCommand::Repair {
            version,
            login,
            depot,
        } => {
            let program = steamcmd::locate()?;
            let paths = steam::locate().ok();
            let root = store::locate_root(paths.as_ref().and_then(|p| p.game_dir.as_deref()))?;
            let store = Store::open(&root)?;
            let cmd = steamcmd::Process::new(program);
            // Fall back to the account remembered by `yapzm auth login`.
            let login = login.or_else(|| auth::load(&root));
            let repaired = download::repair(
                &store,
                &cmd,
                &DownloadRequest {
                    version,
                    login,
                    depot,
                    force: true,
                    live_game_dir: paths.and_then(|p| p.game_dir),
                },
            )?;
            report::print_downloaded(&repaired);
        }
    }
    Ok(())
}
