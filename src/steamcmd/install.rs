//! steamcmd installer: downloads the official steamcmd.zip, verifies its
//! checksum against the registry, and unpacks it onto a Windows drive.
//!
//! The drive is picked deliberately: `download_depot` drops multi-gigabyte
//! files next to the steamcmd binary before they are moved into the
//! store, so steamcmd belongs on the same drive as the game install —
//! otherwise every pinned-version download pays a cross-drive copy.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result};

use super::{Process, SteamCmd as _, is_update_restart};
use crate::platform;
use crate::registry::{self, SteamcmdPackage};
use crate::{debug, info, warn};

/// How long the whole download may take. The zip is well under a
/// megabyte, but slow links exist.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// Install steamcmd and return the path of the installed executable.
/// `to` overrides the automatically picked install directory.
pub fn install(to: Option<&Path>) -> Result<PathBuf> {
    if to.is_none()
        && let Ok(existing) = super::locate()
    {
        anyhow::bail!(
            "steamcmd is already installed at {}; nothing to do",
            existing.display()
        );
    }

    let package: SteamcmdPackage = registry::load()?
        .steamcmd
        .context("the registry does not list a steamcmd package")?;

    let target = match to {
        Some(dir) => {
            validate_target(dir)?;
            dir.to_path_buf()
        }
        None => default_target()?,
    };

    info!("downloading {}", package.url);
    let bytes = download(&package.url)?;
    let hash = sha256_hex(&bytes);
    if !hash.eq_ignore_ascii_case(&package.sha256) {
        anyhow::bail!(
            "the downloaded steamcmd.zip does not match the checksum in \
             the registry: got {hash}, expected {}; refusing to install \
             it — the registry entry may be outdated, or the download \
             was tampered with",
            package.sha256
        );
    }
    debug!("checksum OK ({hash})");

    extract(&bytes, &target).with_context(|| format!("extracting into {}", target.display()))?;
    let exe = target.join("steamcmd.exe");
    if !exe.is_file() {
        anyhow::bail!("the archive did not contain {}", exe.display());
    }

    // First run: lets steamcmd self-update and proves the install works.
    // Exit code 7 is steamcmd asking for a restart after self-updating —
    // expected on a fresh install, not a failure.
    info!("running steamcmd once to self-update");
    let cmd = Process::new(exe.clone());
    if let Err(error) = cmd.run(&["+quit".to_owned()]) {
        if is_update_restart(&error) {
            debug!("steamcmd self-updated and asked for a restart; expected");
        } else {
            warn!("steamcmd did not run cleanly after install ({error:#})");
        }
    }

    Ok(exe)
}

/// Pick the default install dir: `steamcmd` on the drive holding the
/// game install (or the Steam install), else the classic `C:\steamcmd`
/// from Valve's install guide.
fn default_target() -> Result<PathBuf> {
    let paths = crate::steam::locate().ok();
    let anchor = paths
        .as_ref()
        .and_then(|p| p.game_dir.as_deref())
        .or_else(|| paths.as_ref().map(|p| p.steam_dir.as_path()));
    if let Some(dir) = anchor.and_then(drive_root) {
        return Ok(dir.join("steamcmd"));
    }
    fallback_target()
}

/// The drive root holding `path`, in host form: the /mnt/<letter> root
/// under WSL, the <X>:\ root on Windows.
fn drive_root(path: &Path) -> Option<PathBuf> {
    if platform::is_wsl() {
        return platform::wsl_drive_root(path);
    }
    if cfg!(windows) {
        let text = path.to_string_lossy();
        let mut chars = text.chars();
        if let (Some(drive), Some(':')) = (chars.next(), chars.next())
            && drive.is_ascii_alphabetic()
        {
            return Some(PathBuf::from(format!("{drive}:\\")));
        }
    }
    None
}

fn fallback_target() -> Result<PathBuf> {
    if cfg!(windows) {
        return Ok(PathBuf::from(r"C:\steamcmd"));
    }
    if platform::is_wsl() {
        return Ok(PathBuf::from("/mnt/c/steamcmd"));
    }
    anyhow::bail!(
        "steamcmd cannot be installed automatically on this platform; \
         on Linux, install it with your package manager instead"
    )
}

/// Under WSL the Windows steamcmd.exe is useless on the Linux filesystem;
/// it must live on a Windows drive automount.
fn validate_target(target: &Path) -> Result<()> {
    if platform::is_wsl() && platform::wsl_drive_root(target).is_none() {
        anyhow::bail!(
            "under WSL, steamcmd must be installed on a Windows drive \
             (/mnt/<letter>/...); {} is not on one",
            target.display()
        );
    }
    Ok(())
}

fn download(url: &str) -> Result<Vec<u8>> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .build()
        .new_agent();
    let mut response = agent
        .get(url)
        // Same-path-different-content artifact: a stale CDN entry would
        // serve an old zip that fails the checksum, so pin identity.
        .header("accept-encoding", "identity")
        .call()
        .with_context(|| format!("downloading {url}"))?;
    response
        .body_mut()
        .read_to_vec()
        .with_context(|| format!("reading the response from {url}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Extract the zip into `target`, refusing entries that escape it.
fn extract(bytes: &[u8], target: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).context("reading steamcmd.zip")?;
    std::fs::create_dir_all(target).with_context(|| format!("creating {}", target.display()))?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().context("reading an archive entry name")?;
        let out = entry_target(target, &name)?;
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut file)?;
        // The zip is built on Windows and carries no Unix mode bits; WSL
        // needs the execute bit to launch the exe through interop.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(())
}

/// Resolve an archive entry name against `target`, refusing paths that
/// escape it (zip-slip).
fn entry_target(target: &Path, name: &str) -> Result<PathBuf> {
    let escapes = Path::new(name).components().any(|component| {
        !matches!(
            component,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    });
    if escapes {
        anyhow::bail!("the archive contains an unsafe path: {name}");
    }
    Ok(target.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_known_vectors() {
        // SHA-256 of the empty string.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn extracts_zip_entries_into_target() -> Result<()> {
        // Build a small zip in memory: one nested file, one plain file.
        let mut zip_bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut zip_bytes));
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("steamcmd.exe", options)?;
            std::io::Write::write_all(&mut writer, b"MZ fake exe")?;
            writer.start_file("sub/steamclient.dll", options)?;
            std::io::Write::write_all(&mut writer, b"dll bytes")?;
            writer.finish()?;
        }

        let base = std::env::temp_dir().join(format!("yapzm-install-{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        extract(&zip_bytes, &base)?;

        assert_eq!(std::fs::read(base.join("steamcmd.exe"))?, b"MZ fake exe");
        assert_eq!(
            std::fs::read(base.join("sub/steamclient.dll"))?,
            b"dll bytes"
        );

        std::fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn rejects_entries_that_escape_target() {
        let base = Path::new("/store");
        assert!(entry_target(base, "../evil.txt").is_err());
        assert!(entry_target(base, "a/../../evil.txt").is_err());
        assert!(entry_target(base, "steamcmd.exe").is_ok());
        assert!(entry_target(base, "sub/steamclient.dll").is_ok());
        assert!(entry_target(base, "./fine.txt").is_ok());
    }

    #[test]
    fn picks_the_drive_of_the_game_install() {
        if platform::is_wsl() {
            assert_eq!(
                drive_root(Path::new("/mnt/c/Program Files (x86)/Steam/game")),
                Some(PathBuf::from("/mnt/c"))
            );
            assert_eq!(drive_root(Path::new("/home/user/game")), None);
        } else if cfg!(windows) {
            assert_eq!(
                drive_root(Path::new(r"D:\Games\ProjectZomboid")),
                Some(PathBuf::from(r"D:\"))
            );
        }
    }
}
