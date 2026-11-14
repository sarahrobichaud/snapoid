use std::path::PathBuf;

use clap::Subcommand;

use yapzm_lib::steamcmd::{self, SteamCmd as _, install};

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
    if let Some(command) = command {
        return match command {
            SteamcmdCommand::Install { to } => {
                let exe = install::install(to.as_deref())?;
                println!("steamcmd installed: {}", exe.display());
                // Only the conventional <drive>:\steamcmd locations are
                // discovered automatically; a custom one needs the env
                // override.
                if steamcmd::locate().map(|found| found != exe).unwrap_or(true) {
                    println!("  snapoid will not find this copy on its own;");
                    println!("  set SNAPOID_STEAMCMD to {}", exe.display());
                }
                println!("  log in once with: snapoid auth login <username>");
                Ok(())
            }
        };
    }
    let program = steamcmd::locate()?;
    if args.is_empty() {
        println!("steamcmd: {}", program.display());
        return Ok(());
    }
    let cmd = steamcmd::Process::new(program);
    cmd.run(&args)
}
