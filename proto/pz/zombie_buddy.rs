//! ZombieBuddy: a mod that ships a Java agent (a jar) plus a native
//! loader for it. The mod's manual instructions ask for a copy of both
//! in the game folder and `-agentlib:zbNative` in the launch options;
//! snapoid automates this without touching the shared game folder more
//! than the mod's own loader requires:
//!
//! - The native loader (`zbNative.dll`) is copied into the instance's
//!   own `agent/` dir and loaded by absolute path via `-agentpath=`,
//!   which the JVM treats exactly like `-agentlib` — per instance.
//! - The agent jar is copied next to the loader. On Windows the loader
//!   resolves `ZombieBuddy.jar` relative to the game's working
//!   directory (hardcoded in its C source), so the jar must also exist
//!   in the game folder — the same thing the mod's own installer does.
//!   On a native Linux JVM no loader is needed: the jar is loaded
//!   directly via `-javaagent:` from the instance's agent dir.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::{debug, info, warn};

/// The Mod ID the game keys the mod on; also its mod folder name.
pub const MOD_ID: &str = "ZombieBuddy";

/// The `-agentlib:` name the mod's dialog asks for.
pub const AGENT_LIB: &str = "zbNative";

/// The agent jar the loader loads, and its signature sidecar (verified
/// by the agent's mod-approval system).
pub const AGENT_JAR: &str = "ZombieBuddy.jar";
pub const AGENT_JAR_SIDECAR: &str = "ZombieBuddy.jar.zbs";

/// The instance dir holding the agent files, private per instance so
/// the loader is never shared.
pub const AGENT_DIR: &str = "agent";

/// The agent files for one launch: the native loader in the instance's
/// agent dir (Windows flow), and the agent jar in the same dir (the
/// `-javaagent:` fallback for a native Linux JVM).
#[derive(Debug, Default)]
pub struct AgentPaths {
    pub library: Option<PathBuf>,
    pub jar: Option<PathBuf>,
}

/// True when a Mod ID or mod folder name is ZombieBuddy's.
pub fn is_mod_name(name: &str) -> bool {
    name.eq_ignore_ascii_case(MOD_ID)
}

/// ZombieBuddy loads before everything else when installed: its native
/// loader must be up before any other mod's code runs, regardless of
/// what the manifests declare (the game's own dialog asks for a manual
/// agent setup; snapoid automates it). Dependency resolution runs
/// first, so the promotion only overrides the tail of the order.
pub fn promote_first(mod_ids: &mut [String]) {
    if let Some(index) = mod_ids.iter().position(|id| is_mod_name(id)) {
        mod_ids[..=index].rotate_right(1);
    }
}

/// True when the mod is installed into an instance profile.
pub fn installed_in(profile_dir: &Path) -> bool {
    profile_dir.join("mods").join(MOD_ID).is_dir()
}

/// The native loader in `agent_dir`, by its known file names (Windows
/// `LoadLibrary` adds `.dll`, Unix loaders add the `lib` prefix and
/// `.so`).
pub fn agent_library(agent_dir: &Path) -> Option<PathBuf> {
    [
        "zbNative.dll",
        "zbNative.so",
        "libzbNative.so",
        "libzbNative.dylib",
    ]
    .iter()
    .map(|name| agent_dir.join(name))
    .find(|path| path.is_file())
}

/// Native libraries shipped inside an installed mod folder, searched
/// recursively.
pub fn native_libraries(mod_dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect(mod_dir, &mut found, &is_native_library);
    found
}

/// The agent jar and its signature sidecar, searched recursively.
pub fn agent_jar_files(mod_dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect(mod_dir, &mut found, &|path: &Path| {
        path.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            name.eq_ignore_ascii_case(AGENT_JAR) || name.eq_ignore_ascii_case(AGENT_JAR_SIDECAR)
        })
    });
    found
}

fn is_native_library(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext == "dll" || ext == "so" || ext == "dylib")
}

fn collect(dir: &Path, found: &mut Vec<PathBuf>, keep: &dyn Fn(&Path) -> bool) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, found, keep);
        } else if keep(&path) {
            found.push(path);
        }
    }
}

/// Copy the mod's agent files out of the installed mod folder: the
/// native loader and the agent jar (with its sidecar) into the
/// instance's agent dir, and the jar into the game folder when the
/// Windows loader flow needs it there. Existing files are replaced, so
/// re-installing the mod refreshes every copy. Returns the copied file
/// names.
pub fn install_agent(mod_dir: &Path, agent_dir: &Path, game_dir: &Path) -> Result<Vec<String>> {
    fs::create_dir_all(agent_dir).with_context(|| format!("creating {}", agent_dir.display()))?;
    let mut copied = Vec::new();

    let libraries = native_libraries(mod_dir);
    for source in &libraries {
        copied.push(copy_into(source, agent_dir)?);
    }
    let jars = agent_jar_files(mod_dir);
    for source in &jars {
        copied.push(copy_into(source, agent_dir)?);
        // The Windows loader resolves the jar relative to the game's
        // working directory; without a loader (a Linux-only mod) the
        // game folder is never touched. The copy mirrors the same file
        // name, so it adds nothing to the reported list.
        if !libraries.is_empty() {
            copy_into(source, game_dir)?;
        }
    }

    if copied.is_empty() {
        warn!("zombie buddy ships no agent files; the game will report it as not installed");
    } else {
        info!("zombie buddy agent installed: {}", copied.join(", "));
    }
    Ok(copied)
}

fn copy_into(source: &Path, dir: &Path) -> Result<String> {
    let name = source
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let target = dir.join(&name);
    fs::copy(source, &target)
        .with_context(|| format!("copying {} into {}", source.display(), target.display()))?;
    debug!("zombie buddy agent: {}", target.display());
    Ok(name)
}

/// Launch-time self-healing: the agent dir can be missing while the
/// profile keeps the mod (an install from before the agent dir existed,
/// or a wiped instance dir), and the game folder's jar copy can lag the
/// installed mod version. Re-copy from the installed mod folder as
/// needed, then return the agent paths for the launch arguments.
pub fn ensure_agent(mod_dir: &Path, agent_dir: &Path, game_dir: &Path) -> Result<AgentPaths> {
    if agent_library(agent_dir).is_none() || agent_jar_files(agent_dir).is_empty() {
        install_agent(mod_dir, agent_dir, game_dir)?;
    } else {
        // The agent dir is complete; refresh only the game folder's jar
        // copy so it matches the instance's installed mod version.
        if agent_library(agent_dir).is_some() {
            for source in agent_jar_files(mod_dir) {
                copy_into(&source, game_dir)?;
            }
        }
    }
    Ok(AgentPaths {
        library: agent_library(agent_dir),
        jar: agent_dir
            .join(AGENT_JAR)
            .is_file()
            .then_some(agent_dir.join(AGENT_JAR)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mod_fixture(base: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let mod_dir = base.join("mods/ZombieBuddy");
        let agent_dir = base.join("instance/test/agent");
        let game_dir = base.join("game");
        fs::create_dir_all(mod_dir.join("libs")).unwrap();
        fs::create_dir_all(mod_dir.join("42/media")).unwrap();
        fs::create_dir_all(&agent_dir).unwrap();
        fs::create_dir_all(&game_dir).unwrap();
        fs::write(mod_dir.join("libs/zbNative.dll"), "loader").unwrap();
        fs::write(mod_dir.join("libs/ZombieBuddy.jar"), "agent jar").unwrap();
        fs::write(mod_dir.join("libs/ZombieBuddy.jar.zbs"), "signature").unwrap();
        fs::write(mod_dir.join("42/media/buddy.lua"), "lua").unwrap();
        (mod_dir, agent_dir, game_dir)
    }

    #[test]
    fn recognizes_the_mod_by_id_or_folder_name() {
        assert!(is_mod_name("ZombieBuddy"));
        assert!(is_mod_name("zombiebuddy"));
        assert!(!is_mod_name("Zombie Buddy"));
        assert!(!is_mod_name("OtherMod"));
    }

    #[test]
    fn installs_loader_and_jar_into_the_agent_dir_and_jar_into_the_game_dir() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-zb-{}", std::process::id()));
        let (mod_dir, agent_dir, game_dir) = mod_fixture(&base);

        let copied = install_agent(&mod_dir, &agent_dir, &game_dir)?;
        assert_eq!(
            copied,
            vec!["zbNative.dll", "ZombieBuddy.jar", "ZombieBuddy.jar.zbs"]
        );
        assert_eq!(
            agent_library(&agent_dir),
            Some(agent_dir.join("zbNative.dll"))
        );
        assert_eq!(fs::read(agent_dir.join("ZombieBuddy.jar"))?, b"agent jar");
        assert_eq!(
            fs::read(agent_dir.join("ZombieBuddy.jar.zbs"))?,
            b"signature"
        );
        // The Windows loader flow: the jar (and sidecar) in the game's
        // working directory, the loader never there.
        assert_eq!(fs::read(game_dir.join("ZombieBuddy.jar"))?, b"agent jar");
        assert!(game_dir.join("ZombieBuddy.jar.zbs").is_file());
        assert!(!game_dir.join("zbNative.dll").exists());

        // Idempotent: a refresh replaces every copy.
        fs::write(mod_dir.join("libs/ZombieBuddy.jar"), "agent jar v2")?;
        install_agent(&mod_dir, &agent_dir, &game_dir)?;
        assert_eq!(
            fs::read(agent_dir.join("ZombieBuddy.jar"))?,
            b"agent jar v2"
        );
        assert_eq!(fs::read(game_dir.join("ZombieBuddy.jar"))?, b"agent jar v2");

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn a_mod_without_a_loader_never_touches_the_game_folder() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-zb-nodll-{}", std::process::id()));
        let mod_dir = base.join("mods/ZombieBuddy");
        let agent_dir = base.join("instance/test/agent");
        let game_dir = base.join("game");
        fs::create_dir_all(mod_dir.join("libs"))?;
        fs::create_dir_all(&agent_dir)?;
        fs::create_dir_all(&game_dir)?;
        fs::write(mod_dir.join("libs/ZombieBuddy.jar"), "agent jar")?;

        install_agent(&mod_dir, &agent_dir, &game_dir)?;
        assert!(agent_dir.join("ZombieBuddy.jar").is_file());
        assert!(!game_dir.join("ZombieBuddy.jar").exists());

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn a_mod_without_agent_files_yields_nothing() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-zb-empty-{}", std::process::id()));
        let mod_dir = base.join("mods/ZombieBuddy");
        let agent_dir = base.join("instance/test/agent");
        let game_dir = base.join("game");
        fs::create_dir_all(&mod_dir)?;
        fs::create_dir_all(&agent_dir)?;
        fs::create_dir_all(&game_dir)?;

        assert_eq!(agent_library(&agent_dir), None);
        let paths = ensure_agent(&mod_dir, &agent_dir, &game_dir)?;
        assert_eq!(paths.library, None);
        assert_eq!(paths.jar, None);

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn ensure_agent_self_heals_a_missing_agent_dir() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-zb-heal-{}", std::process::id()));
        let (mod_dir, agent_dir, game_dir) = mod_fixture(&base);
        fs::remove_dir_all(&agent_dir)?;

        let paths = ensure_agent(&mod_dir, &agent_dir, &game_dir)?;
        assert_eq!(paths.library, Some(agent_dir.join("zbNative.dll")));
        assert_eq!(paths.jar, Some(agent_dir.join("ZombieBuddy.jar")));
        assert!(game_dir.join("ZombieBuddy.jar").is_file());

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn ensure_agent_refreshes_a_stale_game_folder_jar() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-zb-stale-{}", std::process::id()));
        let (mod_dir, agent_dir, game_dir) = mod_fixture(&base);
        install_agent(&mod_dir, &agent_dir, &game_dir)?;

        // The installed mod updates; the game folder's copy is stale.
        fs::write(mod_dir.join("libs/ZombieBuddy.jar"), "agent jar v2")?;
        ensure_agent(&mod_dir, &agent_dir, &game_dir)?;
        assert_eq!(fs::read(game_dir.join("ZombieBuddy.jar"))?, b"agent jar v2");

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn detects_an_installed_mod_in_the_profile() -> Result<()> {
        let base = std::env::temp_dir().join(format!("snapoid-zb-prof-{}", std::process::id()));
        let profile = base.join("profile");
        assert!(!installed_in(&profile));
        fs::create_dir_all(profile.join("mods").join(MOD_ID))?;
        assert!(installed_in(&profile));
        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn promotes_zombie_buddy_to_the_front_of_the_load_order() {
        let mut order = vec!["HM".to_owned(), "ZombieBuddy".to_owned(), "PM".to_owned()];
        promote_first(&mut order);
        assert_eq!(order, vec!["ZombieBuddy", "HM", "PM"]);

        // Absent: the order is untouched.
        let mut order = vec!["HM".to_owned(), "PM".to_owned()];
        promote_first(&mut order);
        assert_eq!(order, vec!["HM", "PM"]);
    }
}
