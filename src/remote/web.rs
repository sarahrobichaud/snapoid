//! Steam Web API queries.

/// A workshop item's upstream state.
#[derive(Debug)]
pub struct RemoteItem {
    pub id: u64,
    pub time_updated: u64,
}

/// Query the Steam Web API for the given workshop items.
#[allow(dead_code)] // scaffold; not wired up yet
pub fn fetch_remote(_item_ids: &[u64]) -> anyhow::Result<Vec<RemoteItem>> {
    todo!("web api client")
}
