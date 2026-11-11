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
    }
    Ok(())
}
