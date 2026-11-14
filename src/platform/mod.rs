//! Host environment: WSL detection, Windows path mapping, and directory
//! links that work across the WSL boundary.

pub mod link;

pub use link::create_link;

use std::path::{MAIN_SEPARATOR_STR, Path, PathBuf};

/// Entries of C:\Users that are not user profiles.
const NON_PROFILE_USERS: &[&str] = &["public", "default", "default user", "all users"];

/// True when compiled for Windows.
pub fn is_windows() -> bool {
    cfg!(windows)
}

/// True when running under WSL (kernel release reports Microsoft/WSL).
pub fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|release| {
            let release = release.to_ascii_lowercase();
            release.contains("microsoft") || release.contains("wsl")
        })
        .unwrap_or(false)
}

/// True when the Windows host filesystem is the target: native Windows,
/// or WSL reaching the host through /mnt.
pub fn is_windows_host() -> bool {
    is_windows() || is_wsl()
}

/// For a path under a WSL drive automount (/mnt/<letter>/...), return the
/// drive root /mnt/<letter>. None for anything else, so a Linux-side
/// install keeps Linux-side behavior.
pub fn wsl_drive_root(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    match components.next()? {
        std::path::Component::RootDir => {}
        _ => return None,
    }
    match components.next()? {
        std::path::Component::Normal(name) if name == "mnt" => {}
        _ => return None,
    }
    match components.next()? {
        std::path::Component::Normal(name) => {
            let letter = name.to_string_lossy();
            if letter.len() == 1 && letter.chars().all(|c| c.is_ascii_alphabetic()) {
                Some(PathBuf::from("/mnt").join(letter.into_owned()))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Automounted WSL drive roots (/mnt/<letter>), sorted by letter.
pub fn wsl_drives() -> Vec<PathBuf> {
    let mut drives: Vec<PathBuf> = std::fs::read_dir("/mnt")
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| wsl_drive_root(&entry.path()))
                .filter(|drive| drive.is_dir())
                .collect()
        })
        .unwrap_or_default();
    drives.sort();
    drives
}

/// Find a Windows user profile on the given drive: entries of
/// <drive>/Users, skipping non-profile dirs, that have an
/// AppData/Roaming folder. Prefers the profile that owns the Zomboid
/// user dir, since that is the user yapzm is managing; otherwise the
/// first (sorted) candidate.
pub fn windows_profile_on(drive: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(drive.join("Users"))
        .ok()?
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|path| {
            let Some(name) = path.file_name() else {
                return false;
            };
            let name = name.to_string_lossy().to_lowercase();
            !NON_PROFILE_USERS.contains(&name.as_str()) && path.join("AppData/Roaming").is_dir()
        })
        .collect();
    candidates.sort();
    candidates
        .iter()
        .find(|profile| profile.join("Zomboid").is_dir())
        .or_else(|| candidates.first())
        .cloned()
}

/// Normalize a path to the host platform's canonical form: separators
/// become the platform-native one and a Windows drive letter is
/// uppercased. Paths entering from external sources are inconsistent —
/// registry values use `/` and a lowercase drive (`c:/program files`),
/// VDF library paths use `\\`, env vars use `\\` — so every path that
/// crosses such a boundary should pass through here. This is purely
/// cosmetic on Windows (case-insensitive filesystem); on other platforms
/// it is the identity.
pub fn normalize_path(path: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return path;
    }
    let text = path.to_string_lossy().replace('/', MAIN_SEPARATOR_STR);
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic() => {
            // The drive letter is ASCII, so byte-offset slicing is safe.
            PathBuf::from(format!(
                "{}{}",
                drive.to_ascii_uppercase(),
                &text[drive.len_utf8()..]
            ))
        }
        _ => PathBuf::from(text),
    }
}

/// Convert a path to its Windows form via wslpath; None when unavailable.
#[cfg(unix)]
pub fn wslpath(path: &Path) -> Option<String> {
    let output = std::process::Command::new("wslpath")
        .arg("-w")
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_drive_root_from_mnt_paths() {
        assert_eq!(
            wsl_drive_root(Path::new("/mnt/c/Program Files (x86)/Steam/game")),
            Some(PathBuf::from("/mnt/c"))
        );
        assert_eq!(
            wsl_drive_root(Path::new("/mnt/d")),
            Some(PathBuf::from("/mnt/d"))
        );
        // Linux-side installs keep Linux-side behavior.
        assert_eq!(wsl_drive_root(Path::new("/home/user/game")), None);
        // Non-drive automounts are not drives.
        assert_eq!(wsl_drive_root(Path::new("/mnt/cdrom/game")), None);
        assert_eq!(wsl_drive_root(Path::new("/mnt")), None);
        assert_eq!(wsl_drive_root(Path::new("relative/path")), None);
    }

    #[test]
    fn normalizes_paths_to_platform_native_form() {
        let mixed = PathBuf::from("c:/program files (x86)/steam").join("steamapps/common");
        let normalized = normalize_path(mixed);
        if cfg!(windows) {
            assert_eq!(
                normalized,
                PathBuf::from(r"C:\program files (x86)\steam\steamapps\common")
            );
            // Already-canonical paths pass through unchanged.
            assert_eq!(
                normalize_path(PathBuf::from(r"C:\Users\zyriel\Zomboid")),
                PathBuf::from(r"C:\Users\zyriel\Zomboid")
            );
            // UNC paths have no drive letter to uppercase.
            assert_eq!(
                normalize_path(PathBuf::from(r"\\server/share/dir")),
                PathBuf::from(r"\\server\share\dir")
            );
        } else {
            assert_eq!(
                normalized,
                PathBuf::from("c:/program files (x86)/steam/steamapps/common")
            );
        }
    }

    #[test]
    fn finds_windows_profile_on_drive() -> anyhow::Result<()> {
        let base = std::env::temp_dir().join(format!("yapzm-prof-{}", std::process::id()));
        let users = base.join("Users");
        std::fs::create_dir_all(users.join("Public"))?;
        std::fs::create_dir_all(users.join("Alpha/AppData/Roaming"))?;
        std::fs::create_dir_all(users.join("Beta/AppData/Roaming"))?;
        std::fs::create_dir_all(users.join("Gamma"))?; // profile without AppData

        // No Zomboid owner yet: first sorted profile wins.
        assert_eq!(windows_profile_on(&base), Some(users.join("Alpha")));

        // The Zomboid owner is preferred over sort order.
        std::fs::create_dir_all(users.join("Beta/Zomboid"))?;
        assert_eq!(windows_profile_on(&base), Some(users.join("Beta")));

        std::fs::remove_dir_all(&base)?;
        Ok(())
    }
}
