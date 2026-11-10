//! Game version detection for Project Zomboid.
//!
//! Primary source: the bundled headless dedicated server prints
//! `version=<full> <revision>` to stdout within the first moments of
//! startup, before mod loading; spawning it and killing it as soon as the
//! line appears takes well under a second and yields the full version
//! without a client launch. That is faster than reading the jar (slow on
//! network/9p filesystems) and the only source that carries the hotfix
//! digit.
//!
//! If the probe fails, detection falls back to the static jar parse: the
//! jar bakes `new GameVersion(major, minor, suffix)` into
//! `zombie/core/Core.class` and the git revision into
//! `zombie/GitVersion.class`. Always fresh, but the hotfix component is
//! assembled at runtime and is not statically present, so this yields
//! e.g. "42.22" rather than "42.22.2".

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::parser::class::{self, ConstantPool};
use crate::scanner::steam::SteamPaths;

const JAR: &str = "projectzomboid.jar";
const CORE_CLASS: &str = "zombie/core/Core.class";
const GIT_VERSION_CLASS: &str = "zombie/GitVersion.class";
const GAME_VERSION_CLASS: &str = "zombie/core/GameVersion";
const PROBE_TIMEOUT: Duration = Duration::from_secs(90);

/// Where the reported version string came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionSource {
    /// Static jar parse; major.minor only.
    StaticJar,
    /// Headless dedicated-server probe; full version.
    Probe,
}

#[derive(Debug, Clone, Default)]
pub struct GameVersionInfo {
    /// Best known version string, e.g. "42.22.2".
    pub version: Option<String>,
    /// Build revision fingerprint from the jar, e.g. "3d39148846".
    pub revision: Option<String>,
    pub source: Option<VersionSource>,
}

/// Detect the game version. Primary source: the bundled headless server
/// instance — fastest (well under a second) and the only source that
/// yields the full version including the hotfix digit. If the probe fails
/// (missing JRE, game changes, etc.), falls back to the static jar parse.
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
            return Ok(info);
        }
        Ok(None) => {
            eprintln!("warning: server probe did not report a version");
        }
        Err(error) => {
            eprintln!("warning: server probe failed: {error:#}");
        }
    }

    // Fallback: the static jar parse. GitVersion.class is tiny; Core.class
    // is a multi-MB entry — slow on network/9p filesystems.
    let jar = game_dir.join(JAR);
    info.revision = match jar_class(&jar, GIT_VERSION_CLASS) {
        Ok(Some(bytes)) => match revision_from_class(&bytes) {
            Ok(revision) => revision,
            Err(error) => {
                eprintln!("warning: {error}");
                None
            }
        },
        Ok(None) => {
            eprintln!("warning: {GIT_VERSION_CLASS} not found in {}", jar.display());
            None
        }
        Err(error) => {
            eprintln!("warning: {error}");
            None
        }
    };

    info.version = match jar_class(&jar, CORE_CLASS) {
        Ok(Some(bytes)) => match version_from_core_class(&bytes) {
            Ok(version) => version,
            Err(error) => {
                eprintln!("warning: {error}");
                None
            }
        },
        Ok(None) => {
            eprintln!("warning: {CORE_CLASS} not found in {}", jar.display());
            None
        }
        Err(error) => {
            eprintln!("warning: {error}");
            None
        }
    };
    if info.version.is_some() {
        info.source = Some(VersionSource::StaticJar);
    }

    Ok(info)
}

fn jar_class(jar: &Path, entry_name: &str) -> Result<Option<Vec<u8>>> {
    let file = std::fs::File::open(jar).with_context(|| format!("opening {}", jar.display()))?;
    // Buffered: the zip crate does many small reads/seeks, which are
    // latency-bound on network/9p filesystems.
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
        .with_context(|| format!("reading {}", jar.display()))?;
    let mut bytes = Vec::new();
    match archive.by_name(entry_name) {
        Ok(mut entry) => {
            entry
                .read_to_end(&mut bytes)
                .with_context(|| format!("reading {entry_name}"))?;
            Ok(Some(bytes))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("reading {entry_name} from {}", jar.display()))
        }
    }
}

/// The revision is the sole lowercase-hex Utf8 constant in GitVersion.class.
fn revision_from_class(bytes: &[u8]) -> Result<Option<String>> {
    let pool = ConstantPool::parse(bytes)?;
    let mut found: Option<String> = None;
    for index in 1..pool.len() {
        let Some(text) = pool.utf8(index as u16) else {
            continue;
        };
        if !is_revision(text) {
            continue;
        }
        if found.as_deref().is_some_and(|previous| previous != text) {
            bail!("ambiguous revision constants in {GIT_VERSION_CLASS}");
        }
        found = Some(text.to_owned());
    }
    Ok(found)
}

fn is_revision(text: &str) -> bool {
    (8..=40).contains(&text.len())
        && text
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// The game version is compiler-emitted as
/// `new GameVersion(major, minor, suffix)` inside Core; the site that
/// stores its result into a static field (putstatic) is the game's version
/// constant, which distinguishes it from fallback constructions.
fn version_from_core_class(bytes: &[u8]) -> Result<Option<String>> {
    let pool = ConstantPool::parse(bytes)?;
    let calls = class::find_constructor_calls(bytes, &pool, GAME_VERSION_CLASS, "<init>");
    let chosen = calls
        .iter()
        .find(|call| bytes.get(call.end) == Some(&0xB3)) // putstatic
        .or_else(|| calls.last());
    let Some(call) = chosen else {
        return Ok(None);
    };
    let Some(major) = call.int_args.first().copied() else {
        return Ok(None);
    };
    let Some(minor) = call.int_args.get(1).copied() else {
        return Ok(None);
    };
    if major <= 0 {
        return Ok(None);
    }
    let suffix = call.string_arg.clone().unwrap_or_default();
    Ok(Some(format!("{major}.{minor}{suffix}")))
}

fn probe_server(game_dir: &Path) -> Result<Option<(String, String)>> {
    let java = game_dir.join("jre64/bin/java.exe");
    eprintln!("probe: spawning headless server...");
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
    let mut progress_shown = false;
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
                // Live status line; stderr keeps stdout pipeable.
                eprint!(
                    "\rprobe: {}s elapsed, {lines_seen} log lines...   ",
                    start.elapsed().as_secs()
                );
                progress_shown = true;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    if progress_shown {
        eprintln!();
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
