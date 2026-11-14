use clap::Subcommand;

use snapoid::auth;
use snapoid::steam;
use snapoid::steamcmd::{self, SteamCmd as _};
use snapoid::store;

/// Auth subcommand names, used to catch `auth login status`-style
/// mix-ups where a subcommand is typed as an account name.
const AUTH_SUBCOMMANDS: [&str; 3] = ["login", "logout", "status"];

#[derive(Subcommand)]
pub enum AuthCommand {
    /// Log in to steamcmd once and remember the account for downloads
    Login {
        /// Steam account name (default: the remembered account, if any)
        username: Option<String>,
    },
    /// Show the remembered account and whether steamcmd still accepts it
    Status,
    /// Revoke steamcmd's saved login and forget the remembered account
    Logout {
        /// Also clear steamcmd's cached credentials, dropping the Steam
        /// Guard machine auth (the next login needs an email or mobile
        /// confirmation)
        #[arg(long)]
        clear_credentials: bool,
    },
}

pub fn run(command: AuthCommand) -> anyhow::Result<()> {
    match command {
        AuthCommand::Login { username } => login(username),
        AuthCommand::Status => status(),
        AuthCommand::Logout { clear_credentials } => logout(clear_credentials),
    }
}

/// Interactive steamcmd login, remembered on verified success.
///
/// steamcmd prompts for the password and Steam Guard code on the terminal
/// (inherited stdio) and caches the credentials on success; the captured
/// re-login afterwards proves the cache took, since that is exactly what
/// downloads rely on.
fn login(username: Option<String>) -> anyhow::Result<()> {
    let program = steamcmd::locate()?;
    let cmd = steamcmd::Process::new(program);
    let root = store_root()?;

    let explicit = username.is_some();
    let name = match username {
        Some(name) => name,
        None => match auth::load(&root) {
            Some(name) => name,
            None => anyhow::bail!(
                "no account given and none remembered; pass the account \
                 name, e.g. `snapoid auth login <username>`"
            ),
        },
    };
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("the account name is empty");
    }

    // Fast path: steamcmd already has working cached credentials for
    // the account (a previous `auth login`, or a manual steamcmd login),
    // so there is nothing to re-authenticate. The probe fails fast for
    // accounts without a cache, falling through to the interactive login.
    if let Ok(output) = cmd.capture(&login_args(name))
        && steamcmd::login_ok(&output)
    {
        auth::save(&root, name)?;
        println!("steamcmd login for {name} verified (cached credentials work)");
        println!("  `snapoid version download` no longer needs --login");
        return Ok(());
    }

    // `auth login status` reads like a status query but would launch an
    // interactive password prompt for an account named "status"; catch
    // the mix-up once the probe has ruled out real cached credentials.
    if explicit && AUTH_SUBCOMMANDS.contains(&name) {
        anyhow::bail!(
            "'{name}' looks like an auth subcommand, not a Steam account \
             name; to check the remembered account, run \
             `snapoid auth status` — if the account really is named \
             '{name}', run `snapoid auth login -- {name}`"
        );
    }

    snapoid::info!("logging in to steamcmd as {name} (steamcmd prompts for the password)");
    if let Err(error) = cmd.run(&login_args(name)) {
        anyhow::bail!(
            "steamcmd login for {name} failed: {error}; check the account \
             name and password, or run `snapoid auth status` to inspect \
             the remembered account without logging in"
        );
    }

    let output = cmd.capture(&login_args(name))?;
    if !steamcmd::login_ok(&output) {
        anyhow::bail!(
            "steamcmd did not accept the credentials for {name} without \
             a password; downloads would still prompt, so the account \
             was not remembered"
        );
    }
    auth::save(&root, name)?;
    println!("steamcmd login for {name} verified and remembered");
    println!("  `snapoid version download` no longer needs --login");
    Ok(())
}

/// Revoke steamcmd's saved login information and forget the remembered
/// account, so the next login (and download) needs the password again.
/// With `clear_credentials`, steamcmd's cached credentials are also
/// cleared, dropping the Steam Guard machine auth: the next login then
/// needs an email or mobile confirmation on top of the password.
///
/// The revoke commands only act inside a session, so when the account is
/// known we log in with the cached credentials first (captured, never
/// prompting) and revoke right after. An outcome probe then confirms the
/// passwordless re-login really fails.
fn logout(clear_credentials: bool) -> anyhow::Result<()> {
    let program = steamcmd::locate()?;
    let cmd = steamcmd::Process::new(program);
    let root = store_root()?;
    let name = auth::load(&root);

    let mut revoke = revoke_args(clear_credentials);
    revoke.push("+quit".to_owned());
    match &name {
        Some(name) => {
            let mut args = vec!["+login".to_owned(), name.clone()];
            args.extend(revoke);
            cmd.capture(&args)?;
        }
        None => {
            // No remembered account; revoke whatever steamcmd has saved.
            cmd.capture(&revoke)?;
        }
    }
    auth::clear(&root)?;

    match &name {
        Some(name) => {
            let output = cmd.capture(&login_args(name))?;
            if steamcmd::login_ok(&output) {
                snapoid::debug!("probe output: {output}");
                if clear_credentials {
                    snapoid::warn!("steamcmd still accepts cached credentials for {name}");
                } else {
                    snapoid::warn!(
                        "steamcmd still accepts cached credentials for {name}; \
                         run `snapoid auth logout --clear-credentials` to \
                         also drop the Steam Guard machine auth"
                    );
                }
            }
            let next = if clear_credentials {
                "the next login needs the password and a Steam Guard \
                 confirmation (email or mobile)"
            } else {
                "the next login needs the password again"
            };
            println!("account {name} logged out; {next}");
        }
        None => {
            println!(
                "no account was remembered; asked steamcmd to revoke its \
                 saved login anyway"
            );
        }
    }
    Ok(())
}

/// The steamcmd commands that drop saved login information. With
/// `clear_credentials`, the stronger command runs first, while the
/// session is still up.
fn revoke_args(clear_credentials: bool) -> Vec<String> {
    if clear_credentials {
        vec!["+clear_credentials".to_owned(), "+logout".to_owned()]
    } else {
        vec!["+logout".to_owned()]
    }
}

/// Show the remembered account and whether its cached credentials work.
/// A pure check: it never logs in and never prompts, and it fails when
/// the account is not logged in.
fn status() -> anyhow::Result<()> {
    let program = steamcmd::locate()?;
    let cmd = steamcmd::Process::new(program);
    let root = store_root()?;

    let Some(name) = auth::load(&root) else {
        anyhow::bail!("no account remembered; run `snapoid auth login <username>`");
    };
    println!("account:  {name}");
    let ok = cmd
        .capture(&login_args(&name))
        .map(|output| steamcmd::login_ok(&output))
        .unwrap_or(false);
    if !ok {
        anyhow::bail!(
            "not logged in: steamcmd rejects the saved credentials for \
             {name}; run `snapoid auth login` to re-authenticate"
        );
    }
    println!("steamcmd: login OK (cached credentials work)");
    Ok(())
}

/// `+login <name> +quit`: no password, so steamcmd uses its cached
/// credentials, and `+quit` ends the session instead of dropping to the
/// interactive prompt.
fn login_args(name: &str) -> Vec<String> {
    vec!["+login".to_owned(), name.to_owned(), "+quit".to_owned()]
}

/// The store root, tolerating a missing Steam install (see
/// `open_store`).
fn store_root() -> anyhow::Result<std::path::PathBuf> {
    let game_dir = steam::locate().ok().and_then(|p| p.game_dir);
    store::locate_root(game_dir.as_deref())
}
