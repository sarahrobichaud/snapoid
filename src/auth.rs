//! The remembered Steam account for steamcmd.
//!
//! `snapoid auth login` runs an interactive steamcmd login and, when the
//! credentials verify, remembers the account name in `auth.json` in the
//! store root. Downloads then reuse it instead of requiring `--login` on
//! every invocation. Only the account *name* is stored here — the
//! password stays in steamcmd's own credential cache, which `+login
//! <name>` (no password) picks up.
//!
//! The store root is used on purpose: under WSL it lives on the Windows
//! side, the same side as the Windows steamcmd.exe whose credential cache
//! is being relied on, so Windows and WSL invocations share one account.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::fsops::unix_now;
use crate::store::meta;

/// Sidecar holding the remembered account.
#[derive(Debug, Serialize, Deserialize)]
struct AuthMeta {
    login: String,
    /// Unix seconds at creation time.
    created: u64,
}

/// Path of the auth sidecar in the store root.
fn path(root: &Path) -> PathBuf {
    root.join("auth.json")
}

/// The remembered account name, if any.
pub fn load(root: &Path) -> Option<String> {
    meta::read::<AuthMeta>(&path(root))
        .ok()
        .flatten()
        .map(|meta| meta.login)
}

/// Remember `login` as the account for future downloads.
pub fn save(root: &Path, login: &str) -> Result<()> {
    fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
    meta::write(
        &path(root),
        &AuthMeta {
            login: login.to_owned(),
            created: unix_now(),
        },
    )
}

/// Forget the remembered account, if any. A missing file counts as done.
pub fn clear(root: &Path) -> Result<()> {
    match fs::remove_file(path(root)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("removing {}", path(root).display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load_roundtrip() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-auth-{}", std::process::id()));
        fs::remove_dir_all(&base).ok();

        assert_eq!(load(&base), None);

        save(&base, "gordon")?;
        assert_eq!(load(&base).as_deref(), Some("gordon"));

        clear(&base)?;
        assert_eq!(load(&base), None);
        // Clearing again is not an error.
        clear(&base)?;

        fs::remove_dir_all(&base)?;
        Ok(())
    }
}
