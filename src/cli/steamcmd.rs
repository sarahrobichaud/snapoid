use std::path::PathBuf;

use anyhow::Context as _;
use clap::Subcommand;

use crate::discovery;
use crate::steamcmd::{self, SteamCmd as _, install};

#[derive(Subcommand)]
pub enum SteamcmdCommand {
    /// Download steamcmd from the registry-listed package, verify its
    /// checksum, and install it on the game's drive
    Install {
        /// Directory to install into (default: \steamcmd on the drive
        /// holding the game install)
        #[arg(long)]
        to: Option<PathBuf>,
    },
}

/// Run steamcmd with the given arguments, handle steamcmd management
/// subcommands, or show the located binary when there are none.
pub fn run(command: Option<SteamcmdCommand>, args: Vec<String>) -> anyhow::Result<()> {
    if let Some(SteamcmdCommand::Install { to }) = command {
        let exe = install::install(to.as_deref())?;
        println!("steamcmd installed: {}", exe.display());
        // Only the conventional <drive>:\steamcmd locations are
        // discovered automatically; a custom one needs the env override.
        // Discovered after the install so a copy in a conventional
        // location is recognized as found.
        let components = discovery::discover();
        if components.steamcmd.as_deref() != Some(exe.as_path()) {
            println!("  yapzm will not find this copy on its own;");
            println!("  set YAPZM_STEAMCMD to {}", exe.display());
        }
        println!("  log in once with: yapzm auth login <username>");
        return Ok(());
    }
    let components = discovery::discover();
    let program = components
        .steamcmd
        .clone()
        .context("steamcmd not found; install it or set YAPZM_STEAMCMD")?;
    if args.is_empty() {
        println!("steamcmd: {}", program.display());
        return Ok(());
    }
    let cmd = steamcmd::Process::new(program);
    cmd.run(&args)
}
