//! Directory links: symlinks natively, junctions on Windows drives.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result};

// Only the unix branch of create_link crosses the WSL boundary.
#[cfg(unix)]
use super::{is_wsl, wsl_drive_root, wslpath};
#[cfg(unix)]
use crate::debug;

/// Create a directory link at `link` pointing at `target` (a path
/// relative to the link's directory).
///
/// Under WSL with the link on a Windows drive, the link is created as a
/// junction through cmd.exe interop: junctions need no privileges and are
/// followed by Windows processes (the game). Native Linux uses a
/// relative symlink; native Windows uses mklink /J, which needs an
/// absolute backslash target (its parser reads `/..` as a switch).
pub fn create_link(link: &Path, target: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        if is_wsl() && wsl_drive_root(link).is_some() {
            let absolute = link.parent().map(|base| base.join(target));
            let absolute = normalize_path(&absolute.unwrap_or_else(|| target.to_path_buf()));
            if junction_via_interop(link, &absolute)? {
                return Ok(());
            }
        }
        std::os::unix::fs::symlink(target, link)
            .with_context(|| format!("linking {} -> {}", link.display(), target.display()))?;
        Ok(())
    }
    #[cfg(windows)]
    {
        let absolute = link.parent().map(|base| base.join(target));
        let absolute = normalize_path(&absolute.unwrap_or_else(|| target.to_path_buf()));
        let absolute = absolute.to_string_lossy().replace('/', "\\");
        let status = Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(&absolute)
            .status()
            .context("running mklink")?;
        if !status.success() {
            anyhow::bail!("mklink /J {} {} failed", link.display(), absolute);
        }
        Ok(())
    }
}

/// Resolve `..` components lexically, without touching the filesystem.
fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            component => out.push(component.as_os_str()),
        }
    }
    out
}

/// Try to create a junction through WSL↔Windows interop, converting both
/// paths to their Windows form with wslpath. Returns Ok(false) when
/// interop is unavailable (no wslpath or cmd.exe).
#[cfg(unix)]
fn junction_via_interop(link: &Path, target: &Path) -> Result<bool> {
    let (Some(win_link), Some(win_target)) = (wslpath(link), wslpath(target)) else {
        return Ok(false);
    };
    match Command::new("cmd.exe")
        .args(["/c", "mklink", "/J"])
        .arg(&win_link)
        .arg(&win_target)
        .status()
    {
        Ok(status) if status.success() => {
            debug!("junction {win_link} -> {win_target}");
            Ok(true)
        }
        Ok(status) => anyhow::bail!("mklink /J failed ({status})"),
        Err(_) => Ok(false),
    }
}
