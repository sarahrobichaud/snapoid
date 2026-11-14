//! The Steam client's own footprint — root install and library list —
//! with nothing here specific to Project Zomboid.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result};

use crate::config;
use crate::parser::acf::{self, Acf};
use crate::platform;
use crate::warn;

/// The Steam client footprint: the root install and every library it
/// knows about.
pub struct Client {
    /// Steam root (contains steamapps/).
    pub root: PathBuf,
    /// Library roots, Steam root first, then any extras from
    /// steamapps/libraryfolders.vdf in their declared order.
    pub libraries: Vec<PathBuf>,
}

impl Client {
    /// Locate the Steam client footprint: the root, then its libraries.
    pub fn locate() -> Result<Self> {
        let root = find_root()?;
        let libraries = find_libraries(&root)?;
        Ok(Self {
            root: platform::normalize_path(root),
            libraries: libraries
                .into_iter()
                .map(platform::normalize_path)
                .collect(),
        })
    }

    /// Workshop content dir for an app: the first library holding it,
    /// else the Steam root's default path.
    pub fn workshop_dir(&self, app_id: u32) -> PathBuf {
        let found = self
            .libraries
            .iter()
            .map(|lib| workshop_dir_in(lib, app_id))
            .find(|p| p.is_dir());
        platform::normalize_path(found.unwrap_or_else(|| workshop_dir_in(&self.root, app_id)))
    }

    /// Game install dir for an app: the first library holding
    /// `steamapps/common/<common_dir>`.
    pub fn game_dir(&self, common_dir: &str) -> Option<PathBuf> {
        self.libraries
            .iter()
            .map(|lib| lib.join("steamapps/common").join(common_dir))
            .find(|p| p.is_dir())
            .map(platform::normalize_path)
    }
}

/// The workshop content path for an app within one library.
fn workshop_dir_in(library: &Path, app_id: u32) -> PathBuf {
    library.join(format!("steamapps/workshop/content/{app_id}"))
}

/// The Steam root: the explicit override when set, else the first
/// candidate holding a steamapps/ directory.
fn find_root() -> Result<PathBuf> {
    if let Some(root) = config::steam_override() {
        if root.join("steamapps").is_dir() {
            return Ok(root);
        }
        anyhow::bail!(
            "YAPZM_STEAM points to {}, which has no steamapps/ folder",
            root.display()
        );
    }

    let mut candidates: Vec<PathBuf> = Vec::new();

    if platform::is_windows_host() {
        if let Some(path) = root_from_registry() {
            candidates.push(path);
        }
    }
    if platform::is_windows() {
        if let Ok(program_files) = std::env::var("ProgramFiles(x86)") {
            candidates.push(PathBuf::from(program_files).join("Steam"));
        }
    }
    if platform::is_wsl() {
        // Steam installed on the Windows host.
        candidates.push(PathBuf::from("/mnt/c/Program Files (x86)/Steam"));
    }
    if !platform::is_windows() {
        if let Ok(home) = std::env::var("HOME") {
            let home = PathBuf::from(home);
            candidates.push(home.join(".steam/steam"));
            candidates.push(home.join(".local/share/Steam"));
            candidates.push(home.join("Library/Application Support/Steam"));
        }
    }

    candidates
        .into_iter()
        .find(|p| p.join("steamapps").is_dir())
        .with_context(|| "no Steam directory with a steamapps/ folder found")
}

/// The Steam root from the Windows registry, read via reg.exe so no
/// Windows-only crate is needed. Also works under WSL interop.
fn root_from_registry() -> Option<PathBuf> {
    let output = Command::new("reg")
        .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_reg_steam_path(&String::from_utf8_lossy(&output.stdout))
}

/// The SteamPath value from `reg query` output, anchored on the type
/// marker since paths may contain spaces.
fn parse_reg_steam_path(stdout: &str) -> Option<PathBuf> {
    for line in stdout.lines() {
        if !line.contains("SteamPath") {
            continue;
        }
        let Some(marker) = line.find("REG_SZ") else {
            continue;
        };
        let value = line[marker + "REG_SZ".len()..].trim();
        if !value.is_empty() {
            return Some(PathBuf::from(value));
        }
    }
    None
}

/// The library list: the Steam root first, then extras from
/// libraryfolders.vdf in their declared order.
fn find_libraries(root: &Path) -> Result<Vec<PathBuf>> {
    let mut libraries = vec![root.to_path_buf()];

    let vdf = root.join("steamapps/libraryfolders.vdf");
    if let Ok(text) = std::fs::read_to_string(&vdf) {
        match acf::parse(&text) {
            Ok(parsed) => {
                for path in extra_libraries(&parsed) {
                    if !libraries.contains(&path) {
                        libraries.push(path);
                    }
                }
            }
            Err(error) => {
                warn!("could not parse {}: {error}", vdf.display());
            }
        }
    }

    Ok(libraries)
}

/// Extra library paths from a parsed libraryfolders.vdf, in declared
/// (numeric key) order.
fn extra_libraries(parsed: &Acf) -> Vec<PathBuf> {
    let mut extra: Vec<(u32, PathBuf)> = Vec::new();
    if let Acf::Map(folders) = parsed {
        for (key, folder) in folders {
            let Some(index) = key.parse::<u32>().ok() else {
                continue;
            };
            let Some(path) = folder.get("path").and_then(Acf::as_str) else {
                continue;
            };
            extra.push((index, PathBuf::from(path)));
        }
    }
    extra.sort_by_key(|(index, _)| *index);
    extra.into_iter().map(|(_, path)| path).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reg_query_output() {
        let output = "\r\nHKEY_CURRENT_USER\\Software\\Valve\\Steam\r\n    SteamPath    REG_SZ    c:/program files (x86)/steam\r\n\r\n";
        assert_eq!(
            parse_reg_steam_path(output),
            Some(PathBuf::from("c:/program files (x86)/steam"))
        );
        assert_eq!(parse_reg_steam_path("no value here"), None);
    }

    #[test]
    fn extra_libraries_follow_declared_order() {
        let text = concat!(
            "\r\n\"libraryfolders\"\r\n{\r\n",
            "    \"1\"\r\n    {\r\n        \"path\"\t\t\"D:\\\\SteamLibrary\"\r\n    }\r\n",
            "    \"0\"\r\n    {\r\n        \"path\"\t\t\"C:\\\\Steam\"\r\n    }\r\n",
            "    \"timestamp\"\r\n    {\r\n        \"path\"\t\t\"E:\\\\ignored\"\r\n    }\r\n}",
        );
        let parsed = acf::parse(text).unwrap();
        assert_eq!(
            extra_libraries(&parsed),
            vec![PathBuf::from("C:\\Steam"), PathBuf::from("D:\\SteamLibrary")]
        );
    }

    #[test]
    fn queries_pick_the_first_library_holding_the_content() {
        let base = std::env::temp_dir().join(format!("yapzm-client-{}", std::process::id()));
        let root = base.join("root");
        let library = base.join("library");
        let content = library.join(format!("steamapps/workshop/content/{}", 108600u32));
        let game = library.join("steamapps/common/ProjectZomboid");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::create_dir_all(&game).unwrap();

        let client = Client {
            root: root.clone(),
            libraries: vec![root, library.clone()],
        };
        assert_eq!(
            client.workshop_dir(108600),
            platform::normalize_path(content)
        );
        assert_eq!(
            client.game_dir("ProjectZomboid"),
            Some(platform::normalize_path(game))
        );
        assert_eq!(client.game_dir("NotInstalled"), None);

        std::fs::remove_dir_all(&base).ok();
    }
}
