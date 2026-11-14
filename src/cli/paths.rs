use anyhow::Context as _;

use crate::pz::{self, install};
use crate::report;
use crate::steam::client::Client;
use crate::steam::steamcmd;
use crate::steam::workshop::Workshop;

/// Show the discovered Steam and Project Zomboid layout.
pub fn run() -> anyhow::Result<()> {
    let client = Client::locate().context("no Steam installation found")?;
    let workshop = Workshop::locate(&client, pz::APP_ID);
    let install = match client.game_dir(pz::COMMON_DIR) {
        Some(game_dir) => Some(install::probe_install(&game_dir)?),
        None => None,
    };
    let steamcmd_path = steamcmd::locate().ok();
    report::print_paths(
        &client,
        &workshop,
        install.as_ref(),
        install::user_dir().as_deref(),
        steamcmd_path.as_deref(),
    );
    Ok(())
}
