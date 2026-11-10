//! Component 5: Snapshot store.
//!
//! The stable territory snapoid owns. Steam owns the workshop folder; this
//! store holds captured copies that Steam will never touch.
//!
//! Layout: <root>/store/<pfid>/<timeupdated>/... plus index.json.

use std::path::PathBuf;

pub struct Store {
    root: PathBuf,
}

#[derive(Debug)]
pub struct SnapshotRef {
    pub item_id: u64,
    pub time_updated: u64,
}

impl Store {
    /// Open (creating if needed) the default store location via `dirs`:
    /// ~/.local/share/snapoid on Linux, %APPDATA%\snapoid on Windows.
    pub fn open() -> anyhow::Result<Self> {
        todo!("store open")
    }

    /// Copy the current on-disk state of one workshop item into the store,
    /// keyed by its current timeupdated from the ACF.
    pub fn snapshot(&self, _item_id: u64) -> anyhow::Result<SnapshotRef> {
        todo!("snapshot capture")
    }

    /// List stored snapshots for one item, or all items if None.
    pub fn list(&self, _item_id: Option<u64>) -> anyhow::Result<Vec<SnapshotRef>> {
        todo!("store listing")
    }

    /// Prune old snapshots, keeping the newest `keep` per item.
    pub fn prune(&self, _keep: usize) -> anyhow::Result<()> {
        todo!("store pruning")
    }
}
