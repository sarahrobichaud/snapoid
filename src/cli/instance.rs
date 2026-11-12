use clap::Subcommand;

use super::open_store;
use crate::report;
use crate::ui;

#[derive(Subcommand)]
pub enum InstanceCommand {
    /// Create instance/<name> with a game folder linked to a stored version
    New {
        /// Instance name
        name: String,
        /// Game version to link (interactive picker when omitted)
        #[arg(short, long)]
        version: Option<String>,
    },
    /// List stored game instances
    List,
    /// Launch the game for an instance, with its own private profile
    Launch {
        /// Instance name
        name: String,
        /// Launch the dedicated server instead of the client
        #[arg(long)]
        server: bool,
        /// Disable Steam integration
        #[arg(long)]
        nosteam: bool,
        /// Server admin password; on the server's first launch it
        /// defaults to 'snapoid', passing this changes it
        #[arg(long)]
        admin_password: Option<String>,
    },
    /// Reset an instance profile to fresh (deletes saves, settings, mods)
    Reset { name: String },
}

pub fn run(command: InstanceCommand) -> anyhow::Result<()> {
    match command {
        InstanceCommand::New { name, version } => {
            let store = open_store()?;
            let games = store.list_games()?;
            let version = match version {
                Some(version) => version,
                None => {
                    if games.is_empty() {
                        anyhow::bail!(
                            "no game versions stored; download one with \
                             `snapoid version download` or capture the install with \
                             `snapoid version snapshot`"
                        );
                    }
                    let items: Vec<String> = games
                        .iter()
                        .map(|game| {
                            let build = game
                                .buildid
                                .map(|buildid| format!("  build {buildid}"))
                                .unwrap_or_default();
                            format!("{}{build}", game.version)
                        })
                        .collect();
                    let index = ui::pick(&items, "Pick version", games.len() - 1)?;
                    games[index].version.clone()
                }
            };
            let instance = store.instance_new(&name, &version)?;
            report::print_instance(&instance);
        }
        InstanceCommand::List => {
            let store = open_store()?;
            let instances = store.list_instances()?;
            report::print_instances(&instances);
        }
        InstanceCommand::Launch {
            name,
            server,
            nosteam,
            admin_password,
        } => {
            let store = open_store()?;
            let instance = store.instance(&name)?;
            let options = snapoid::launch::LaunchOptions {
                server,
                nosteam,
                admin_password,
            };
            let plan = snapoid::launch::plan(&instance, &options)?;
            report::print_launched(&instance);
            let status = snapoid::launch::run(&plan)?;
            report::print_exited(&instance, &status);
        }
        InstanceCommand::Reset { name } => {
            let store = open_store()?;
            let instance = store.instance_reset(&name)?;
            report::print_reset(&instance);
        }
    }
    Ok(())
}
