//! Steam appmanifest reads: the build an install is at.

use std::path::Path;

use crate::parser::acf::{self, Acf};

/// Read the buildid from `<steamapps_dir>/appmanifest_<app_id>.acf`.
/// None when the manifest is missing or unparsable.
pub fn buildid_in_steamapps(steamapps_dir: &Path, app_id: u32) -> Option<u64> {
    let text =
        std::fs::read_to_string(steamapps_dir.join(format!("appmanifest_{app_id}.acf"))).ok()?;
    let Acf::Map(map) = acf::parse(&text).ok()? else {
        return None;
    };
    map.get("buildid")?.as_str()?.parse().ok()
}

/// Buildid of the install at `game_dir` (.../steamapps/common/<name>).
/// None when the manifest is missing or unparsable.
pub fn buildid_for_install(game_dir: &Path, app_id: u32) -> Option<u64> {
    buildid_in_steamapps(game_dir.parent()?.parent()?, app_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_buildid_from_appmanifest() -> anyhow::Result<()> {
        let base = std::env::temp_dir().join(format!("yapzm-manifest-{}", std::process::id()));
        let steamapps = base.join("steamapps");
        std::fs::create_dir_all(&steamapps)?;
        std::fs::write(
            steamapps.join("appmanifest_108600.acf"),
            "\"AppState\"\n{\n\t\"buildid\"\t\t\"17774562\"\n}\n",
        )?;

        assert_eq!(buildid_in_steamapps(&steamapps, 108600), Some(17_774_562));
        assert_eq!(
            buildid_for_install(&steamapps.join("common/ProjectZomboid"), 108600),
            Some(17_774_562)
        );
        assert_eq!(buildid_in_steamapps(&steamapps, 999), None);

        std::fs::remove_dir_all(&base)?;
        Ok(())
    }
}
