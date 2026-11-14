use anyhow::Context as _;

use crate::discovery;
use crate::pz;
use crate::report;

/// Show the discovered Steam and Project Zomboid paths.
pub fn run() -> anyhow::Result<()> {
    let components = discovery::discover();
    let paths = components
        .steam
        .as_ref()
        .context("no Steam installation found")?;
    let version = match paths.game_dir.as_deref() {
        Some(game_dir) => pz::probe::detect_dir(game_dir)?,
        None => pz::probe::GameVersionInfo::default(),
    };
    report::print_paths(paths, &version, components.steamcmd.as_deref());
    Ok(())
}
