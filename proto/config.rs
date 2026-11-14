//! Environment-based configuration.

use std::path::PathBuf;

/// Registry file location; override with SNAPOID_REGISTRY_URL.
const DEFAULT_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/sarahrobichaud/snapoid/main/registry.json";

/// Path to a steamcmd binary overriding discovery (`SNAPOID_STEAMCMD`).
pub fn steamcmd_override() -> Option<PathBuf> {
    std::env::var_os("SNAPOID_STEAMCMD").map(PathBuf::from)
}

/// URL of the version registry (authoritative source of version → depot
/// manifest mappings).
pub fn registry_url() -> String {
    std::env::var("SNAPOID_REGISTRY_URL").unwrap_or_else(|_| DEFAULT_REGISTRY_URL.to_owned())
}
