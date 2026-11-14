use clap::Subcommand;

use yapzm_lib::store::Store;

use super::instance_mods;
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
    /// Launch the game for the instance, with its own private profile
    Launch {
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
        /// Pass `-debug` (enables the game's debug log channels, e.g. the
        /// checksum dumps read from debuglog.cfg / debuglog-server.cfg)
        #[arg(long)]
        debug: bool,
    },
    /// Reset the instance profile to fresh (deletes saves, settings, mods)
    Reset,
    /// Manage the stored mods of the instance
    Mods {
        #[command(subcommand)]
        command: instance_mods::InstanceModsCommand,
    },
}

pub fn run(name: Option<String>, command: Option<InstanceCommand>) -> anyhow::Result<()> {
    match command {
        Some(InstanceCommand::New { name, version }) => {
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
        Some(InstanceCommand::List) => {
            let store = open_store()?;
            let instances = store.list_instances()?;
            report::print_instances(&instances);
        }
        Some(InstanceCommand::Launch {
            server,
            nosteam,
            admin_password,
            debug,
        }) => {
            let store = open_store()?;
            let name = resolve_name(&store, name)?;
            let instance = store.instance(&name)?;
            let options = yapzm_lib::launch::LaunchOptions {
                server,
                nosteam,
                admin_password,
                debug,
            };
            let plan = yapzm_lib::launch::plan(&instance, &options)?;
            report::print_launched(&instance);
            let status = yapzm_lib::launch::run(&plan)?;
            report::print_exited(&instance, &status);
        }
        Some(InstanceCommand::Reset) => {
            let store = open_store()?;
            let name = resolve_name(&store, name)?;
            let instance = store.instance_reset(&name)?;
            report::print_reset(&instance);
        }
        Some(InstanceCommand::Mods { command }) => instance_mods::run(name, command)?,
        None => match name {
            // `instance <name>`: show the instance's details.
            Some(name) => {
                let store = open_store()?;
                let instance = store.instance(&name)?;
                report::print_instances(std::slice::from_ref(&instance));
            }
            None => anyhow::bail!(
                "nothing to do; try `snapoid instance list`, \
                 `snapoid instance new <name>`, or `snapoid instance <name> launch`"
            ),
        },
    }
    Ok(())
}

/// Resolve the instance to act on: the explicit name, else an
/// interactive picker.
pub(crate) fn resolve_name(store: &Store, name: Option<String>) -> anyhow::Result<String> {
    match name {
        Some(name) => Ok(name),
        None => pick_instance(store),
    }
}

/// Interactive instance picker, used when the name is omitted.
fn pick_instance(store: &Store) -> anyhow::Result<String> {
    let instances = store.list_instances()?;
    if instances.is_empty() {
        anyhow::bail!("no game instances stored; create one with `snapoid instance new`");
    }
    let items: Vec<String> = instances
        .iter()
        .map(|instance| format!("{}  (version {})", instance.name, instance.version))
        .collect();
    let index = ui::pick(&items, "Pick instance", 0)?;
    Ok(instances[index].name.clone())
}
