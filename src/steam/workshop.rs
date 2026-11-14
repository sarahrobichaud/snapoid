//! The Steam workshop: where an app's downloaded content lives in a
//! Steam layout.

use std::path::{Path, PathBuf};

use crate::platform;
use crate::steam::client::Client;

/// The Steam workshop content for one app.
pub struct Workshop {
    /// Content dir for the app (.../steamapps/workshop/content/<app_id>).
    pub content_dir: PathBuf,
}

impl Workshop {
    /// Locate the workshop content dir for an app: the first client
    /// library holding it, else the root's default path.
    pub fn locate(client: &Client, app_id: u32) -> Self {
        let found = client
            .libraries
            .iter()
            .map(|lib| content_dir_in(lib, app_id))
            .find(|p| p.is_dir());
        Self {
            content_dir: platform::normalize_path(
                found.unwrap_or_else(|| content_dir_in(&client.root, app_id)),
            ),
        }
    }
}

/// The workshop content path for an app within one library.
fn content_dir_in(library: &Path, app_id: u32) -> PathBuf {
    library.join(format!("steamapps/workshop/content/{app_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_the_first_library_holding_the_content() {
        let base = std::env::temp_dir().join(format!("yapzm-workshop-{}", std::process::id()));
        let root = base.join("root");
        let library = base.join("library");
        let content = library.join(format!("steamapps/workshop/content/{}", 108600u32));
        std::fs::create_dir_all(&content).unwrap();

        let client = Client {
            root: root.clone(),
            libraries: vec![root, library.clone()],
        };
        assert_eq!(
            Workshop::locate(&client, 108600).content_dir,
            platform::normalize_path(content)
        );

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn falls_back_to_the_root_default_when_nothing_is_downloaded() {
        let base = std::env::temp_dir().join(format!("yapzm-workshop-empty-{}", std::process::id()));
        let root = base.join("root");
        std::fs::create_dir_all(root.join("steamapps")).unwrap();

        let client = Client {
            root: root.clone(),
            libraries: vec![root.clone()],
        };
        assert_eq!(
            Workshop::locate(&client, 108600).content_dir,
            platform::normalize_path(root.join(format!("steamapps/workshop/content/108600")))
        );

        std::fs::remove_dir_all(&base).ok();
    }
}
