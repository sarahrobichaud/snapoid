//! SteamCMD runner: locates the executable, runs it as a child
//! process, and installs it in the first place ([`install`]). This is
//! the plumbing layer only; callers assemble the
//! steamcmd command line (`+force_install_dir`, `+login`, `+app_update`,
//! `download_depot`, ...).
//!
//! SteamCMD's exit codes are unreliable — it often exits 0 even when the
//! requested operation failed — so callers that must verify success
//! should use [`capture`] and check [`reports_success`] instead of
//! trusting the process status.
//!
//! Under WSL the located binary is always the Windows steamcmd.exe: the
//! Linux steamcmd would download Linux depots into the WSL filesystem,
//! while the game files must be Windows builds on a Windows drive.
//! Callers must therefore pass Windows-style paths for arguments like
//! `+force_install_dir` (see [`crate::platform::wslpath`]).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result};

use crate::config;
use crate::debug;
use crate::platform;

pub mod install;

/// A steamcmd executable that can be run or captured. The seam for
/// faking steamcmd in tests.
pub trait SteamCmd {
    /// Run with inherited stdio so the user sees SteamCMD's own progress
    /// output (download percentages, update logs) live.
    fn run(&self, args: &[String]) -> Result<()>;
    /// Run capturing all output as a single string for parsing
    /// (`app_info_print`, manifest listings, ...). stdin is closed so an
    /// interactive prompt cannot hang the call.
    fn capture(&self, args: &[String]) -> Result<String>;
    /// The directory containing the executable; `download_depot` drops
    /// results under it.
    fn program_dir(&self) -> Option<PathBuf>;
}

/// The real steamcmd process.
pub struct Process {
    program: PathBuf,
}

impl Process {
    pub fn new(program: PathBuf) -> Self {
        Self { program }
    }
}

impl SteamCmd for Process {
    fn run(&self, args: &[String]) -> Result<()> {
        run(&self.program, args)
    }

    fn capture(&self, args: &[String]) -> Result<String> {
        capture(&self.program, args)
    }

    fn program_dir(&self) -> Option<PathBuf> {
        self.program.parent().map(Path::to_path_buf)
    }
}

/// Locate the steamcmd executable.
///
/// Checked in order: the `SNAPOID_STEAMCMD` environment variable (explicit
/// override), the Steam install directory (steamcmd is sometimes installed
/// there), every directory on `PATH`, then the usual manual-install
/// locations. Under WSL only the Windows `steamcmd.exe` is considered;
/// elsewhere the platform's native binary name applies.
pub fn locate() -> Result<PathBuf> {
    if let Some(path) = config::steamcmd_override() {
        if path.is_file() {
            return Ok(platform::normalize_path(path));
        }
        anyhow::bail!(
            "SNAPOID_STEAMCMD points to {}, which is not a file",
            path.display()
        );
    }

    let wsl = platform::is_wsl();
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(steam_dir) = crate::steam::locate() {
        candidates.push(steam_dir.steam_dir.join("steamcmd.exe"));
    }

    // Under WSL the Windows PATH is appended to the WSL PATH (via
    // interop), so a Windows steamcmd.exe on the host PATH resolves here.
    let names: &[&str] = if wsl {
        &["steamcmd.exe"]
    } else if cfg!(windows) {
        &["steamcmd.exe", "steamcmd"]
    } else {
        &["steamcmd"]
    };
    for name in names {
        if let Some(path) = from_path(name) {
            candidates.push(path);
        }
    }

    if wsl {
        // The default location from Valve's install guide on the Windows
        // host, via the /mnt automount.
        candidates.push(PathBuf::from("/mnt/c/steamcmd/steamcmd.exe"));
    } else {
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            // Bundled with the Steam client itself.
            candidates.push(home.join(".steam/steam/linux32/steamcmd"));
            candidates.push(home.join("steamcmd/steamcmd"));
        }
        candidates.push(PathBuf::from("/usr/bin/steamcmd"));
        candidates.push(PathBuf::from("/usr/games/steamcmd"));
        if cfg!(windows) {
            candidates.push(PathBuf::from(r"C:\steamcmd\steamcmd.exe"));
        }
    }

    // Any drive's steamcmd: the installer places it on the drive
    // holding the game install, which is not necessarily C:.
    if wsl {
        if let Ok(entries) = std::fs::read_dir("/mnt") {
            for entry in entries.flatten() {
                candidates.push(entry.path().join("steamcmd/steamcmd.exe"));
            }
        }
    } else if cfg!(windows) {
        for letter in b'A'..=b'Z' {
            candidates.push(PathBuf::from(format!(
                "{}:\\steamcmd\\steamcmd.exe",
                letter as char
            )));
        }
    }

    candidates
        .into_iter()
        .find(|p| p.is_file())
        .map(platform::normalize_path)
        .with_context(|| {
            if wsl {
                "no Windows steamcmd.exe found; install steamcmd on the \
                 Windows host (e.g. C:\\steamcmd) or set SNAPOID_STEAMCMD"
            } else {
                "steamcmd executable not found; install it or set SNAPOID_STEAMCMD"
            }
        })
}

fn run(program: &Path, args: &[String]) -> Result<()> {
    debug!("steamcmd: {} {}", program.display(), args.join(" "));
    let status = command(program)
        .args(args)
        .status()
        .with_context(|| format!("running {}", program.display()))?;
    if !status.success() {
        anyhow::bail!(ExitStatusError { status });
    }
    Ok(())
}

/// An error carrying the steamcmd exit status, so callers can recognize
/// specific codes.
#[derive(Debug, thiserror::Error)]
#[error("steamcmd exited with {status}")]
pub struct ExitStatusError {
    pub status: std::process::ExitStatus,
}

/// True when the error is steamcmd's post-self-update restart request
/// (exit code 7) — expected on the first run after a fresh install.
pub fn is_update_restart(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ExitStatusError>()
        .and_then(|error| error.status.code())
        .is_some_and(|code| code == 7)
}

fn capture(program: &Path, args: &[String]) -> Result<String> {
    debug!(
        "steamcmd (captured): {} {}",
        program.display(),
        args.join(" ")
    );
    let output = command(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {}", program.display()))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(text)
}

/// Build the Command for steamcmd. Under WSL, Windows binaries are
/// anchored to their own directory: steamcmd misbehaves when the working
/// directory is a WSL path (it cannot map it into Windows terms), while
/// the binary's own directory is always a Windows-side path.
fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    if platform::is_wsl()
        && program
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
        && let Some(dir) = program.parent()
    {
        cmd.current_dir(dir);
    }
    cmd
}

/// Heuristic success check for captured steamcmd output. Scans backwards
/// for the last `Success!`/`Error!` line, since SteamCMD often exits 0
/// even after a failed operation.
pub fn reports_success(output: &str) -> bool {
    for line in output.lines().rev() {
        if line.contains("Success!") {
            return true;
        }
        if line.contains("Error!") {
            return false;
        }
    }
    false
}

/// Heuristic success check for a captured `+login <name> +quit`. The
/// verdict follows the last `Logging in user` line: success ends in `OK`
/// (`... to Steam Public...OK`), failure prints `ERROR (Invalid
/// Password)` on current steamcmd or `FAILED login with result code
/// ...` on older ones — on the same line or the next one. Later `OK`
/// lines (`Unloading Steam API...OK`) must not mask a failure, and no
/// verdict at all (a hung Steam Guard prompt) counts as failure.
pub fn login_ok(output: &str) -> bool {
    let Some(start) = output.rfind("Logging in user") else {
        return false;
    };
    for line in output[start..].lines() {
        let line = line.trim_end();
        if line.contains("ERROR (") || line.contains("FAILED login") || line.contains("Error!") {
            return false;
        }
        if line.ends_with("OK") {
            return true;
        }
    }
    false
}

/// Search `PATH` for `name`, returning the first match that is a file.
fn from_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_success_and_error_lines() {
        assert!(reports_success(
            "... Success! App '380870' fully installed."
        ));
        assert!(!reports_success("Error! Downloading depot manifest failed"));
        // The last verdict wins: a self-update success followed by a
        // failed app_update must count as a failure.
        let both = "Success! App '380870' already up to date.\nError! No subscription";
        assert!(!reports_success(both));
        assert!(!reports_success("no verdict at all"));
    }

    #[test]
    fn parses_login_verdicts() {
        // Real success output (cached credentials), as steamcmd prints
        // it after its self-update relaunch.
        let ok = "\
Steam Console Client (c) Valve Corporation - version 1794425457
Loading Steam API...OK
Logging in using cached credentials.
Logging in user 'gordon' [U:1:1234567] to Steam Public...OK
Waiting for client config...OK
Waiting for user info...OK
Unloading Steam API...OK";
        assert!(login_ok(ok));
        // Verdict on its own line after the login line.
        assert!(login_ok(
            "Loading Steam API...OK\nLogging in user 'x' to Steam Public ...\nOK"
        ));
        // Real failure output (cached credentials cleared), as
        // steamcmd prints it: the verdict is `ERROR (...)`, and the
        // trailing `Unloading Steam API...OK` must not mask it.
        let failed = "\
Loading Steam API...OK
Cached credentials not found.
password:
Proceeding with login using username/password.
Logging in user 'gordon' [U:1:1234567] to Steam Public...
ERROR (Invalid Password)
Unloading Steam API...OK";
        assert!(!login_ok(failed));
        assert!(!login_ok(
            "Logging in user 'x' to Steam Public...ERROR (Invalid Password)"
        ));
        // Older steamcmd failure format.
        assert!(!login_ok(
            "Logging in user 'x' to Steam Public ...\nFAILED login with result code Invalid Password (2)"
        ));
        assert!(!login_ok(
            "Logging in user 'x' to Steam Public...FAILED login with result code Invalid Password"
        ));
        // The last login attempt wins: a failed retry after an earlier
        // success must count as a failure.
        let retried = "\
Logging in user 'x' to Steam Public...OK
Logging in user 'x' to Steam Public ...\nFAILED login with result code Expired";
        assert!(!login_ok(retried));
        // Earlier OK lines (Steam API loading) do not count.
        assert!(!login_ok("Loading Steam API...OK\nnothing else happened"));
        assert!(!login_ok("no login attempt at all"));
    }

    /// Tolerant smoke test: on machines with steamcmd installed, locate()
    /// must return an existing file.
    #[test]
    fn locate_smoke() {
        let Ok(program) = locate() else {
            eprintln!("no steamcmd install visible; skipping");
            return;
        };
        assert!(program.is_file());
    }
}
