//! CLI layer: argument definitions and thin handlers that compose library
//! entry points.

mod auth;
mod instance;
mod instance_mods;
mod mods;
mod paths;
mod steamcmd;
mod version;

use clap::{Parser, Subcommand};

use snapoid::logger;

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
    /// Manage workshop mods
    Mods {
        #[command(subcommand)]
        command: mods::ModsCommand,
    },
    /// Manage stored game versions (game files per version)
    Version {
        #[command(subcommand)]
        command: version::VersionCommand,
    },
    /// Manage game instances
    Instance {
        /// Instance name, when running a per-instance action
        #[arg(value_name = "NAME")]
        name: Option<String>,
        #[command(subcommand)]
        command: Option<instance::InstanceCommand>,
    },
    /// Manage the remembered steamcmd account
    Auth {
        #[command(subcommand)]
        command: auth::AuthCommand,
    },
    /// Show the resolved Steam and Project Zomboid paths
    Paths,
    /// Run steamcmd with the given arguments, or manage the install
    #[command(args_conflicts_with_subcommands = true)]
    Steamcmd {
        #[command(subcommand)]
        command: Option<steamcmd::SteamcmdCommand>,
        /// Arguments passed to steamcmd, e.g. `+app_info_print 380870 +quit`.
        /// With no arguments, just show the located steamcmd binary.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

/// Run the CLI.
pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    logger::init(logger::Level::Info);
    match cli.command {
        Commands::Mods { command } => mods::run(command),
        Commands::Version { command } => version::run(command),
        Commands::Instance { name, command } => instance::run(name, command),
        Commands::Auth { command } => auth::run(command),
        Commands::Paths => paths::run(),
        Commands::Steamcmd { command, args } => steamcmd::run(command, args),
    }
}

/// Open the store, tolerating a missing Steam install (the store location
/// depends on the game drive under WSL, but listing should still work).
pub(crate) fn open_store() -> anyhow::Result<snapoid::store::Store> {
    let game_dir = snapoid::steam::locate().ok().and_then(|p| p.game_dir);
    let root = snapoid::store::locate_root(game_dir.as_deref())?;
    snapoid::store::Store::open(&root)
}
