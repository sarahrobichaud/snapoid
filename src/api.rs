//! Component 7: Update checker.
//!
//! POSTs installed item ids to the Steam Web API endpoint
//! ISteamRemoteStorage/GetPublishedFileDetails/v1 (no API key required)
//! and reports upstream time_updated values for diffing against the store.

#[derive(Debug)]
pub struct RemoteItem {
    pub id: u64,
    pub time_updated: u64,
}

/// Query the Steam Web API for the given workshop items.
pub fn fetch_remote(_item_ids: &[u64]) -> anyhow::Result<Vec<RemoteItem>> {
    todo!("web api client")
}
