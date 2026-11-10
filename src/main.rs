mod mods;
mod paths;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "snapoid")]
#[command(version, about = "Version manager for Project Zomboid", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List workshop mods and their version builds
    Mods {
        /// Override the detected workshop mods directory
        #[arg(long)]
        workshop: Option<PathBuf>,
    },
    /// Detect Project Zomboid game and mod paths
    Paths {
        /// Override the detected game install directory
        #[arg(long)]
        game: Option<PathBuf>,
        /// Override the detected workshop mods directory
        #[arg(long)]
        workshop: Option<PathBuf>,
        /// Override the detected user data directory
        #[arg(long)]
        userdata: Option<PathBuf>,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Mods { workshop } => {
            mods::run(workshop);
        }
        Commands::Paths {
            game,
            workshop,
            userdata,
        } => {
            paths::run(game, workshop, userdata);
        }
    }
}
