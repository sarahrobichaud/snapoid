//! Human-readable rendering of library results. No orchestration, no
//! library imports beyond data types.

use snapoid::download::Downloaded;
use snapoid::fmt;
use snapoid::pz::item::Layout;
use snapoid::pz::probe::GameVersionInfo;
use snapoid::pz::version::Version;
use snapoid::registry::Registry;
use snapoid::scanner::workshop::{ModEntry, WorkshopItem};
use snapoid::steam::SteamPaths;
use snapoid::store::{GameInstance, Source, StoredGame, StoredMod};

/// Render the workshop inventory from `scan`: per item, one row per
/// bundled mod with its layout, Mod ID, version range, and declared
/// dependencies.
pub fn print_scan(items: &[WorkshopItem]) {
    let mut tally = LayoutTally::default();
    let mut inferred = 0;
    for item in items {
        println!(
            "\n{}  updated {}  {} mod{}",
            item.id,
            updated_text(item.time_updated),
            item.mods.len(),
            plural(item.mods.len())
        );
        let (name_width, id_width) = row_widths(&item.mods);
        if item.mods.is_empty() {
            println!("  (no mods found)");
        }
        for m in &item.mods {
            tally.add(m.layout);
            if is_inferred(m) {
                inferred += 1;
            }
            println!("{}", mod_row(m, name_width, id_width));
        }
    }

    println!(
        "\n{} workshop item{}, {} mod{} — {}{}",
        items.len(),
        plural(items.len()),
        tally.total(),
        plural(tally.total()),
        tally.breakdown(),
        inferred_legend(inferred)
    );
}

/// Render the stored workshop items: what the library holds, from the
/// metadata captured at save time — no disk walks.
pub fn print_stored_mods(items: &[StoredMod]) {
    let mut tally = LayoutTally::default();
    let mut inferred = 0;
    for item in items {
        let mut header = format!(
            "\n{}  updated {}",
            item.id,
            updated_text(item.time_updated)
        );
        if let Some(title) = &item.title {
            header.push_str(&format!("  {title}"));
        }
        if let Some(source) = item.source {
            header.push_str(&format!("  ({})", source_text(source)));
        }
        header.push_str(&format!(
            "  {} mod{}",
            item.mods.len(),
            plural(item.mods.len())
        ));
        println!("{header}");

        let (name_width, id_width) = row_widths(&item.mods);
        if item.mods.is_empty() {
            // Old sidecars predate the captured metadata.
            println!("  (no mods recorded)");
        }
        for m in &item.mods {
            tally.add(m.layout);
            if is_inferred(m) {
                inferred += 1;
            }
            println!("{}", mod_row(m, name_width, id_width));
        }
    }

    println!(
        "\n{} workshop item{}, {} mod{} — {}{}",
        items.len(),
        plural(items.len()),
        tally.total(),
        plural(tally.total()),
        tally.breakdown(),
        inferred_legend(inferred)
    );
}

/// The update timestamp of an item, as displayed.
fn updated_text(time_updated: u64) -> String {
    if time_updated == 0 {
        "unknown".to_string()
    } else {
        fmt::format_date_utc(time_updated)
    }
}

/// How a stored copy was obtained, as displayed.
fn source_text(source: Source) -> &'static str {
    match source {
        Source::WorkshopDisk => "from disk",
        Source::Steamcmd => "from steamcmd",
    }
}

/// Legend for inferred ranges, shown only when any were displayed.
fn inferred_legend(count: usize) -> String {
    if count > 0 {
        " (~ = inferred from folder names)".to_owned()
    } else {
        String::new()
    }
}

/// Column widths for the shared mod-row layout.
fn row_widths(mods: &[ModEntry]) -> (usize, usize) {
    let name_width = mods
        .iter()
        .map(|m| m.name.chars().count())
        .max()
        .unwrap_or(0);
    let id_width = mods
        .iter()
        .map(|m| mod_id_text(m).chars().count())
        .max()
        .unwrap_or(0);
    (name_width, id_width)
}

/// One row per bundled mod: name, layout, Mod ID, version range, and
/// declared dependencies.
fn mod_row(mod_entry: &ModEntry, name_width: usize, id_width: usize) -> String {
    let mut row = format!(
        "  {:name_width$}  {:<6}  {:id_width$}  {}",
        mod_entry.name,
        layout_tag(mod_entry.layout),
        mod_id_text(mod_entry),
        version_range(mod_entry)
    );
    let dependencies = dependency_text(mod_entry);
    if !dependencies.is_empty() {
        row.push_str("  ");
        row.push_str(&dependencies);
    }
    if mod_entry.layout == Layout::B41 {
        // The B42 loader never reads a root mod.info, so a B41-only
        // folder is invisible to it.
        row.push_str("  (not loaded by B42)");
    }
    row
}

/// The Mod ID column: the activation key the game and dependency
/// declarations reference.
fn mod_id_text(mod_entry: &ModEntry) -> String {
    match &mod_entry.mod_id {
        Some(id) => format!("id={id}"),
        None => "id ?".to_owned(),
    }
}

fn layout_tag(layout: Layout) -> &'static str {
    match layout {
        Layout::B41 => "B41",
        Layout::B42 => "B42",
        Layout::Hybrid => "HYBRID",
    }
}

/// Declared dependencies and load-order overrides, as displayed. Empty
/// when the mod declares none.
fn dependency_text(mod_entry: &ModEntry) -> String {
    let mut parts = Vec::new();
    if !mod_entry.requires.is_empty() {
        parts.push(format!("requires {}", mod_entry.requires.join(", ")));
    }
    if !mod_entry.incompatible.is_empty() {
        parts.push(format!(
            "incompatible {}",
            mod_entry.incompatible.join(", ")
        ));
    }
    if !mod_entry.load_before.is_empty() {
        parts.push(format!("before {}", mod_entry.load_before.join(", ")));
    }
    if !mod_entry.load_after.is_empty() {
        parts.push(format!("after {}", mod_entry.load_after.join(", ")));
    }
    parts.join("  ")
}

/// Running tally of mod layouts for the summary line.
#[derive(Default)]
struct LayoutTally {
    b41: usize,
    b42: usize,
    hybrid: usize,
}

impl LayoutTally {
    fn add(&mut self, layout: Layout) {
        match layout {
            Layout::B41 => self.b41 += 1,
            Layout::B42 => self.b42 += 1,
            Layout::Hybrid => self.hybrid += 1,
        }
    }

    fn total(&self) -> usize {
        self.b41 + self.b42 + self.hybrid
    }

    fn breakdown(&self) -> String {
        let mut parts = Vec::new();
        if self.b41 > 0 {
            parts.push(format!("{} B41-only", self.b41));
        }
        if self.b42 > 0 {
            parts.push(format!("{} B42", self.b42));
        }
        if self.hybrid > 0 {
            parts.push(format!("{} hybrid", self.hybrid));
        }
        if parts.is_empty() {
            "no mods".to_owned()
        } else {
            parts.join(", ")
        }
    }
}

/// The supported version range of a mod, as displayed. Unbounded ends
/// are shown as unknown, not as compatible. Declared constraints win;
/// when the manifests declare nothing, the folder-name hint is shown
/// prefixed with ~ — a tier below a declared constraint.
fn version_range(mod_entry: &ModEntry) -> String {
    let min = mod_entry.version_min.as_deref();
    let max = mod_entry.version_max.as_deref();
    match (min, max) {
        (Some(min), Some(max)) if Version::new(min) == Version::new(max) => min.to_owned(),
        (Some(min), Some(max)) => format!("{min} - {max}"),
        (Some(min), None) => format!("{min}+"),
        (None, Some(max)) => format!("- {max}"),
        (None, None) => inferred_range(mod_entry),
    }
}

/// The folder-name hint as a range. Inference yields both ends or
/// neither — both come from the same folder set.
fn inferred_range(mod_entry: &ModEntry) -> String {
    let min = mod_entry.inferred_min.as_deref();
    let max = mod_entry.inferred_max.as_deref();
    match (min, max) {
        (Some(min), Some(max)) if min == max => format!("~{min}"),
        (Some(min), Some(max)) => format!("~{min} - {max}"),
        (Some(min), None) => format!("~{min}+"),
        (None, Some(max)) => format!("~≤ {max}"),
        (None, None) => "-".to_owned(),
    }
}

/// True when the displayed range is the folder-name hint rather than a
/// declared constraint.
fn is_inferred(mod_entry: &ModEntry) -> bool {
    mod_entry.version_min.is_none()
        && mod_entry.version_max.is_none()
        && mod_entry.inferred_min.is_some()
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

/// Render a freshly saved workshop mod.
pub fn print_saved_mod(stored: &StoredMod) {
    println!("mod {} saved", stored.id);
    if let Some(title) = &stored.title {
        println!("  title: {title}");
    }
    println!("  mods:  {}", stored.mods.len());
    println!("  path:  {}", stored.dir.display());
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
    println!("  path:    {}", instance.dir.display());
    println!("  game:    {}", instance.game_dir.display());
    println!("  profile: {}", instance.profile_dir.display());
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
        let profile = profile_state(instance);
        println!(
            "{}  (version {})  captured {}  game link {}  profile {}",
            instance.name, instance.version, captured, game, profile
        );
        println!("  {}", instance.dir.display());
    }
}

/// Profile state as displayed: fresh when empty or missing, in use
/// otherwise.
fn profile_state(instance: &GameInstance) -> &'static str {
    let empty = instance
        .profile_dir
        .read_dir()
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(true);
    if empty { "fresh" } else { "in use" }
}

/// Render a launched instance: one line, since the game output is
/// suppressed and snapoid now blocks until the game exits.
pub fn print_launched(instance: &GameInstance) {
    println!(
        "instance {} launched — Ctrl+C or closing the terminal stops the game",
        instance.name
    );
}

/// Render the game's exit after a launch.
pub fn print_exited(instance: &GameInstance, status: &std::process::ExitStatus) {
    println!("instance {} exited ({status})", instance.name);
    if !status.success() {
        println!(
            "  the game's output is suppressed; run its .bat in the game \
             folder manually to see it"
        );
    }
}

/// Render a profile reset.
pub fn print_reset(instance: &GameInstance) {
    println!("instance {} profile reset to fresh", instance.name);
    println!("  {}", instance.profile_dir.display());
}

/// Render the resolved Steam and Project Zomboid paths, grouped by
/// category: game first (version, install, user data), then Steam.
pub fn print_paths(
    paths: &SteamPaths,
    version: &GameVersionInfo,
    steamcmd: Option<&std::path::Path>,
) {
    println!("Game version:     {}", describe_version(version));
    println!(
        "Game install:     {}",
        optional_path(paths.game_dir.as_deref())
    );
    println!(
        "Zomboid user dir: {}",
        optional_path(paths.zomboid_user_dir.as_deref())
    );
    println!();
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
