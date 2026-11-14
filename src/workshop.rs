//! Workshop item downloads via steamcmd's `workshop_download_item`.
//! steamcmd drops results into its own steamapps/workshop/content tree;
//! callers adopt them into the store. The ACF steamcmd maintains there
//! also records each item's `timeupdated`, which callers store as the
//! freshness marker.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::parser::acf::{self, Acf};
use crate::pz;
use crate::steamcmd::SteamCmd;

/// Download the given workshop items with a single steamcmd invocation
/// and return the content dir holding them
/// (`<steamcmd dir>/steamapps/workshop/content/<app id>`).
///
/// Cached copies of the items in steamcmd's own workshop tree are
/// removed first, so a directory present after the run means a fresh
/// download (steamcmd would otherwise validate and keep an old copy).
/// The caller decides per item whether the download succeeded by
/// checking its directory there.
pub fn download_items(cmd: &dyn SteamCmd, login: &str, item_ids: &[u64]) -> Result<PathBuf> {
    let steamcmd_dir = cmd
        .program_dir()
        .context("steamcmd has no parent directory")?;
    let content = steamcmd_dir
        .join("steamapps/workshop/content")
        .join(pz::APP_ID.to_string());

    for id in item_ids {
        let cached = content.join(id.to_string());
        if cached.is_dir() {
            fs::remove_dir_all(&cached)
                .with_context(|| format!("clearing steamcmd's cached copy {}", cached.display()))?;
        }
    }

    let mut args = vec!["+login".to_owned(), login.to_owned()];
    for id in item_ids {
        args.extend([
            "+workshop_download_item".to_owned(),
            pz::APP_ID.to_string(),
            id.to_string(),
        ]);
    }
    args.push("+quit".to_owned());
    // Inherited stdio: steamcmd renders its own progress and prompts
    // (password, Steam Guard) directly on the terminal, as with the game
    // downloader. Success is verified per item by the downloaded dirs,
    // since steamcmd's exit codes are unreliable.
    cmd.run(&args)?;
    Ok(content)
}

/// Steam's `timeupdated` per item from the workshop ACF in
/// `steamapps_dir`, for the items it knows. One read + parse covers all
/// items, so callers fetching many items look the ids up in the result.
pub fn time_updated_all(steamapps_dir: &Path) -> HashMap<u64, u64> {
    let Ok(text) =
        fs::read_to_string(steamapps_dir.join(format!("appworkshop_{}.acf", pz::APP_ID)))
    else {
        return HashMap::new();
    };
    let Ok(root) = acf::parse(&text) else {
        return HashMap::new();
    };

    let mut times = HashMap::new();
    for section in ["WorkshopItemsInstalled", "WorkshopItemDetails"] {
        let Some(Acf::Map(entries)) = root.get(section) else {
            continue;
        };
        for (id, entry) in entries {
            let Ok(id) = id.parse::<u64>() else {
                continue;
            };
            if let Some(time) = entry
                .get("timeupdated")
                .and_then(Acf::as_str)
                .and_then(|s| s.parse().ok())
            {
                // The installed section wins over the details section.
                times.entry(id).or_insert(time);
            }
        }
    }
    times
}

/// Steam's `timeupdated` for `item_id` from the workshop ACF in
/// `steamapps_dir`, when it knows the item.
pub fn time_updated_in(steamapps_dir: &Path, item_id: u64) -> Option<u64> {
    time_updated_all(steamapps_dir).get(&item_id).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_timeupdated_from_acf() {
        let base = std::env::temp_dir().join(format!("snapoid-wsacf-{}", std::process::id()));
        let steamapps = base.join("steamapps");
        fs::create_dir_all(&steamapps).unwrap();
        fs::write(
            steamapps.join(format!("appworkshop_{}.acf", pz::APP_ID)),
            "\"AppWorkshop_108600\"\n{\n\t\"WorkshopItemsInstalled\"\n\t{\n\t\t\"123\"\n\t\t{\n\t\t\t\"timeupdated\"\t\t\"1700000000\"\n\t\t}\n\t}\n\t\"WorkshopItemDetails\"\n\t{\n\t\t\"123\"\n\t\t{\n\t\t\t\"timeupdated\"\t\t\"1700000001\"\n\t\t}\n\t}\n}\n",
        )
        .unwrap();

        // The installed section wins; unknown items report none.
        assert_eq!(time_updated_in(&steamapps, 123), Some(1_700_000_000));
        assert_eq!(time_updated_in(&steamapps, 456), None);

        fs::remove_dir_all(&base).unwrap();
    }
}
