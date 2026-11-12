//! Launching game instances with isolated profiles.
//!
//! The game resolves its user-data dir (the "Zomboid dir") from the
//! `-cachedir` launch argument (ZomboidFileSystem.setCacheDir; verified
//! against 42.22.2 and documented on the wiki's Startup parameters
//! page). Pointing it at a per-instance directory gives every instance
//! a private profile, and an empty directory is a fresh profile: the
//! game generates everything it needs for its own version on first
//! launch, so nothing is ever hand-generated here — new game versions
//! are handled by the game itself, not by this tool.
//!
//! The launch lines replicate the game's own .bat files
//! (ProjectZomboid64.bat / ProjectZomboidServer.bat), with the working
//! directory set to the instance's linked game folder.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result};

use crate::platform;
use crate::store::GameInstance;

/// Everything needed to spawn one game process.
pub struct LaunchPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

/// How to launch an instance.
pub struct LaunchOptions {
    /// Launch the dedicated server instead of the client.
    pub server: bool,
    /// Pass `-nosteam` (disables Steam integration, e.g. for tests).
    pub nosteam: bool,
    /// Server admin password. Given explicitly, it is always applied
    /// (changing the password); otherwise the default is used on the
    /// server's first launch only, and the password persists after.
    pub admin_password: Option<String>,
}

/// Admin password used when none was given and the server has none yet.
pub const DEFAULT_ADMIN_PASSWORD: &str = "snapoid";

/// Build the launch plan for `instance`. The instance profile directory
/// is created if missing, so legacy instances (without one) launch too.
pub fn plan(instance: &GameInstance, options: &LaunchOptions) -> Result<LaunchPlan> {
    std::fs::create_dir_all(&instance.profile_dir).with_context(|| {
        format!(
            "creating instance profile {}",
            instance.profile_dir.display()
        )
    })?;

    let program = java_in(&instance.game_dir)
        .with_context(|| format!("no bundled JRE in {}", instance.game_dir.display()))?;
    let cachedir = host_path(&instance.profile_dir)?;
    let sep = classpath_separator(&program);
    let cwd = canonical_cwd(&instance.game_dir);

    let mut args = if options.server {
        server_args(sep, &cachedir)
    } else {
        client_args(sep, &cachedir, options.nosteam)
    };
    if options.server {
        // A server whose UDP port is already taken binds nothing yet
        // still logs "listening" and runs its loop — deaf but alive.
        // Probe the port and fail with a clear error instead.
        let port = server_port(&instance.profile_dir);
        match std::net::UdpSocket::bind(("0.0.0.0", port)) {
            Ok(probe) => drop(probe),
            Err(error) => anyhow::bail!(
                "UDP port {port} is already in use ({error}); the server \
                 would start without a socket. Stop the other server first, \
                 or change DefaultPort in the instance's servertest.ini"
            ),
        }

        // Set the admin password when the server has none yet: a fresh
        // profile prompts for one on stdin, which would hang invisibly
        // now that the game output is suppressed. An explicit password
        // always applies, so it can also change an existing one.
        let explicit = options.admin_password.as_deref();
        if explicit.is_some() || !admin_exists(&instance.profile_dir) {
            let password = explicit.unwrap_or(DEFAULT_ADMIN_PASSWORD);
            args.push("-adminpassword".to_owned());
            args.push(password.to_owned());
        }
    }
    if options.nosteam {
        args.push("-nosteam".to_owned());
    }

    Ok(LaunchPlan {
        program,
        args,
        cwd,
    })
}

/// Run the game attached to this console and block until it exits.
/// The game's output is suppressed.
///
/// The child shares snapoid's console, so the console itself delivers
/// Ctrl+C to the game, and closing the terminal closes the game too
/// (verified on native Windows; under WSL a closed terminal may leave
/// the game running — accepted edge case).
pub fn run(plan: &LaunchPlan) -> Result<std::process::ExitStatus> {
    let mut child = Command::new(&plan.program)
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawning {}", plan.program.display()))?;

    let status = child
        .wait()
        .with_context(|| format!("waiting for {}", plan.program.display()))?;

    Ok(status)
}

/// The instance game folder in canonical form: junctions and symlinks
/// resolved, without the verbatim `\\?\` prefix canonicalize adds on
/// Windows.
///
/// The game validates file paths against its working directory
/// (ZomboidFileSystem.validatePrefix) but canonicalizes the files it
/// finds — so a junction CWD makes every model load fail ("Invalid
/// prefix found for ...") and the server terminate on a missing
/// animation asset. WSL interop resolves the junction and masks this;
/// native Windows does not.
fn canonical_cwd(game_dir: &Path) -> PathBuf {
    let canonical = game_dir
        .canonicalize()
        .unwrap_or_else(|_| game_dir.to_path_buf());
    #[cfg(windows)]
    let canonical = match canonical.to_string_lossy().strip_prefix(r"\\?\") {
        Some(stripped) => PathBuf::from(stripped.to_owned()),
        None => canonical,
    };
    canonical
}

/// The bundled JRE launcher: java.exe (Windows depot) or java (native).
fn java_in(game_dir: &Path) -> Option<PathBuf> {
    let bin = game_dir.join("jre64").join("bin");
    for name in ["java.exe", "java"] {
        let candidate = bin.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Client launch line, from ProjectZomboid64.bat.
fn client_args(sep: &str, cachedir: &str, nosteam: bool) -> Vec<String> {
    let mut args = vec![
        "-Djava.awt.headless=true".to_owned(),
        "--enable-native-access=ALL-UNNAMED".to_owned(),
        "--add-exports=java.base/jdk.internal.misc=ALL-UNNAMED".to_owned(),
        "-Dzomboid.steam=1".to_owned(),
        "-Dzomboid.znetlog=1".to_owned(),
        "-XX:-CreateCoredumpOnCrash".to_owned(),
        "-XX:-OmitStackTraceInFastThrow".to_owned(),
        "-Xmx3072m".to_owned(),
        format!("-Djava.library.path=./win64/{sep}./"),
        "-cp".to_owned(),
        format!(".{sep}projectzomboid.jar"),
        "zombie.gameStates.MainScreenState".to_owned(),
        format!("-cachedir={cachedir}"),
    ];
    if nosteam {
        args.push("-nosteam".to_owned());
    }
    args
}

/// Dedicated server launch line, from ProjectZomboidServer.bat, plus
/// `-Dzomboid.steam=1` from the client bat: without it the server
/// starts steamless ("*** Steam is not enabled") and Steam clients
/// cannot see or join it. (The password is added by [`plan`] when
/// needed.)
fn server_args(sep: &str, cachedir: &str) -> Vec<String> {
    vec![
        "--enable-native-access=ALL-UNNAMED".to_owned(),
        "--add-exports=java.base/jdk.internal.misc=ALL-UNNAMED".to_owned(),
        "-Dzomboid.steam=1".to_owned(),
        "-XX:+UseZGC".to_owned(),
        "-XX:-CreateCoredumpOnCrash".to_owned(),
        "-XX:-OmitStackTraceInFastThrow".to_owned(),
        "-Xmx3072m".to_owned(),
        format!("-Djava.library.path=./natives/{sep}./natives/win64/{sep}./"),
        "-cp".to_owned(),
        format!(".{sep}projectzomboid.jar"),
        "zombie.network.GameServer".to_owned(),
        format!("-cachedir={cachedir}"),
    ]
}

/// True when the instance's server db already contains the default
/// admin user. The db file is created before the admin prompt, so its
/// existence alone proves nothing; the username is only written when
/// the user is actually created (verified: the db of a run that
/// crashed at the prompt contains no "admin" at all).
fn admin_exists(profile_dir: &Path) -> bool {
    let db = profile_dir.join("db").join("servertest.db");
    std::fs::read(&db)
        .map(|bytes| {
            bytes
                .windows(b"admin".len())
                .any(|window| window == b"admin")
        })
        .unwrap_or(false)
}

/// The server's UDP port: DefaultPort from the profile's servertest.ini
/// (written by the game on first launch), else the game default.
fn server_port(profile_dir: &Path) -> u16 {
    const DEFAULT_PORT: u16 = 16261;
    let ini = profile_dir.join("Server").join("servertest.ini");
    std::fs::read_to_string(&ini)
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.trim_start().starts_with("DefaultPort="))
                .and_then(|line| line.split('=').nth(1))
                .and_then(|value| value.trim().parse().ok())
        })
        .unwrap_or(DEFAULT_PORT)
}

/// Classpath / library-path separator of the JVM being launched: the
/// Windows-bundled java.exe expects `;`, a native Linux `java` expects
/// `:`.
fn classpath_separator(program: &Path) -> &'static str {
    if program
        .file_name()
        .is_some_and(|n| n.to_string_lossy().ends_with(".exe"))
    {
        ";"
    } else {
        ":"
    }
}

/// Format a path for the JVM being launched. Under WSL the Windows
/// java.exe needs the Windows form (C:\...); everywhere else the path
/// is used as-is.
fn host_path(path: &Path) -> Result<String> {
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

    fn game_dir_with(java: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("snapoid-launch-{}", std::process::id()));
        let dir = base.join(java.replace(['/', ':'], "_"));
        std::fs::create_dir_all(dir.join("jre64/bin")).unwrap();
        std::fs::write(dir.join("jre64/bin").join(java), "jvm").unwrap();
        dir
    }

    #[test]
    fn client_args_match_bat_and_redirect_cachedir() {
        let args = client_args(";", r"C:\store\instance\p\profile", false);
        let text = args.join(" ");
        assert!(text.contains("-cp .;projectzomboid.jar"));
        assert!(text.contains("-Djava.library.path=./win64/;./"));
        assert!(text.contains("zombie.gameStates.MainScreenState"));
        assert!(text.contains(r"-cachedir=C:\store\instance\p\profile"));
        assert!(!text.contains("-nosteam"));

        let with = client_args(";", "x", true);
        assert!(with.last().is_some_and(|a| a == "-nosteam"));
    }

    #[test]
    fn server_args_match_bat() {
        let args = server_args(";", r"C:\p");
        let text = args.join(" ");
        assert!(text.contains("zombie.network.GameServer"));
        assert!(text.contains("-Dzomboid.steam=1"), "server must be Steam-enabled");
        assert!(text.contains("-XX:+UseZGC"));
        assert!(text.contains("-Djava.library.path=./natives/;./natives/win64/;./"));
        assert!(text.contains("-cachedir=C:\\p"));
        // The password is added by plan(), not here.
        assert!(!text.contains("-adminpassword"));
    }

    #[test]
    fn reads_server_port_from_ini() {
        let base = std::env::temp_dir().join(format!("snapoid-port-{}", std::process::id()));
        let profile = base.join("profile");

        // No ini yet (fresh profile): game default.
        assert_eq!(server_port(&profile), 16261);

        std::fs::create_dir_all(profile.join("Server")).unwrap();
        std::fs::write(
            profile.join("Server/servertest.ini"),
            "DefaultPort=16261\nUDPPort=16262\n",
        )
        .unwrap();
        assert_eq!(server_port(&profile), 16261);

        std::fs::write(profile.join("Server/servertest.ini"), "DefaultPort=17000\n").unwrap();
        assert_eq!(server_port(&profile), 17000);

        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn detects_existing_server_admin() {
        let base = std::env::temp_dir().join(format!("snapoid-admin-{}", std::process::id()));
        let profile = base.join("profile");

        // No db yet: fresh profile, no admin.
        assert!(!admin_exists(&profile));

        // The db is created before the admin prompt; a run that crashed
        // there leaves no admin string behind.
        std::fs::create_dir_all(profile.join("db")).unwrap();
        std::fs::write(profile.join("db/servertest.db"), b"no user here").unwrap();
        assert!(!admin_exists(&profile));

        // A created admin user writes its name into the db.
        std::fs::write(profile.join("db/servertest.db"), b"users\x00admin\x00hash").unwrap();
        assert!(admin_exists(&profile));

        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn server_password_set_on_first_launch_only() {
        let base = std::env::temp_dir().join(format!("snapoid-pw-{}", std::process::id()));
        let game = base.join("game");
        std::fs::create_dir_all(game.join("jre64/bin")).unwrap();
        std::fs::write(game.join("jre64/bin/java.exe"), "jvm").unwrap();
        let profile = base.join("profile");
        let instance = GameInstance {
            name: "t".to_owned(),
            version: "42.22.2".to_owned(),
            revision: None,
            dir: base.join("inst"),
            game_dir: game,
            profile_dir: profile.clone(),
            created: 0,
        };
        let opts = |password| LaunchOptions {
            server: true,
            nosteam: true,
            admin_password: password,
        };
        let password_of = |plan: &LaunchPlan| {
            let index = plan
                .args
                .iter()
                .position(|a| a == "-adminpassword")
                .expect("-adminpassword present");
            plan.args[index + 1].clone()
        };

        // First launch, no flag: default password.
        let launched = plan(&instance, &opts(None)).unwrap();
        assert_eq!(password_of(&launched), DEFAULT_ADMIN_PASSWORD);

        // Db exists but has no admin yet (crashed first run): still set.
        std::fs::create_dir_all(profile.join("db")).unwrap();
        std::fs::write(profile.join("db/servertest.db"), b"no user").unwrap();
        let launched = plan(&instance, &opts(None)).unwrap();
        assert_eq!(password_of(&launched), DEFAULT_ADMIN_PASSWORD);

        // Admin exists, no flag: password persists, nothing passed.
        std::fs::write(profile.join("db/servertest.db"), b"users\x00admin").unwrap();
        let launched = plan(&instance, &opts(None)).unwrap();
        assert!(!launched.args.contains(&"-adminpassword".to_owned()));

        // Explicit flag always applies, even with an admin present.
        let launched = plan(&instance, &opts(Some("secret".to_owned()))).unwrap();
        assert_eq!(password_of(&launched), "secret");

        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn separator_follows_jvm_flavor() {
        assert_eq!(classpath_separator(Path::new("/g/jre64/bin/java.exe")), ";");
        assert_eq!(classpath_separator(Path::new("/g/jre64/bin/java")), ":");
    }

    #[test]
    fn finds_bundled_jre() {
        let dir = game_dir_with("java.exe");
        assert_eq!(
            java_in(&dir),
            Some(dir.join("jre64/bin/java.exe")),
            "java.exe is preferred over java"
        );
        std::fs::remove_file(dir.join("jre64/bin/java.exe")).unwrap();
        assert_eq!(java_in(&dir), None, "a native java is not in a Windows depot");

        std::fs::write(dir.join("jre64/bin/java"), "jvm").unwrap();
        assert_eq!(java_in(&dir), Some(dir.join("jre64/bin/java")));
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }
}
