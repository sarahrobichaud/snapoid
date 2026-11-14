use std::path::{Path, PathBuf};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(target_os = "linux")]
mod wsl;

/// A located Steam installation, rooted at the directory holding its libraries.
pub struct SteamInstall {
    pub root: PathBuf,
    pub kind: SteamKind,
}

/// The packaging format an installation came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SteamKind {
    /// Installed by the Steam installer or a system package manager.
    Native,
    /// Installed as a Flatpak sandbox.
    Flatpak,
    /// Installed as a Snap sandbox.
    Snap,
}

/// Accepts a root only if it holds a real, used Steam library.
pub(crate) fn validate(root: &Path, kind: SteamKind) -> Option<SteamInstall> {
    let root = root.canonicalize().ok()?;
    if !root.join("steamapps/libraryfolders.vdf").exists() {
        return None;
    }
    Some(SteamInstall {
        root: strip_verbatim(&root),
        kind,
    })
}

/// Removes the extended-length prefix canonicalize adds on Windows drive paths.
fn strip_verbatim(path: &Path) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.chars().nth(1) == Some(':') => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}
