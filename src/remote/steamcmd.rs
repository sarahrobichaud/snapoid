//! Remote metadata reads driven through the steamcmd CLI. The plumbing
//! (locate, run, capture) lives in [`crate::steamcmd`]; this module
//! decides what to ask Steam and how to read the response.
//!
//! The facts provided here (the current buildid, depot manifest IDs) are
//! what the Web API either gates behind a key or does not expose.

use std::path::Path;

use anyhow::{Context, Result};

use crate::steamcmd;

/// Query the current build ID of an app's public branch — a cheap
/// metadata read, no download. Used to detect whether a `latest`
/// download is already stored.
pub fn current_buildid(program: &Path, app_id: u32) -> Result<u64> {
    let output = app_info(program, app_id)?;
    parse_buildid(&output).with_context(|| {
        format!("no public-branch buildid for app {app_id} in app_info_print output")
    })
}

/// Query the manifest ID of a depot's public branch — the value the
/// registry records for pinned `download_depot` downloads.
pub fn public_manifest(program: &Path, app_id: u32, depot: u64) -> Result<u64> {
    let output = app_info(program, app_id)?;
    parse_manifest(&output, depot).with_context(|| {
        format!("no public-branch manifest for depot {depot} of app {app_id} in app_info_print output")
    })
}

/// Run `+app_info_print <app_id>` as anonymous and return the raw output.
fn app_info(program: &Path, app_id: u32) -> Result<String> {
    let args = [
        "+login".to_owned(),
        "anonymous".to_owned(),
        "+app_info_print".to_owned(),
        app_id.to_string(),
        "+quit".to_owned(),
    ];
    steamcmd::capture(program, &args)
}

/// Extract the public branch's buildid from `app_info_print` output:
///
/// ```text
/// "branches"
/// {
///     "public"
///     {
///         "buildid"    "17774562"
/// ```
fn parse_buildid(output: &str) -> Option<u64> {
    value_at_path(output, &["branches", "public", "buildid"])?
        .parse()
        .ok()
}

/// Extract the public branch's manifest ID for `depot` from
/// `app_info_print` output:
///
/// ```text
/// "depots"
/// {
///     "108604"
///     {
///         "manifests"
///         {
///             "public"    "2378094061158458897"
/// ```
fn parse_manifest(output: &str, depot: u64) -> Option<u64> {
    let depot_key = depot.to_string();
    let path = ["depots", depot_key.as_str(), "manifests", "public"];
    value_at_path(output, &path)?.parse().ok()
}

/// Return the value of the leaf whose key path ends with `path`.
///
/// `app_info_print` emits a quoted-key tree whose top levels vary (an
/// appid or "appinfo" wrapper), so callers name the path relative to the
/// nearest unambiguous ancestor. Nesting is tracked with a stack of
/// section keys:
///
/// ```text
/// "appinfo"
/// {
///     "branches"                  <- pushed
///     {
///         "public"                <- pushed
///         {
///             "buildid" "17774562"  <- leaf at [.., "branches", "public", "buildid"]
/// ```
fn value_at_path(output: &str, path: &[&str]) -> Option<String> {
    let mut stack: Vec<&str> = Vec::new();
    for raw in output.lines() {
        let line = raw.trim();
        match line {
            "{" => continue,
            "}" => {
                stack.pop();
                continue;
            }
            _ => {}
        }
        let mut tokens = line.split_whitespace();
        let Some(key_token) = tokens.next() else {
            continue;
        };
        if !(key_token.starts_with('"') && key_token.ends_with('"')) {
            continue; // prompt/banner noise between quoted sections
        }
        let key = key_token.trim_matches('"');
        match tokens.next() {
            // Section header; its "{" opens on this or a later line.
            None | Some("{") => stack.push(key),
            // Leaf; the last `path.len()` names from the root must match.
            Some(value) => {
                let mut full: Vec<&str> = stack.clone();
                full.push(key);
                if full.ends_with(path) {
                    return Some(value.trim_matches('"').to_owned());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Representative `app_info_print` output: an appid wrapper, the
    /// branches tree, and the depots tree with two depots' public
    /// manifests plus an encrypted branch that must not match.
    const APP_INFO: &str = r#""108600"
{
    "appinfo"
    {
        "common"
        {
            "name"    "Project Zomboid"
        }
        "branches"
        {
            "public"
            {
                "buildid"    "17774562"
                "timeupdated"    "1700000000"
            }
            "unstable"
            {
                "buildid"    "999"
            }
        }
        "depots"
        {
            "108600"
            {
                "manifests"
                {
                    "public"    "1111111111111111111"
                }
            }
            "108604"
            {
                "maxsize"    "4294967296"
                "manifests"
                {
                    "public"    "2378094061158458897"
                }
                "encryptedmanifests"
                {
                    "beta"
                    {
                        "encryptedmanifest"    "deadbeef"
                    }
                }
            }
        }
    }
}
"#;

    #[test]
    fn parses_public_branch_buildid() {
        assert_eq!(parse_buildid(APP_INFO), Some(17_774_562));
        assert_eq!(parse_buildid("no branches here"), None);
    }

    #[test]
    fn parses_depot_public_manifest() {
        assert_eq!(
            parse_manifest(APP_INFO, 108604),
            Some(2_378_094_061_158_458_897)
        );
        // Only the requested depot's manifest matches.
        assert_eq!(
            parse_manifest(APP_INFO, 108600),
            Some(1_111_111_111_111_111_111)
        );
        assert_eq!(parse_manifest(APP_INFO, 999_999), None);
    }

    #[test]
    fn tolerates_steamcmd_noise() {
        let noisy = format!("Redirecting stderr...\n{APP_INFO}Steam> ");
        assert_eq!(parse_buildid(&noisy), Some(17_774_562));
        assert_eq!(
            parse_manifest(&noisy, 108604),
            Some(2_378_094_061_158_458_897)
        );
    }
}
