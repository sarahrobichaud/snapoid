use snapoid::scanner::workshop;
use snapoid::steam;

use crate::report;

/// List installed workshop items and the mods they contain.
pub fn run() -> anyhow::Result<()> {
    let paths = steam::locate()?;
    let items = workshop::scan(&paths)?;
    if items.is_empty() {
        println!("No workshop items installed for Project Zomboid.");
        return Ok(());
    }
    snapoid::info!("found {} workshop item(s)", items.len());
    report::print_scan(&items);
    Ok(())
}
