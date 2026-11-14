//! Steam client implementation of the game client interface.

use crate::{ClientPaths, GameClient, platform};

/// The Steam game client
pub struct SteamClient {
    pub paths: ClientPaths,
}

impl GameClient for SteamClient {
    /// Detects the Steam install
    fn detect() -> Option<Self> {
        let install = platform::locate_steam().ok()??;

        Some(Self {
            paths: ClientPaths {
                root: install.root,
                libraries: Vec::new(),
            },
        })
    }

    /// The client's display name.
    fn name(&self) -> &str {
        "Steam"
    }

    /// The paths belonging to this client.
    fn paths(&self) -> &ClientPaths {
        &self.paths
    }
}
