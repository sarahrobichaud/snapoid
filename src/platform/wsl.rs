use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config;

use super::{SteamInstall, SteamKind, validate};

/// Reports whether this process runs inside WSL.
pub(super) fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/version")
        .map(|version| version.to_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

/// Locates the Windows Steam install via the configured override, registry interop, then the C mount.
pub(super) fn locate_windows_steam() -> Option<SteamInstall> {
    if let Some(install) = override_root().and_then(|root| validate(&root, SteamKind::Native)) {
        return Some(install);
    }
    if let Some(root) = registry_root() {
        if let Some(install) = validate(&root, SteamKind::Native) {
            return Some(install);
        }
    }
    for root in c_drive_candidates() {
        if let Some(install) = validate(&root, SteamKind::Native) {
            return Some(install);
        }
    }
    None
}

/// Reads SteamPath from the Windows registry through WSL interop.
fn registry_root() -> Option<PathBuf> {
    let output = Command::new("reg.exe")
        .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|line| line.contains("SteamPath") && line.contains("REG_SZ"))?;
    let windows_path = line.split_once("REG_SZ")?.1.trim();
    mounted_windows_path(windows_path, &automount_root())
}

/// Maps a Windows path onto the drive mount root, rejecting paths without a drive.
fn mounted_windows_path(path: &str, mount_root: &Path) -> Option<PathBuf> {
    let normalized = path.replace('/', "\\");
    let mut parts = normalized.split('\\').filter(|part| !part.is_empty());
    let drive = parts.next()?;
    let mut chars = drive.chars();
    let letter = chars.next()?.to_ascii_lowercase();
    if chars.next() != Some(':') || !letter.is_ascii_lowercase() {
        return None;
    }
    let mut mounted = mount_root.to_path_buf();
    mounted.push(letter.to_string());
    for part in parts {
        mounted.push(part);
    }
    Some(mounted)
}

/// Returns the drive mount root, honoring a custom root declared in WSL's configuration.
fn automount_root() -> PathBuf {
    match std::fs::read_to_string("/etc/wsl.conf") {
        Ok(conf) => parse_automount_root(&conf),
        Err(_) => PathBuf::from("/mnt"),
    }
}

/// Parses the automount root from WSL configuration contents, defaulting when undeclared.
fn parse_automount_root(conf: &str) -> PathBuf {
    let mut in_automount = false;
    for line in conf.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_automount = line.eq_ignore_ascii_case("[automount]");
        } else if in_automount {
            if let Some(value) = line.split_once('=').filter(|(key, _)| key.trim() == "root") {
                let value = value.1.trim().trim_matches('"');
                if !value.is_empty() {
                    return PathBuf::from(value);
                }
            }
        }
    }
    PathBuf::from("/mnt")
}

/// Translates the configured Steam directory into a WSL path when given as a Windows path.
fn override_root() -> Option<PathBuf> {
    let dir = config::steam_dir()?;
    Some(mounted_windows_path(&dir.to_string_lossy(), &automount_root()).unwrap_or(dir))
}

/// Probes the C: mount's default Steam directories.
fn c_drive_candidates() -> Vec<PathBuf> {
    let mount = automount_root().join("c");
    ["Program Files (x86)", "Program Files"]
        .iter()
        .map(|dir| mount.join(dir).join("Steam"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_windows_paths_onto_the_mount_root() {
        let root = Path::new("/mnt");
        assert_eq!(
            mounted_windows_path(r"C:\Steam", root),
            Some(PathBuf::from("/mnt/c/Steam"))
        );
        assert_eq!(
            mounted_windows_path("D:/Games/Steam", root),
            Some(PathBuf::from("/mnt/d/Games/Steam"))
        );
    }

    #[test]
    fn accepts_a_bare_drive_and_a_lowercase_letter() {
        let root = Path::new("/mnt");
        assert_eq!(
            mounted_windows_path("C:", root),
            Some(PathBuf::from("/mnt/c"))
        );
        assert_eq!(
            mounted_windows_path(r"c:\Steam", root),
            Some(PathBuf::from("/mnt/c/Steam"))
        );
    }

    #[test]
    fn rejects_paths_without_a_drive_prefix() {
        let root = Path::new("/mnt");
        assert_eq!(mounted_windows_path(r"\\server\share\Steam", root), None);
        assert_eq!(mounted_windows_path("Steam", root), None);
        assert_eq!(mounted_windows_path("", root), None);
    }

    #[test]
    fn automount_root_defaults_when_undeclared() {
        assert_eq!(parse_automount_root(""), PathBuf::from("/mnt"));
        assert_eq!(
            parse_automount_root(
                r#"
[interop]
root = "/ignored"
"#
            ),
            PathBuf::from("/mnt")
        );
    }

    #[test]
    fn automount_root_reads_the_declared_root() {
        assert_eq!(
            parse_automount_root(
                r#"
[automount]
root = "/windir"
"#
            ),
            PathBuf::from("/windir")
        );
        // The section header matches case-insensitively.
        assert_eq!(
            parse_automount_root(
                r#"
[Automount]
root=/wind
"#
            ),
            PathBuf::from("/wind")
        );
    }

    #[test]
    fn automount_root_ignores_roots_outside_the_section() {
        // A root before any section header, or after the section closes, is ignored.
        assert_eq!(
            parse_automount_root(
                r#"
root = "/early"
[automount]
root = "/wind"
[interop]
root = "/late"
"#
            ),
            PathBuf::from("/wind")
        );
    }
}
