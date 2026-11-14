use snapoid::pz;
use snapoid::steam;
use snapoid::steamcmd;

use crate::report;

/// Show the resolved Steam and Project Zomboid paths.
pub fn run() -> anyhow::Result<()> {
    let paths = steam::locate()?;
    let version = match paths.game_dir.as_deref() {
        Some(game_dir) => pz::probe::detect_dir(game_dir)?,
        None => pz::probe::GameVersionInfo::default(),
    };
    let steamcmd_path = steamcmd::locate().ok();
    report::print_paths(&paths, &version, steamcmd_path.as_deref());
    Ok(())
}
