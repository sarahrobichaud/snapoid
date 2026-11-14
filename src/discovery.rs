//! Best-effort discovery of the local environment: the Steam install,
//! the steamcmd executable, and the mod store root. Discovery reports
//! what exists; it never fails and never decides what a command
//! requires.

use std::path::{Path, PathBuf};

use crate::config;
use crate::steam::{self, SteamPaths};
use crate::steamcmd;
use crate::store;
use crate::warn;

/// The components discovered on this machine. A missing component is
/// `None`; requiring one is the caller's decision.
pub struct Components {
    /// Steam install with the PZ-relevant paths derived from it.
    pub steam: Option<SteamPaths>,
    /// The steamcmd executable.
    pub steamcmd: Option<PathBuf>,
    /// The mod store root.
    pub store_root: Option<PathBuf>,
}

impl Components {
    /// The PZ game install directory, when the Steam install holds one.
    pub fn game_dir(&self) -> Option<&Path> {
        self.steam
            .as_ref()
            .and_then(|paths| paths.game_dir.as_deref())
    }
}

/// Discover the local environment once per command and thread the
/// result down, instead of re-locating at each call site.
pub fn discover() -> Components {
    let steam = match steam::locate() {
        Ok(paths) => Some(paths),
        Err(error) => {
            // Same rule as steamcmd below: a set override that does not
            // resolve is a misconfiguration worth surfacing.
            if config::steam_override().is_some() {
                warn!("{error:#}");
            }
            None
        }
    };
    let steamcmd = match steamcmd::locate() {
        Ok(path) => Some(path),
        Err(error) => {
            // Plain absence is normal and quiet, but a set override that
            // does not resolve is a misconfiguration worth surfacing even
            // to commands that never touch steamcmd.
            if config::steamcmd_override().is_some() {
                warn!("{error:#}");
            }
            None
        }
    };
    let store_root =
        store::locate_root(steam.as_ref().and_then(|p| p.game_dir.as_deref())).ok();
    Components {
        steam,
        steamcmd,
        store_root,
    }
}
