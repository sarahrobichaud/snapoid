//! Remote metadata reads driven through the steamcmd CLI. The plumbing
//! (locate, run, capture) lives in [`crate::steamcmd`]; this module
//! decides what to ask Steam and how to read the response.
//!
//! The facts provided here (the current buildid, depot manifest IDs) are
//! what the Web API either gates behind a key or does not expose.

use anyhow::{Context as _, Result};

use crate::parser::lenient;
use crate::steamcmd::SteamCmd;

/// Query the current build ID of an app's public branch — a cheap
/// metadata read, no download. Used to detect whether a `latest`
/// download is already stored.
pub fn current_buildid(cmd: &dyn SteamCmd, app_id: u32) -> Result<u64> {
    let output = app_info(cmd, app_id)?;
    parse_buildid(&output).with_context(|| {
        format!("no public-branch buildid for app {app_id} in app_info_print output")
    })
}

/// Query the manifest ID of a depot's public branch — the value the
/// registry records for pinned `download_depot` downloads.
pub fn public_manifest(cmd: &dyn SteamCmd, app_id: u32, depot: u64) -> Result<u64> {
    let output = app_info(cmd, app_id)?;
    parse_manifest(&output, depot).with_context(|| {
        format!(
            "no public-branch manifest for depot {depot} of app {app_id} in app_info_print output"
        )
    })
}

/// Query the current public buildid and depot manifest in a single
/// `app_info_print` pass — callers checking both (the `latest` skip
/// logic) avoid spinning steamcmd up twice.
pub fn public_build(cmd: &dyn SteamCmd, app_id: u32, depot: u64) -> Result<(u64, u64)> {
    let output = app_info(cmd, app_id)?;
    let buildid = parse_buildid(&output).with_context(|| {
        format!("no public-branch buildid for app {app_id} in app_info_print output")
    })?;
    let manifest = parse_manifest(&output, depot).with_context(|| {
        format!(
            "no public-branch manifest for depot {depot} of app {app_id} in app_info_print output"
        )
    })?;
    Ok((buildid, manifest))
}

/// Run `+app_info_print <app_id>` as anonymous and return the raw output.
fn app_info(cmd: &dyn SteamCmd, app_id: u32) -> Result<String> {
    let args = [
        "+login".to_owned(),
        "anonymous".to_owned(),
        "+app_info_print".to_owned(),
        app_id.to_string(),
        "+quit".to_owned(),
    ];
    cmd.capture(&args)
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
    lenient::value_at_path(output, &["branches", "public", "buildid"])?
        .parse()
        .ok()
}

/// Extract the public branch's manifest ID for `depot` from
/// `app_info_print` output. Current appinfo nests the GID one level
/// deeper than the old flat form:
///
/// ```text
/// "depots"
/// {
///     "108604"
///     {
///         "manifests"
///         {
///             "public"
///             {
///                 "gid"    "2378094061158458897"
/// ```
fn parse_manifest(output: &str, depot: u64) -> Option<u64> {
    let depot_key = depot.to_string();
    let path = ["depots", depot_key.as_str(), "manifests", "public", "gid"];
    lenient::value_at_path(output, &path)?.parse().ok()
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
                "buildid"    "17774563"
            }
        }
        "depots"
        {
            "108600"
            {
                "manifests"
                {
                    "public"
                    {
                        "gid"    "1111111111111111111"
                    }
                }
            }
            "108604"
            {
                "manifests"
                {
                    "public"
                    {
                        "gid"    "2378094061158458897"
                    }
                }
                "encryptedmanifests"
                {
                    "beta"
                    {
                        "gid"    "2222222222222222222"
                    }
                }
            }
        }
    }
}
"#;

    #[test]
    fn parses_public_buildid() {
        assert_eq!(parse_buildid(APP_INFO), Some(17_774_562));
        assert_eq!(parse_buildid("no branches here"), None);
    }

    #[test]
    fn parses_public_manifest() {
        assert_eq!(
            parse_manifest(APP_INFO, 108604),
            Some(2_378_094_061_158_458_897)
        );
        assert_eq!(parse_manifest(APP_INFO, 999999), None);
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
