//! Filesystem operations shared by the store: tree copies with progress,
//! atomic directory swaps, and name sanitization.

use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};

use crate::{debug, warn};

/// Copy progress sink. The CLI renders it; the library only feeds it.
pub trait Progress {
    fn step(&mut self, bytes: u64);
    fn finish(&mut self);
}

/// A progress sink that renders nothing.
pub struct NoopProgress;

impl Progress for NoopProgress {
    fn step(&mut self, _bytes: u64) {}
    fn finish(&mut self) {}
}

/// Creates a progress bar for a copy of `total_bytes`.
pub type ProgressFactory = fn(u64) -> Box<dyn Progress>;

/// A factory that renders nothing.
pub fn noop_progress(_total_bytes: u64) -> Box<dyn Progress> {
    Box::new(NoopProgress)
}

/// Recursively measure `src`: (file count, total bytes). Top-level
/// entries named in `exclude` are skipped, mirroring [`copy_tree`].
pub fn measure_tree(src: &Path, exclude: &[&str]) -> Result<(u64, u64)> {
    let mut files = 0u64;
    let mut bytes = 0u64;
    for entry in fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry.with_context(|| format!("reading {}", src.display()))?;
        let name = entry.file_name();
        if excluded(&name, exclude) {
            continue;
        }

        let file_type = entry
            .file_type()
            .with_context(|| format!("reading {}", entry.path().display()))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let (dir_files, dir_bytes) = measure_tree(&entry.path(), &[])?;
            files += dir_files;
            bytes += dir_bytes;
        } else {
            files += 1;
            bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    Ok((files, bytes))
}

/// Recursively copy `src` into `dst`, reporting each file to `progress`.
/// Top-level entries named in `exclude` are skipped. Symlinks are
/// skipped with a warning: the game install is not expected to contain
/// any, and copying them as links risks dangling or privileged-link
/// errors.
pub fn copy_tree(
    src: &Path,
    dst: &Path,
    exclude: &[&str],
    progress: &mut dyn Progress,
) -> Result<usize> {
    fs::create_dir_all(dst).with_context(|| format!("creating {}", dst.display()))?;
    let mut files = 0usize;
    for entry in fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry.with_context(|| format!("reading {}", src.display()))?;
        let name = entry.file_name();
        if excluded(&name, exclude) {
            debug!("skipping {name:?} (excluded)");
            continue;
        }

        let from = entry.path();
        let to = dst.join(&name);
        let file_type = entry
            .file_type()
            .with_context(|| format!("reading {}", from.display()))?;

        if file_type.is_symlink() {
            warn!("skipping symlink {}", from.display());
        } else if file_type.is_dir() {
            fs::create_dir_all(&to).with_context(|| format!("creating {}", to.display()))?;
            files += copy_tree(&from, &to, &[], progress)?;
        } else {
            let copied =
                fs::copy(&from, &to).with_context(|| format!("copying {}", from.display()))?;
            progress.step(copied);
            files += 1;
            if files.is_multiple_of(2_000) {
                debug!("{files} files copied");
            }
        }
    }
    Ok(files)
}

/// True when `name` matches an entry of `exclude` (case-insensitive).
fn excluded(name: &OsStr, exclude: &[&str]) -> bool {
    let name = name.to_string_lossy();
    exclude.iter().any(|x| x.eq_ignore_ascii_case(&name))
}

/// Move a directory: rename when the filesystems match, copy and remove
/// the source when they do not (e.g. steamcmd's download cache on another
/// drive than the store).
pub fn move_dir(from: &Path, to: &Path) -> Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    let mut progress = NoopProgress;
    copy_tree(from, to, &[], &mut progress)?;
    fs::remove_dir_all(from).with_context(|| format!("removing {}", from.display()))?;
    Ok(())
}

/// Replace `dir` with `staging`, keeping the old copy until the swap
/// succeeds so a failed rename cannot lose an existing directory.
pub fn swap_into_place(staging: &Path, dir: &Path) -> Result<()> {
    let parent = dir
        .parent()
        .context("directory has no parent")?
        .to_path_buf();
    let retired = parent.join(format!(
        ".retired-{}",
        dir.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));

    if dir.exists() {
        if retired.exists() {
            fs::remove_dir_all(&retired)
                .with_context(|| format!("clearing {}", retired.display()))?;
        }
        fs::rename(dir, &retired).with_context(|| format!("retiring {}", dir.display()))?;
        if let Err(error) = fs::rename(staging, dir) {
            fs::rename(&retired, dir).with_context(|| format!("restoring {}", dir.display()))?;
            return Err(error).with_context(|| format!("activating {}", dir.display()));
        }
        fs::remove_dir_all(&retired).with_context(|| format!("removing {}", retired.display()))?;
    } else {
        fs::rename(staging, dir).with_context(|| format!("activating {}", dir.display()))?;
    }
    Ok(())
}

/// Reduce a name to safe path characters.
pub fn sanitize_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        "unknown".to_owned()
    } else {
        safe
    }
}

/// Current Unix time in seconds.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pz;

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize_name("42.22.2"), "42.22.2");
        assert_eq!(sanitize_name("42.10 beta"), "42.10_beta");
        assert_eq!(sanitize_name(""), "unknown");
    }

    #[test]
    fn copies_tree_excluding_top_level_entries() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-test-{}", std::process::id()));
        let src = base.join("game");
        let dst = base.join("out");
        for rel in ["media/lua", "mods/standalone", "Zomboid/saves", "jre64/bin"] {
            fs::create_dir_all(src.join(rel))?;
        }
        fs::write(src.join("projectzomboid.jar"), "jar")?;
        fs::write(src.join("media/lua/a.lua"), "lua")?;
        fs::write(src.join("mods/standalone/mod.info"), "mod")?;
        fs::write(src.join("Zomboid/saves/save.sav"), "save")?;
        fs::write(src.join("jre64/bin/java.exe"), "jvm")?;

        let files = copy_tree(&src, &dst, pz::EXCLUDED_INSTALL_ENTRIES, &mut NoopProgress)?;
        assert_eq!(files, 3, "mods and Zomboid contents must be excluded");
        assert!(dst.join("projectzomboid.jar").is_file());
        assert!(dst.join("media/lua/a.lua").is_file());
        assert!(dst.join("jre64/bin/java.exe").is_file());
        assert!(!dst.join("mods").exists());
        assert!(!dst.join("Zomboid").exists());

        // Measurement mirrors the copy exclusions (3 + 3 + 3 bytes).
        assert_eq!(measure_tree(&src, pz::EXCLUDED_INSTALL_ENTRIES)?, (3, 9));

        fs::remove_dir_all(&base)?;
        Ok(())
    }
}
