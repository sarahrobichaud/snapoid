//! Game instances: named slots whose game folder links to a stored
//! version.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context as _, Result};

use crate::fsops::{sanitize_name, unix_now};
use crate::platform;
use crate::store::meta::{self, GameMeta, InstanceMeta};
use crate::store::{Error, Store};
use crate::warn;

/// A game instance: a named slot whose game folder links to a version.
#[derive(Debug)]
pub struct GameInstance {
    /// Instance name; also the name of its folder.
    pub name: String,
    /// Game version the instance is linked to.
    pub version: String,
    pub revision: Option<String>,
    pub dir: PathBuf,
    /// The instance's linked game folder (instance/<name>/game).
    pub game_dir: PathBuf,
    /// Unix seconds at creation time; 0 when unknown.
    pub created: u64,
}

impl Store {
    /// Create `instance/<name>` whose game folder links to
    /// `game/<version>`.
    pub fn instance_new(&self, name: &str, version: &str) -> Result<GameInstance> {
        let game = self.game_dir(version);
        if !game.is_dir() {
            return Err(Error::VersionNotStored {
                version: version.to_owned(),
            }
            .into());
        }
        let revision = meta::read::<GameMeta>(&game.join("game.json"))
            .ok()
            .flatten()
            .and_then(|m| m.revision);

        let dir = self.instance_dir(name);
        if dir.exists() {
            if dir.join("instance.json").is_file() {
                return Err(Error::InstanceExists {
                    name: name.to_owned(),
                    dir: dir.clone(),
                }
                .into());
            }
            // No metadata: a leftover from a failed creation. snapoid owns
            // this folder, so clear it and start fresh.
            warn!("removing leftover instance dir {}", dir.display());
            fs::remove_dir_all(&dir).with_context(|| format!("clearing {}", dir.display()))?;
        }
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

        let link = dir.join("game");
        if let Err(error) = platform::create_link(
            &link,
            &PathBuf::from("../../game").join(sanitize_name(version)),
        ) {
            // Don't leave a half-created instance behind.
            let _ = fs::remove_dir_all(&dir);
            return Err(error)
                .with_context(|| format!("linking instance {name} to version {version}"));
        }

        let created = unix_now();
        meta::write(
            &dir.join("instance.json"),
            &InstanceMeta {
                version: version.to_owned(),
                revision: revision.clone(),
                created,
            },
        )?;

        Ok(GameInstance {
            name: name.to_owned(),
            version: version.to_owned(),
            revision,
            dir,
            game_dir: link,
            created,
        })
    }

    /// List stored game instances, sorted by name.
    pub fn list_instances(&self) -> Result<Vec<GameInstance>> {
        let dir = self.instances_dir();
        if !dir.is_dir() {
            return Ok(Vec::new());
        }

        let mut instances = Vec::new();
        for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
            let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
            let path = entry.path();
            if !path.is_dir()
                || path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            let instance_meta = meta::read::<InstanceMeta>(&path.join("instance.json"))
                .ok()
                .flatten();
            instances.push(GameInstance {
                name: entry.file_name().to_string_lossy().into_owned(),
                version: instance_meta
                    .as_ref()
                    .map(|m| m.version.clone())
                    .unwrap_or_else(|| "unknown".to_owned()),
                revision: instance_meta.as_ref().and_then(|m| m.revision.clone()),
                game_dir: path.join("game"),
                dir: path,
                created: instance_meta.as_ref().map(|m| m.created).unwrap_or(0),
            });
        }

        instances.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(instances)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_instance_linked_to_version() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-inst-{}", std::process::id()));
        let store = Store::open_at(&base)?;

        let game = store.game_dir("42.22.2");
        fs::create_dir_all(&game)?;
        fs::write(game.join("projectzomboid.jar"), "jar")?;

        let instance = store.instance_new("survivor", "42.22.2")?;
        assert_eq!(instance.version, "42.22.2");
        assert!(instance.game_dir.is_dir(), "game link should resolve");
        assert!(instance.game_dir.join("projectzomboid.jar").is_file());
        assert!(instance.dir.join("instance.json").is_file());

        // Unknown versions and duplicate names are rejected.
        assert!(store.instance_new("bad", "0.0").is_err());
        assert!(store.instance_new("survivor", "42.22.2").is_err());

        fs::remove_dir_all(&base)?;
        Ok(())
    }
}
