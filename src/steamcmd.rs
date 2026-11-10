//! Component 8: SteamCMD runner.
//!
//! Locates the steamcmd executable and runs it as a child process. This is
//! the plumbing layer only: callers assemble the steamcmd command line
//! (`+force_install_dir`, `+login`, `+app_update`, `download_depot`, ...).
//!
//! SteamCMD's exit codes are unreliable — it often exits 0 even when the
//! requested operation failed — so callers that must verify success should
//! use [`capture`] and check [`reports_success`] instead of trusting the
//! process status.
//!
//! Under WSL the located binary is always the Windows steamcmd.exe: the
//! Linux steamcmd would download Linux depots into the WSL filesystem,
//! while the game files must be Windows builds on a Windows drive. Callers
//! must therefore pass Windows-style paths for arguments like
//! `+force_install_dir` (see `store::wslpath`).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;

use anyhow::{Context, Result};

use crate::debug;

/// Locate the steamcmd executable.
///
/// Checked in order: the `SNAPOID_STEAMCMD` environment variable (explicit
/// override), the Steam install directory (steamcmd is sometimes installed
/// there), every directory on `PATH`, then the usual manual-install
/// locations. Under WSL only the Windows `steamcmd.exe` is considered;
/// elsewhere the platform's native binary name applies.
pub fn locate() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("SNAPOID_STEAMCMD") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        anyhow::bail!(
            "SNAPOID_STEAMCMD points to {}, which is not a file",
            path.display()
        );
    }

    let wsl = crate::store::is_wsl();
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(steam_dir) = crate::scanner::steam::locate() {
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

    candidates
        .into_iter()
        .find(|p| p.is_file())
        .with_context(|| {
            if wsl {
                "no Windows steamcmd.exe found; install steamcmd on the \
                 Windows host (e.g. C:\\steamcmd) or set SNAPOID_STEAMCMD"
            } else {
                "steamcmd executable not found; install it or set SNAPOID_STEAMCMD"
            }
        })
}

/// Run steamcmd with `args`, inheriting stdio so the user sees SteamCMD's
/// own progress output (download percentages, update logs) live.
pub fn run(program: &Path, args: &[String]) -> Result<()> {
    debug!("steamcmd: {} {}", program.display(), args.join(" "));
    let status = command(program)
        .args(args)
        .status()
        .with_context(|| format!("running {}", program.display()))?;
    if !status.success() {
        anyhow::bail!("steamcmd exited with {status}");
    }
    Ok(())
}

/// Run steamcmd with `args`, capturing all output as a single string for
/// parsing (`app_info_print`, manifest listings, ...). stdin is closed so
/// an interactive prompt cannot hang the call.
pub fn capture(program: &Path, args: &[String]) -> Result<String> {
    debug!("steamcmd (captured): {} {}", program.display(), args.join(" "));
    let output = command(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {}", program.display()))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(text)
}

/// Run steamcmd with `args`, streaming its output to the terminal while
/// also capturing it, so callers can verify `Success!`/`Error!` lines
/// (exit codes are unreliable). stdin is inherited so interactive login
/// prompts work.
pub fn run_streaming(program: &Path, args: &[String]) -> Result<String> {
    debug!("steamcmd: {} {}", program.display(), args.join(" "));
    let mut child = command(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running {}", program.display()))?;

    let stdout = child.stdout.take().context("no piped stdout")?;
    let stderr = child.stderr.take().context("no piped stderr")?;
    let (tx, rx) = mpsc::channel::<String>();
    let tx_out = tx.clone();
    let tx_err = tx.clone();
    let out_reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            println!("{line}");
            let _ = tx_out.send(line);
        }
    });
    let err_reader = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            eprintln!("{line}");
            let _ = tx_err.send(line);
        }
    });
    drop(tx);

    let status = child
        .wait()
        .with_context(|| format!("waiting for {}", program.display()))?;
    let _ = out_reader.join();
    let _ = err_reader.join();

    let mut output = String::new();
    while let Ok(line) = rx.recv() {
        output.push_str(&line);
        output.push('\n');
    }
    if !status.success() {
        anyhow::bail!("steamcmd exited with {status}");
    }
    Ok(output)
}

/// Build the Command for steamcmd. Under WSL, Windows binaries are
/// anchored to their own directory: steamcmd misbehaves when the working
/// directory is a WSL path (it cannot map it into Windows terms), while
/// the binary's own directory is always a Windows-side path.
fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    if crate::store::is_wsl()
        && program
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        if let Some(dir) = program.parent() {
            cmd.current_dir(dir);
        }
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
        assert!(reports_success("... Success! App '380870' fully installed."));
        assert!(!reports_success("Error! Downloading depot manifest failed"));
        // The last verdict wins: a self-update success followed by a
        // failed app_update must count as a failure.
        let both = "Success! App '380870' already up to date.\nError! No subscription";
        assert!(!reports_success(both));
        assert!(!reports_success("no verdict at all"));
    }

    /// Tolerant smoke test: on machines with steamcmd installed, locate()
    /// must return an existing file.
    #[test]
    fn locate_smoke() {
        let Ok(program) = locate() else {
            eprintln!("no steamcmd install visible; skipping");
            return;
        };
        eprintln!("{}", program.display());
        assert!(program.is_file());
    }
}
