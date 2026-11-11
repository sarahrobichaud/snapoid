//! Workshop restore: moves stored snapshots back into Steam's workshop
//! folder. Must be idempotent: Steam can overwrite the folder again at
//! any time, and re-running a restore must always converge to the
//! snapshot's state.

use anyhow::Result;

use crate::steam::SteamPaths;
use crate::store::Store;

/// Restore a snapshot for `item_id` into the workshop content dir.
/// If `time_updated` is None, use the newest snapshot for that item.
pub fn restore(
    _paths: &SteamPaths,
    _store: &Store,
    _item_id: u64,
    _time_updated: Option<u64>,
) -> Result<()> {
    todo!("deployer")
}
