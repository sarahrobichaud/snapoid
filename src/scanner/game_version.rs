//! Game version detection for Project Zomboid.
//!
//! The bundled headless dedicated server prints
//! `version=<full> <revision>` to stdout within the first moments of
//! startup, before mod loading; spawning it and killing it as soon as the
//! line appears takes well under a second and yields the full version
//! without a client launch.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::scanner::steam::SteamPaths;
use crate::{debug, warn};

const PROBE_TIMEOUT: Duration = Duration::from_secs(90);

/// Where the reported version string came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionSource {
    /// Headless dedicated-server probe; full version.
    Probe,
}

#[derive(Debug, Clone, Default)]
pub struct GameVersionInfo {
    /// Best known version string, e.g. "42.22.2".
    pub version: Option<String>,
    /// Build revision fingerprint, e.g. "3d39148846".
    pub revision: Option<String>,
    pub source: Option<VersionSource>,
}

/// Detect the game version by briefly spawning the bundled headless
/// server, which prints its full version (including the hotfix digit)
/// during startup.
pub fn detect(paths: &SteamPaths) -> Result<GameVersionInfo> {
    let mut info = GameVersionInfo::default();

    let Some(game_dir) = paths.game_dir.as_deref() else {
        return Ok(info);
    };

    match probe_server(game_dir) {
        Ok(Some((version, revision))) => {
            info.version = Some(version);
            info.revision = Some(revision);
            info.source = Some(VersionSource::Probe);
        }
        Ok(None) => {
            warn!("server probe did not report a version");
        }
        Err(error) => {
            warn!("server probe failed: {error:#}");
        }
    }

    Ok(info)
}

fn probe_server(game_dir: &Path) -> Result<Option<(String, String)>> {
    let java = game_dir.join("jre64/bin/java.exe");
    debug!("probe: spawning headless server");
    let mut child = Command::new(&java)
        .args([
            "--enable-native-access=ALL-UNNAMED",
            "--add-exports=java.base/jdk.internal.misc=ALL-UNNAMED",
            // The version line is printed within the first moments of
            // server init, before mod loading, and the process is killed
            // as soon as it appears — so tune the JVM for fast boot and a
            // small footprint instead of server throughput.
            "-XX:+UseSerialGC",
            "-XX:TieredStopAtLevel=1",
            "-XX:-UsePerfData",
            "-XX:-CreateCoredumpOnCrash",
            "-XX:-OmitStackTraceInFastThrow",
            "-Xmx512m",
            "-Djava.library.path=./natives/;./natives/win64/;./",
            "-cp",
            ".;projectzomboid.jar",
            "zombie.network.GameServer",
        ])
        .current_dir(game_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning {}", java.display()))?;

    let stdout = child.stdout.take().context("no piped stdout")?;
    let stderr = child.stderr.take().context("no piped stderr")?;
    let (tx, rx) = mpsc::channel();
    let tx_err = tx.clone();
    let out_reader = std::thread::spawn(move || watch(stdout, tx));
    let err_reader = std::thread::spawn(move || watch(stderr, tx_err));

    fn watch(stream: impl Read, sender: mpsc::Sender<String>) {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                return;
            }
        }
    }

    let deadline = Instant::now() + PROBE_TIMEOUT;
    let start = Instant::now();
    let mut found = None;
    let mut lines_seen = 0usize;
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let wait = Duration::from_millis(500).min(deadline - now);
        match rx.recv_timeout(wait) {
            Ok(line) => {
                lines_seen += 1;
                if let Some(parsed) = parse_server_version_line(&line) {
                    found = Some(parsed);
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                debug!(
                    "probe: {}s elapsed, {lines_seen} log lines",
                    start.elapsed().as_secs()
                );
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = out_reader.join();
    let _ = err_reader.join();
    Ok(found)
}

/// `... version=42.22.2 3d39148846 integrity=pending demo=false`
fn parse_server_version_line(line: &str) -> Option<(String, String)> {
    let mut tokens = line.split_whitespace();
    while let Some(token) = tokens.next() {
        if let Some(version) = token.strip_prefix("version=") {
            let revision = tokens.next()?;
            if is_revision(revision) {
                return Some((version.to_owned(), revision.to_owned()));
            }
        }
    }
    None
}

fn is_revision(text: &str) -> bool {
    (8..=40).contains(&text.len())
        && text
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_server_version_line() {
        let line = "LOG  : General      f:0 st:104> version=42.22.2 3d39148846 integrity=pending demo=false";
        assert_eq!(
            parse_server_version_line(line),
            Some(("42.22.2".to_owned(), "3d39148846".to_owned()))
        );
        // "fileversion=" must not match the "version=" prefix.
        assert_eq!(parse_server_version_line("fileversion=1.0 other"), None);
    }

    /// Tolerant smoke test against a real install.
    #[test]
    fn detect_smoke() {
        let Ok(paths) = crate::scanner::steam::locate() else {
            eprintln!("no Steam install visible; skipping");
            return;
        };
        if paths.game_dir.is_none() {
            eprintln!("no game install visible; skipping");
            return;
        }
        let info = detect(&paths).unwrap();
        eprintln!("{info:?}");
        assert!(info.revision.is_some(), "version revision should be detected");
        assert!(info.version.is_some(), "version string should be detected");
    }
}
