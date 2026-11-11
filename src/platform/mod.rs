//! Host environment: WSL detection, Windows path mapping, and directory
//! links that work across the WSL boundary.

pub mod link;

pub use link::create_link;

use std::path::{Path, PathBuf};

/// Entries of C:\Users that are not user profiles.
const NON_PROFILE_USERS: &[&str] = &["public", "default", "default user", "all users"];

/// True when running under WSL (kernel release reports Microsoft/WSL).
pub fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|release| {
            let release = release.to_ascii_lowercase();
            release.contains("microsoft") || release.contains("wsl")
        })
        .unwrap_or(false)
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

/// Find a Windows user profile on the given drive: entries of
/// <drive>/Users, skipping non-profile dirs, that have an
/// AppData/Roaming folder. Prefers the profile that owns the Zomboid
/// user dir, since that is the user snapoid is managing; otherwise the
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
    fn finds_windows_profile_on_drive() -> anyhow::Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-prof-{}", std::process::id()));
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
