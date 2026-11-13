//! Environment-based configuration.

use std::path::PathBuf;

/// Registry file location; override with YAPZM_REGISTRY_URL.
const DEFAULT_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/sarahrobichaud/yapzm/main/registry.json";

/// Path to a steamcmd binary overriding discovery (`YAPZM_STEAMCMD`).
pub fn steamcmd_override() -> Option<PathBuf> {
    std::env::var_os("YAPZM_STEAMCMD").map(PathBuf::from)
}

/// URL of the version registry (authoritative source of version → depot
/// manifest mappings).
pub fn registry_url() -> String {
    std::env::var("YAPZM_REGISTRY_URL").unwrap_or_else(|_| DEFAULT_REGISTRY_URL.to_owned())
}

/// Store root override (`YAPZM_STORE`), bypassing store discovery.
pub fn store_root_override() -> Option<PathBuf> {
    std::env::var_os("YAPZM_STORE").map(PathBuf::from)
}

/// Name of the app data directory yapzm owns under the platform data
/// dir (and under the Windows profile dir when running under WSL).
///
/// Resolution order:
///
/// 1. `YAPZM_APP_DIR`, when set — an explicit pin (e.g. `yapzm-dev`
///    or `yapzm-test`).
/// 2. The executable's file stem, when it names a variant: a *copy* of
///    the binary (or a `cargo run --bin` target) named `yapzm-dev`,
///    `yapzm-test`, and so on, uses that variant's store, with nothing
///    to configure per invocation.
/// 3. `yapzm`.
pub fn app_dir_name() -> String {
    if let Some(name) = std::env::var_os("YAPZM_APP_DIR") {
        let name = name.to_string_lossy().trim().to_owned();
        if !name.is_empty() {
            return name;
        }
    }
    variant_from_exe().unwrap_or_else(|| "yapzm".to_owned())
}

/// The app dir named by the current executable, via its file stem.
fn variant_from_exe() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let stem = exe.file_stem()?.to_string_lossy().into_owned();
    stem_app_dir(&stem)
}

/// Names the shipped binary goes by.
const BINARY_NAMES: [&str; 1] = ["yapzm"];

/// The app dir named by an executable file stem: `<binary>` maps to
/// `yapzm`, `<binary>-<variant>` to `yapzm-<variant>`; `None` for
/// anything else, so a renamed copy falls back to the default rather
/// than inventing a store.
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
