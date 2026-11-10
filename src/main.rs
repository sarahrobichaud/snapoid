use clap::Parser;

#[derive(Parser)]
#[command(
    name = "snapoid",
    version,
    about = "Version manager for Project Zomboid workshop mods",
    long_about = None,
    arg_required_else_help = true
)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}
