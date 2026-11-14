use std::io;
use std::path::PathBuf;

use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

use super::{SteamInstall, SteamKind, validate};

/// Locates Steam via the configured override, its registry key, then default paths.
pub fn locate_steam() -> io::Result<Option<SteamInstall>> {
    if let Some(root) = crate::config::steam_dir() {
        if let Some(install) = validate(&root, SteamKind::Native) {
            return Ok(Some(install));
        }
    }
    if let Some(root) = registry_root() {
        if let Some(install) = validate(&root, SteamKind::Native) {
            return Ok(Some(install));
        }
    }
    for root in candidate_roots() {
        if let Some(install) = validate(&root, SteamKind::Native) {
            return Ok(Some(install));
        }
    }
    Ok(None)
}

/// Reads the install path Steam publishes under its own registry key.
fn registry_root() -> Option<PathBuf> {
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Valve\Steam")
        .ok()?;
    let path: String = key.get_value("SteamPath").ok()?;
    Some(PathBuf::from(path.replace('/', "\\")))
}

/// Probes the default install directories for a Steam folder.
fn candidate_roots() -> Vec<PathBuf> {
    ["ProgramFiles(x86)", "ProgramFiles"]
        .iter()
        .filter_map(|var| std::env::var_os(var))
        .map(|dir| PathBuf::from(dir).join("Steam"))
        .collect()
}
