//! Health check command reporting detected game clients.

use anyhow::Result;

use crate::{GameClient, cli::print_header, steam::SteamClient};

/// Clients detected on this machine, absent when not installed.
struct Clients {
    steam: Option<SteamClient>,
}

/// Runs the health check, reporting which game clients were detected.
pub fn run(is_verbose: bool) -> Result<()> {

    let clients = Clients {
        steam: SteamClient::detect(),
    };

    print_header("Health Check");

    println!("Detected Clients:");
    println!("");

    if let Some(steam) = &clients.steam {
        print_client(steam, is_verbose);
    }

    Ok(())
}


/// Prints one client's status, with root and library paths when verbose.
fn print_client(client: &impl GameClient, is_verbose: bool) {
    if is_verbose {
        let paths = client.paths();
        println!("[x] {}:", client.name());
        println!("    Root: {}", paths.root.display());

        if !paths.libraries.is_empty() {
            println!("    Libraries:");
            for lib in &paths.libraries {
                println!("{}", lib.display());
            }
        }
    } else {
        println!("[x] {}", client.name());
    }
}
