//! Human-readable rendering of library results. No orchestration, no
//! library imports beyond data types.

use snapoid::download::Downloaded;
use snapoid::fmt;
use snapoid::pz::probe::GameVersionInfo;
use snapoid::registry::Registry;
use snapoid::scanner::workshop::{ModEntry, WorkshopItem};
use snapoid::steam::SteamPaths;
use snapoid::store::{GameInstance, StoredGame};

/// Render the workshop inventory from `scan`.
pub fn print_scan(items: &[WorkshopItem]) {
    let mut total_mods = 0;
    for item in items {
        total_mods += item.mods.len();
        let updated = if item.time_updated == 0 {
            "unknown".to_string()
        } else {
            fmt::format_date_utc(item.time_updated)
        };
        println!("\n{}  updated {}", item.id, updated);

        let name_width = item
            .mods
            .iter()
            .map(|m| m.name.chars().count())
            .max()
            .unwrap_or(0);
        if item.mods.is_empty() {
            println!("  (no mods found)");
        }
        for m in &item.mods {
            println!("  {:name_width$}  {}", m.name, version_range(m));
        }
    }

    println!(
        "\n{} workshop item{}, {} mod{}",
        items.len(),
        plural(items.len()),
        total_mods,
        plural(total_mods)
    );
}

/// The supported version range of a mod, as displayed.
fn version_range(mod_entry: &ModEntry) -> String {
    match (&mod_entry.version_min, &mod_entry.version_max) {
        (Some(min), Some(max)) if min == max => min.text().to_owned(),
        (Some(min), Some(max)) => format!("{} - {}", min.text(), max.text()),
        (Some(min), None) => format!("{}+", min.text()),
        (None, Some(max)) => format!("- {}", max.text()),
        (None, None) => "-".to_string(),
    }
}

/// Render detected game version info for the detect/probe commands.
pub fn print_version_info(info: &GameVersionInfo) {
    println!("version:  {}", info.version.as_deref().unwrap_or("unknown"));
    println!(
        "revision: {}",
        info.revision.as_deref().unwrap_or("unknown")
    );
    println!(
        "source:   {}",
        info.source
            .map(|source| format!("{source:?}"))
            .unwrap_or_else(|| "none".to_owned())
    );
}

/// Render a freshly captured game version.
pub fn print_stored_game(game: &StoredGame) {
    println!("game version {} captured", game.version);
    if let Some(revision) = &game.revision {
        println!("  revision: {revision}");
    }
    if let Some(buildid) = game.buildid {
        println!("  build:    {buildid}");
    }
    println!("  path:     {}", game.dir.display());
}

/// Render the result of a download request.
pub fn print_downloaded(downloaded: &Downloaded) {
    println!(
        "game version {} {}",
        downloaded.version,
        if downloaded.fresh {
            "downloaded"
        } else {
            "already stored"
        }
    );
    if let Some(revision) = &downloaded.revision {
        println!("  revision: {revision}");
    }
    println!("  path:     {}", downloaded.dir.display());
}

/// Render the list of stored game versions.
pub fn print_games(games: &[StoredGame]) {
    if games.is_empty() {
        println!("No game versions stored.");
        return;
    }
    for game in games {
        let captured = if game.created == 0 {
            "unknown".to_string()
        } else {
            fmt::format_date_utc(game.created)
        };
        println!(
            "{}  captured {}  {}",
            game.version,
            captured,
            game.dir.display()
        );
        if let Some(buildid) = game.buildid {
            println!("  build:    {buildid}");
        }
    }
}

/// Render the registry's available builds.
pub fn print_registry(registry: &Registry) {
    println!(
        "{} build{} available:",
        registry.entries.len(),
        plural(registry.entries.len())
    );
    for entry in &registry.entries {
        println!("{}  {}", entry.version, entry.manifest);
    }
}

/// Render a freshly created instance.
pub fn print_instance(instance: &GameInstance) {
    println!(
        "instance {} created (game version {})",
        instance.name, instance.version
    );
    println!("  path: {}", instance.dir.display());
    println!("  game: {}", instance.game_dir.display());
}

/// Render the list of game instances.
pub fn print_instances(instances: &[GameInstance]) {
    if instances.is_empty() {
        println!("No game instances stored.");
        return;
    }
    for instance in instances {
        let captured = if instance.created == 0 {
            "unknown".to_string()
        } else {
            fmt::format_date_utc(instance.created)
        };
        let game = if instance.game_dir.is_dir() {
            "ok"
        } else {
            "BROKEN LINK"
        };
        println!(
            "{}  (version {})  captured {}  game link {}",
            instance.name, instance.version, captured, game
        );
        println!("  {}", instance.dir.display());
    }
}

/// Render the resolved Steam and Project Zomboid paths.
pub fn print_paths(
    paths: &SteamPaths,
    version: &GameVersionInfo,
    steamcmd: Option<&std::path::Path>,
) {
    println!("Steam root:       {}", paths.steam_dir.display());
    println!(
        "Workshop content: {}{}",
        paths.workshop_content.display(),
        if paths.workshop_content.is_dir() {
            ""
        } else {
            " (no content downloaded)"
        }
    );
    println!(
        "Game install:     {}",
        optional_path(paths.game_dir.as_deref())
    );
    println!("Game version:     {}", describe_version(version));
    println!(
        "Zomboid user dir: {}",
        optional_path(paths.zomboid_user_dir.as_deref())
    );
    println!("SteamCMD:         {}", optional_path(steamcmd));
}

/// Format detected game version info for display.
fn describe_version(info: &GameVersionInfo) -> String {
    let version = info.version.as_deref().unwrap_or("unknown");
    match (&info.revision, info.source) {
        (Some(revision), Some(source)) => {
            format!("{version} (revision {revision}, from {source:?})")
        }
        (Some(revision), None) => format!("{version} (revision {revision})"),
        _ => version.to_owned(),
    }
}

/// Format an optional path for display.
fn optional_path(path: Option<&std::path::Path>) -> String {
    path.map(|p| p.display().to_string())
        .unwrap_or_else(|| "(not found)".to_string())
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}
