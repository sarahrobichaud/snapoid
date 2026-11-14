//! User configuration, resolved from the environment.

use std::path::PathBuf;

/// Steam install root to use in place of automatic discovery, when set.
pub fn steam_dir() -> Option<PathBuf> {
    std::env::var_os("YAPZM_STEAM_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
