//! CLI layer: argument definitions and thin handlers that compose library
//! entry points.

mod auth;
mod instance;
mod instance_mods;
mod mods;
mod paths;
mod steamcmd;
mod version;

use anyhow::Context as _;
use clap::{Parser, Subcommand};

use crate::discovery;
use crate::logger;

#[derive(Parser)]
#[command(
    name = "yapzm",
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
    /// Manage mods
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

/// Open the store at the discovered root. Steam may be missing — the
/// store location only leans on the game drive under WSL, so listing
/// still works without an install.
pub(crate) fn open_store(
    components: &discovery::Components,
) -> anyhow::Result<crate::store::Store> {
    let root = components
        .store_root
        .as_deref()
        .context("no mod store root found")?;
    crate::store::Store::open(root)
}
