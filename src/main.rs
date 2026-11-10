use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "snapoid")]
#[command(version, about = "A CLI tool", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Greet someone by name
    Greet {
        /// Name of the person to greet
        #[arg(default_value = "world")]
        name: String,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Greet { name } => {
            println!("Hello, {name}!");
        }
    }
}
