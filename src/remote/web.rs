//! Steam Web API queries.

use std::time::Duration;

use anyhow::{Context as _, Result};
use serde::Deserialize;

use crate::warn;

/// A workshop item's upstream state.
#[derive(Debug)]
pub struct RemoteItem {
    pub id: u64,
    pub time_updated: u64,
    /// The item's title, when the API reports one.
    pub title: Option<String>,
}

const ENDPOINT: &str =
    "https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/";
const TIMEOUT: Duration = Duration::from_secs(30);
/// The API takes at most 100 ids per request.
const BATCH: usize = 100;

/// Query the Steam Web API for the given workshop items' upstream
/// `time_updated`. Anonymous: published file details are public
/// metadata. Items the API cannot report (deleted, private) are omitted.
pub fn fetch_remote(item_ids: &[u64]) -> Result<Vec<RemoteItem>> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .new_agent();

    let mut items = Vec::new();
    for batch in item_ids.chunks(BATCH) {
        let mut body = format!("itemcount={}", batch.len());
        for (index, id) in batch.iter().enumerate() {
            body.push_str(&format!("&publishedfileids[{index}]={id}"));
        }
        let mut response = agent
            .post(ENDPOINT)
            .header("content-type", "application/x-www-form-urlencoded")
            .send(&body)
            .with_context(|| format!("querying {ENDPOINT}"))?;
        let text = response
            .body_mut()
            .read_to_string()
            .with_context(|| format!("reading the {ENDPOINT} response"))?;
        let details: DetailsResponse = serde_json::from_str(&text)
            .with_context(|| format!("parsing the {ENDPOINT} response"))?;
        for file in details.response.publishedfiledetails {
            if file.result != 1 {
                warn!(
                    "workshop item {} is not publicly readable (result {})",
                    file.publishedfileid, file.result
                );
                continue;
            }
            let Ok(id) = file.publishedfileid.parse() else {
                warn!(
                    "workshop item id {:?} is not a number",
                    file.publishedfileid
                );
                continue;
            };
            items.push(RemoteItem {
                id,
                time_updated: file.time_updated,
                title: file.title,
            });
        }
    }
    Ok(items)
}

#[derive(Debug, Deserialize)]
struct DetailsResponse {
    response: DetailsList,
}

#[derive(Debug, Deserialize)]
struct DetailsList {
    #[serde(default)]
    publishedfiledetails: Vec<PublishedFileDetails>,
}

#[derive(Debug, Deserialize)]
struct PublishedFileDetails {
    /// The API quotes the id.
    publishedfileid: String,
    /// 1 = ok; anything else is a per-item error (deleted, private, ...).
    result: u32,
    #[serde(default)]
    time_updated: u64,
    #[serde(default)]
    title: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_details_response() {
        let text = r#"{
            "response": {
                "publishedfiledetails": [
                    { "publishedfileid": "123", "result": 1, "time_updated": 1700000000, "title": "A Mod" },
                    { "publishedfileid": "456", "result": 9 }
                ]
            }
        }"#;
        let details: DetailsResponse = serde_json::from_str(text).unwrap();
        assert_eq!(details.response.publishedfiledetails.len(), 2);
        assert_eq!(details.response.publishedfiledetails[0].result, 1);
        assert_eq!(
            details.response.publishedfiledetails[0].time_updated,
            1_700_000_000
        );
        assert_eq!(
            details.response.publishedfiledetails[0].title.as_deref(),
            Some("A Mod")
        );
        assert_eq!(details.response.publishedfiledetails[1].result, 9);
        assert_eq!(details.response.publishedfiledetails[1].title, None);
    }
}
