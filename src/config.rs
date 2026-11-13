//! Environment-based configuration.

use std::path::PathBuf;

/// Explicit steamcmd binary path, overriding discovery.
pub fn steamcmd_override() -> Option<PathBuf> {
    std::env::var_os("YAPZM_STEAMCMD").map(PathBuf::from)
}

/// URL of the default version registry.
const DEFAULT_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/sarahrobichaud/yapzm/main/registry.json";

/// URL of the version registry mapping versions to depot manifests.
pub fn registry_url() -> String {
    std::env::var("YAPZM_REGISTRY_URL").unwrap_or_else(|_| DEFAULT_REGISTRY_URL.to_owned())
}

/// Explicit store root, bypassing store discovery.
pub fn store_root_override() -> Option<PathBuf> {
    std::env::var_os("YAPZM_STORE").map(PathBuf::from)
}

/// Name of yapzm's app data directory: the environment override when
/// set, else the variant named by the executable's stem, else the
/// default.
pub fn app_dir_name() -> String {
    if let Some(name) = std::env::var_os("YAPZM_APP_DIR") {
        let name = name.to_string_lossy().trim().to_owned();
        if !name.is_empty() {
            return name;
        }
    }
    variant_from_exe().unwrap_or_else(|| "yapzm".to_owned())
}

/// The app dir named by the current executable's file stem, if any.
fn variant_from_exe() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let stem = exe.file_stem()?.to_string_lossy().into_owned();
    stem_app_dir(&stem)
}

/// Names the shipped binary goes by.
const BINARY_NAMES: [&str; 1] = ["yapzm"];

/// Maps an executable stem to its app dir, or `None` when the stem isn't
/// the shipped binary (or a variant of it), so a renamed copy falls back
/// to the default rather than inventing a store.
fn stem_app_dir(stem: &str) -> Option<String> {
    for base in BINARY_NAMES {
        if let Some(variant) = stem
            .strip_prefix(base)
            .and_then(|rest| rest.strip_prefix('-'))
        {
            return Some(format!("yapzm-{variant}"));
        }
        if stem == base {
            return Some("yapzm".to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_stem_names_the_variant() {
        assert_eq!(stem_app_dir("yapzm"), Some("yapzm".to_owned()));
        assert_eq!(stem_app_dir("yapzm-dev"), Some("yapzm-dev".to_owned()));
        assert_eq!(stem_app_dir("yapzm-test"), Some("yapzm-test".to_owned()));
        assert_eq!(stem_app_dir("notyapzm-dev"), None);
        assert_eq!(stem_app_dir("cargo"), None);
    }
}
