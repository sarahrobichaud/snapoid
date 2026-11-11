//! Workshop mod snapshots: copies of workshop items inside the store.
//!
//! Steam owns the workshop folder and can overwrite it at any time; the
//! store holds captured copies Steam will never touch.

use anyhow::Result;

use crate::store::Store;

/// A stored snapshot of one workshop item.
#[derive(Debug)]
pub struct SnapshotRef {
    pub item_id: u64,
    pub time_updated: u64,
}

impl Store {
    /// Copy the current on-disk state of one workshop item into the
    /// store, keyed by its current timeupdated from the ACF.
    pub fn snapshot(&self, _item_id: u64) -> Result<SnapshotRef> {
        todo!("snapshot capture")
    }

    /// List stored snapshots for one item, or all items if None.
    pub fn list(&self, _item_id: Option<u64>) -> Result<Vec<SnapshotRef>> {
        todo!("store listing")
    }

    /// Prune old snapshots, keeping the newest `keep` per item.
    pub fn prune(&self, _keep: usize) -> Result<()> {
        todo!("store pruning")
    }
}
