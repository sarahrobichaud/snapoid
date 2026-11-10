//! Component 10: Game version downloader, part of the snapshot store.
//!
//! Downloads game builds straight into the store's `game/<version>/`
//! dirs, so the multi-gigabyte files never get copied twice. Two paths:
//!
//! - "latest": `+app_update` with `force_install_dir` pointing at a
//!   provisional `.downloading` dir in the store (no manifest needed);
//!   once the probe reports the version, the dir is renamed to
//!   `game/<version>/`.
//! - pinned version: `download_depot` with the manifest ID from the
//!   registry. steamcmd ignores `force_install_dir` for depots and drops
//!   the files in its own `steamapps/downloading/<appid>/<depot>` dir,
//!   which is then moved into `game/<version>/`.

use std::path::Path;

use anyhow::Context as _;
use anyhow::Result;

use crate::parser::acf::{self, Acf};
use crate::registry;
use crate::remote;
use crate::scanner::game_version;
use crate::scanner::steam::PZ_APP_ID;
use crate::steamcmd;
use crate::{debug, info, warn};
use super::Store;

/// Project Zomboid's Windows depot, the game files snapoid snapshots.
/// Used when neither the CLI nor the registry entry specifies a depot.
const WINDOWS_DEPOT: u64 = 108604;

/// The result of a successful download request, pointing at the stored
/// version.
pub struct Downloaded {
    pub version: String,
    pub revision: Option<String>,
    pub dir: std::path::PathBuf,
    /// False when the version was already in the store and the download
    /// was skipped.
    pub fresh: bool,
}

/// Download `version` ("latest" or a version string from the registry)
/// into the store.
pub fn download(
    store: &Store,
    program: &Path,
    version: &str,
    login: Option<&str>,
    depot_override: Option<u64>,
    force: bool,
) -> Result<Downloaded> {
    if version.eq_ignore_ascii_case("latest") {
        download_latest(store, program, login, force)
    } else {
        download_pinned(store, program, version, login, depot_override, force)
    }
}

/// Download the current live build via `+app_update` into a provisional
/// dir in the store, probe it for its version, and rename it into place.
/// A cheap buildid check first: when the current public build is already
/// stored, the multi-gigabyte download is skipped.
fn download_latest(
    store: &Store,
    program: &Path,
    login: Option<&str>,
    force: bool,
) -> Result<Downloaded> {
    let current = match remote::current_buildid(program, PZ_APP_ID) {
        Ok(buildid) => Some(buildid),
        Err(error) => {
            warn!("could not query the current buildid ({error:#}); downloading anyway");
            None
        }
    };
    if !force {
        if let Some(buildid) = current {
            if let Some(existing) = store
                .list_games()?
                .into_iter()
                .find(|game| game.buildid == Some(buildid))
            {
                info!(
                    "latest (build {buildid}) is already stored as version {}; skipping",
                    existing.version
                );
                return Ok(Downloaded {
                    version: existing.version,
                    revision: existing.revision,
                    dir: existing.dir,
                    fresh: false,
                });
            }
            // Fallback for versions stored before buildid tracking: when
            // the live install is itself at the current build, a stored
            // version matching the live version and revision fingerprint
            // is the same build. The buildid is backfilled so future
            // checks hit the fast path above.
            if let Some(existing) = live_install_match(store, buildid)? {
                return Ok(Downloaded {
                    version: existing.version,
                    revision: existing.revision,
                    dir: existing.dir,
                    fresh: false,
                });
            }
        }
    }

    let login = require_login(login)?;

    let staging = store.games_dir().join(".downloading");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .with_context(|| format!("clearing {}", staging.display()))?;
    }
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("creating {}", staging.display()))?;

    let mut args = vec![
        "+force_install_dir".to_owned(),
        path_arg(&staging)?,
        "+login".to_owned(),
        login.to_owned(),
    ];
    args.extend([
        "+app_update".to_owned(),
        PZ_APP_ID.to_string(),
        "validate".to_owned(),
        "+quit".to_owned(),
    ]);
    steamcmd::run(program, &args)?;

    let info = game_version::detect_dir(&staging)
        .with_context(|| format!("probing the downloaded build in {}", staging.display()))?;
    let Some(version) = info.version else {
        anyhow::bail!("the downloaded build did not report a version");
    };

    let game = store.adopt_game_dir(
        &staging,
        &version,
        info.revision.as_deref(),
        appmanifest_buildid(&staging.join("steamapps")).or(current),
    )?;
    Ok(Downloaded {
        version: game.version,
        revision: game.revision,
        dir: game.dir,
        fresh: true,
    })
}

/// Read the buildid from a steamapps dir's appmanifest.
fn appmanifest_buildid(steamapps_dir: &Path) -> Option<u64> {
    let text =
        std::fs::read_to_string(steamapps_dir.join(format!("appmanifest_{PZ_APP_ID}.acf"))).ok()?;
    let Acf::Map(map) = acf::parse(&text).ok()? else {
        return None;
    };
    map.get("buildid")?.as_str()?.parse().ok()
}

/// Match `buildid` against the live install: Steam keeps it updated, so
/// when the live install is at the current build, a stored version with
/// the same version string is the same build. On a match, the stored
/// version's buildid (and a missing revision) are backfilled.
fn live_install_match(store: &Store, buildid: u64) -> Result<Option<super::StoredGame>> {
    let Ok(paths) = crate::scanner::steam::locate() else {
        return Ok(None);
    };
    let Some(game_dir) = paths.game_dir.as_deref() else {
        return Ok(None);
    };
    // common -> steamapps; if the live install is not at the current
    // build (updates disabled, Steam catching up), we cannot conclude.
    let Some(steamapps) = game_dir.parent().and_then(Path::parent) else {
        return Ok(None);
    };
    if appmanifest_buildid(steamapps) != Some(buildid) {
        return Ok(None);
    }
    let info = game_version::detect(&paths)?;
    let Some(version) = info.version.as_deref() else {
        return Ok(None);
    };
    let Some(existing) = store
        .list_games()?
        .into_iter()
        .find(|game| game.version == version)
    else {
        return Ok(None);
    };
    info!("marking stored version {version} as build {buildid}");
    store.set_game_buildid(version, info.revision.as_deref(), buildid)?;
    info!("latest is already stored as version {version}; skipping");
    Ok(Some(existing))
}

/// Download a pinned build via `download_depot` using the manifest ID from
/// the registry, then snapshot steamcmd's download dir into the store.
fn download_pinned(
    store: &Store,
    program: &Path,
    version: &str,
    login: Option<&str>,
    depot_override: Option<u64>,
    force: bool,
) -> Result<Downloaded> {
    if !force {
        if let Some(existing) = store
            .list_games()?
            .into_iter()
            .find(|game| game.version == version)
        {
            info!("version {version} is already stored; use --force to re-download");
            return Ok(Downloaded {
                version: existing.version,
                revision: existing.revision,
                dir: existing.dir,
                fresh: false,
            });
        }
    }

    let login = require_login(login)?;

    let reg = registry::load()?;
    let entry = reg
        .entries
        .iter()
        .find(|entry| entry.version == version)
        .with_context(|| format!("{version} is not in the registry; see `snapoid version available`"))?;
    let manifest = entry.manifest;
    let depot = depot_override.or(entry.depot).unwrap_or(WINDOWS_DEPOT);

    let mut args = vec!["+login".to_owned(), login.to_owned()];
    args.extend([
        "+download_depot".to_owned(),
        PZ_APP_ID.to_string(),
        depot.to_string(),
        manifest.to_string(),
        "+quit".to_owned(),
    ]);
    let output = steamcmd::run_streaming(program, &args)?;
    if !steamcmd::reports_success(&output) {
        anyhow::bail!("steamcmd did not report success for the depot download");
    }

    let steamcmd_dir = program
        .parent()
        .context("steamcmd has no parent directory")?;
    let downloaded = steamcmd_dir
        .join("steamapps/downloading")
        .join(PZ_APP_ID.to_string())
        .join(depot.to_string());
    if !downloaded.is_dir() {
        anyhow::bail!(
            "steamcmd reported success but {} does not exist",
            downloaded.display()
        );
    }

    // The probe gives the revision fingerprint and sanity-checks that the
    // downloaded content really is the requested version.
    let revision = match game_version::detect_dir(&downloaded) {
        Ok(info) => {
            if let Some(probed) = &info.version {
                if probed != version {
                    warn!("downloaded build reports version {probed}, expected {version}");
                }
            }
            info.revision
        }
        Err(error) => {
            warn!("could not probe the downloaded build: {error:#}");
            None
        }
    };

    // Adopting moves the downloaded dir into the store, so no separate
    // cache cleanup is needed; any sibling depot dirs steamcmd left
    // behind stay in its cache.
    let game = store.adopt_game_dir(&downloaded, version, revision.as_deref(), None)?;
    debug!("{} adopted into the store", game.dir.display());
    Ok(Downloaded {
        version: game.version,
        revision: game.revision,
        dir: game.dir,
        fresh: true,
    })
}

/// Format a path for steamcmd. Under WSL the Windows binary needs the
/// Windows form of the path (C:\...), everywhere else the path is used
/// as-is.
/// steamcmd needs a logged-in account that owns the game for
/// `app_update` and `download_depot` — and `+login` with no arguments is
/// a usage error, not an interactive prompt, so it must be provided.
fn require_login(login: Option<&str>) -> Result<&str> {
    login
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .context(
            "game downloads need a Steam account that owns Project Zomboid; \
             pass --login <username> (steamcmd prompts for the password, \
             or reuses its cached credentials)",
        )
}

fn path_arg(path: &Path) -> Result<String> {
    #[cfg(unix)]
    if super::is_wsl() {
        return super::wslpath(path)
            .with_context(|| format!("converting {} to a Windows path", path.display()));
    }
    Ok(path.display().to_string())
}
