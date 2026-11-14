//! Game version downloader: fetches builds straight into the store's
//! `game/` dirs, so the multi-gigabyte files never get copied twice.
//!
//! Two paths:
//! - "latest": `+app_update` with `force_install_dir` pointing at a
//!   provisional `.downloading` dir in the store (no manifest needed);
//!   once the probe reports the version, the dir is renamed into place.
//! - pinned version: `download_depot` with the manifest ID from the
//!   registry. steamcmd ignores `force_install_dir` for depots and drops
//!   the files in its own `steamapps/downloading/<appid>/<depot>` dir,
//!   which is then moved into `game/<version>/`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

// Only the unix branch of path_arg maps paths for the Windows steamcmd.
#[cfg(unix)]
use crate::platform;
use crate::pz::{self, appmanifest, probe};
use crate::registry;
use crate::remote;
use crate::steamcmd::SteamCmd;
use crate::store::{Store, StoredGame};
use crate::{debug, info, warn};

/// The result of a successful download request, pointing at the stored
/// version.
pub struct Downloaded {
    pub version: String,
    pub revision: Option<String>,
    pub dir: PathBuf,
    /// False when the version was already in the store and the download
    /// was skipped.
    pub fresh: bool,
}

/// A download request.
pub struct DownloadRequest {
    /// Version to download ("latest" or a version string from the
    /// registry).
    pub version: String,
    /// Steam account name for steamcmd (password prompted by steamcmd).
    pub login: Option<String>,
    /// Depot ID override (default: the registry entry's depot, else the
    /// Windows depot).
    pub depot: Option<u64>,
    /// Download even if the version is already stored.
    pub force: bool,
    /// The live game install, when known; used to match stored versions
    /// against the current build.
    pub live_game_dir: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "game downloads need a Steam account that owns Project Zomboid; \
         run `snapoid auth login <username>` once, or pass --login \
         <username> (steamcmd prompts for the password, or reuses its \
         cached credentials)"
    )]
    LoginRequired,
}

/// Download `request.version` into the store.
pub fn download(
    store: &Store,
    cmd: &dyn SteamCmd,
    request: &DownloadRequest,
) -> Result<Downloaded> {
    if request.version.eq_ignore_ascii_case("latest") {
        download_latest(
            store,
            cmd,
            request.login.as_deref(),
            request.force,
            request.live_game_dir.as_deref(),
        )
    } else {
        download_pinned(store, cmd, request)
    }
}

/// Download the current live build via `+app_update` into a provisional
/// dir in the store, probe it for its version, and rename it into place.
/// A cheap check first: when the current public build (by buildid or
/// depot manifest) is already stored, the multi-gigabyte download is
/// skipped. The manifest check also covers branch reverts, where an old
/// pinned download becomes the current build again.
fn download_latest(
    store: &Store,
    cmd: &dyn SteamCmd,
    login: Option<&str>,
    force: bool,
    live_game_dir: Option<&Path>,
) -> Result<Downloaded> {
    let current = match remote::public_build(cmd, pz::APP_ID, pz::WINDOWS_DEPOT) {
        Ok((buildid, manifest)) => Some((buildid, manifest)),
        Err(error) => {
            warn!("could not query the current build ({error:#}); downloading anyway");
            None
        }
    };
    if !force && let Some((buildid, manifest)) = current {
        let existing = store
            .list_games()?
            .into_iter()
            .find(|game| game.buildid == Some(buildid) || game.manifest == Some(manifest));
        if let Some(existing) = existing {
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
        if let Some(live) = live_game_dir
            && let Some(existing) = live_install_match(store, live, buildid)?
        {
            return Ok(Downloaded {
                version: existing.version,
                revision: existing.revision,
                dir: existing.dir,
                fresh: false,
            });
        }
    }

    let login = require_login(login)?;

    let staging = store.games_dir().join(".downloading");
    if staging.exists() {
        fs::remove_dir_all(&staging).with_context(|| format!("clearing {}", staging.display()))?;
    }
    fs::create_dir_all(&staging).with_context(|| format!("creating {}", staging.display()))?;

    let mut args = vec![
        "+force_install_dir".to_owned(),
        path_arg(&staging)?,
        "+login".to_owned(),
        login.to_owned(),
    ];
    args.extend([
        "+app_update".to_owned(),
        pz::APP_ID.to_string(),
        "validate".to_owned(),
        "+quit".to_owned(),
    ]);
    cmd.run(&args)?;

    let info = probe::detect_dir(&staging)
        .with_context(|| format!("probing the downloaded build in {}", staging.display()))?;
    let Some(version) = info.version else {
        anyhow::bail!("the downloaded build did not report a version");
    };

    let game = store.adopt_game_dir(
        &staging,
        &version,
        info.revision.as_deref(),
        appmanifest::buildid_in_steamapps(&staging.join("steamapps"), pz::APP_ID)
            .or(current.map(|(buildid, _)| buildid)),
        current.map(|(_, manifest)| manifest),
    )?;
    Ok(Downloaded {
        version: game.version,
        revision: game.revision,
        dir: game.dir,
        fresh: true,
    })
}

/// Match `buildid` against the live install: Steam keeps it updated, so
/// when the live install is at the current build, a stored version with
/// the same version string is the same build. On a match, the stored
/// version's buildid (and a missing revision) are backfilled.
fn live_install_match(
    store: &Store,
    live_game_dir: &Path,
    buildid: u64,
) -> Result<Option<StoredGame>> {
    // common -> steamapps; if the live install is not at the current
    // build (updates disabled, Steam catching up), we cannot conclude.
    let Some(steamapps) = live_game_dir.parent().and_then(Path::parent) else {
        return Ok(None);
    };
    if appmanifest::buildid_in_steamapps(steamapps, pz::APP_ID) != Some(buildid) {
        return Ok(None);
    }
    let info = probe::detect_dir(live_game_dir)?;
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

/// Download a pinned build via `download_depot` using the manifest ID
/// from the registry, then snapshot steamcmd's download dir into the
/// store.
fn download_pinned(
    store: &Store,
    cmd: &dyn SteamCmd,
    request: &DownloadRequest,
) -> Result<Downloaded> {
    let version = &request.version;
    let reg = registry::load()?;
    let entry = reg.entries.iter().find(|entry| entry.version == *version);

    if !request.force
        && let Some(existing) = store
            .list_games()?
            .into_iter()
            .find(|game| game.version == *version)
    {
        // Reconcile metadata the registry learned since the download
        // (e.g. a buildid added to the registry afterwards).
        if let Some(buildid) = entry.and_then(|entry| entry.buildid)
            && existing.buildid.is_none()
        {
            info!("recording build {buildid} for version {version}");
            store.set_game_buildid(version, existing.revision.as_deref(), buildid)?;
        }
        info!("version {version} is already stored; use --force to re-download");
        return Ok(Downloaded {
            version: existing.version,
            revision: existing.revision,
            dir: existing.dir,
            fresh: false,
        });
    }

    let login = require_login(request.login.as_deref())?;

    let entry = entry.with_context(|| {
        format!("{version} is not in the registry; see `snapoid version available`")
    })?;
    let manifest = entry.manifest;
    let depot = request.depot.or(entry.depot).unwrap_or(pz::WINDOWS_DEPOT);
    let buildid = entry.buildid;

    let mut args = vec!["+login".to_owned(), login.to_owned()];
    args.extend([
        "+download_depot".to_owned(),
        pz::APP_ID.to_string(),
        depot.to_string(),
        manifest.to_string(),
        "+quit".to_owned(),
    ]);
    // Inherited stdio: steamcmd renders its own progress and prompts
    // (password, Steam Guard) directly on the terminal. Piping its output
    // would hide those prompts (they have no trailing newline, so a
    // line-based reader never shows them and the command appears to
    // hang). Success is verified by the downloaded dir below, since
    // steamcmd's exit codes are unreliable.
    info!(
        "downloading depot {depot} (steamcmd caches it under its own \
         steamapps dir; it is moved into the store afterwards)"
    );
    cmd.run(&args)?;

    let steamcmd_dir = cmd
        .program_dir()
        .context("steamcmd has no parent directory")?;
    let Some(downloaded) = depot_download_dir(&steamcmd_dir, pz::APP_ID, depot) else {
        anyhow::bail!(
            "steamcmd did not download anything; expected it under \
             steamapps/content/app_{}/depot_{depot} or \
             steamapps/downloading/{}/{}; check its output above",
            pz::APP_ID,
            pz::APP_ID,
            depot
        );
    };
    let empty = fs::read_dir(&downloaded)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(true);
    if empty {
        anyhow::bail!(
            "steamcmd left nothing in {}; check its output above",
            downloaded.display()
        );
    }

    // The probe gives the revision fingerprint and sanity-checks that the
    // downloaded content really is the requested version.
    let revision = match probe::detect_dir(&downloaded) {
        Ok(info) => {
            if let Some(probed) = &info.version
                && probed != version
            {
                warn!("downloaded build reports version {probed}, expected {version}");
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
    let game = store.adopt_game_dir(
        &downloaded,
        version,
        revision.as_deref(),
        buildid,
        Some(manifest),
    )?;
    debug!("{} adopted into the store", game.dir.display());
    Ok(Downloaded {
        version: game.version,
        revision: game.revision,
        dir: game.dir,
        fresh: true,
    })
}

/// Where steamcmd drops `download_depot` results. Newer versions use
/// `steamapps/content/app_<appid>/depot_<depotid>`, older ones
/// `steamapps/downloading/<appid>/<depotid>`.
fn depot_download_dir(steamcmd_dir: &Path, app_id: u32, depot: u64) -> Option<PathBuf> {
    let content = steamcmd_dir
        .join("steamapps/content")
        .join(format!("app_{app_id}"))
        .join(format!("depot_{depot}"));
    if content.is_dir() {
        return Some(content);
    }
    let downloading = steamcmd_dir
        .join("steamapps/downloading")
        .join(app_id.to_string())
        .join(depot.to_string());
    downloading.is_dir().then_some(downloading)
}

/// steamcmd needs a logged-in account that owns the game for
/// `app_update` and `download_depot` — and `+login` with no arguments is
/// a usage error, not an interactive prompt, so it must be provided.
fn require_login(login: Option<&str>) -> Result<&str> {
    login
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or(Error::LoginRequired)
        .map_err(Into::into)
}

/// Format a path for steamcmd. Under WSL the Windows binary needs the
/// Windows form of the path (C:\...), everywhere else the path is used
/// as-is.
fn path_arg(path: &Path) -> Result<String> {
    #[cfg(unix)]
    if platform::is_wsl() {
        return platform::wslpath(path)
            .with_context(|| format!("converting {} to a Windows path", path.display()));
    }
    Ok(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    struct FakeCmd {
        app_info: String,
    }

    impl SteamCmd for FakeCmd {
        fn run(&self, _args: &[String]) -> Result<()> {
            Ok(())
        }

        fn capture(&self, _args: &[String]) -> Result<String> {
            Ok(self.app_info.clone())
        }

        fn program_dir(&self) -> Option<PathBuf> {
            None
        }
    }

    const APP_INFO: &str = r#""108600"
{
    "appinfo"
    {
        "branches"
        {
            "public"
            {
                "buildid"    "17774562"
            }
        }
        "depots"
        {
            "108604"
            {
                "manifests"
                {
                    "public"
                    {
                        "gid"    "2378094061158458897"
                    }
                }
            }
        }
    }
}
"#;

    #[test]
    fn skips_latest_when_buildid_already_stored() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-dl-{}", std::process::id()));
        let store = Store::open_at(&base)?;

        let incoming = base.join("incoming");
        fs::create_dir_all(&incoming)?;
        store.adopt_game_dir(&incoming, "42.22.2", None, Some(17_774_562), None)?;

        let cmd = FakeCmd {
            app_info: APP_INFO.to_owned(),
        };
        let downloaded = download(
            &store,
            &cmd,
            &DownloadRequest {
                version: "latest".to_owned(),
                login: None,
                depot: None,
                force: false,
                live_game_dir: None,
            },
        )?;
        assert!(!downloaded.fresh);
        assert_eq!(downloaded.version, "42.22.2");

        fs::remove_dir_all(&base)?;
        Ok(())
    }
}
