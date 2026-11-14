//! Human-readable rendering of library results. No orchestration, no
//! library imports beyond data types.

use std::cmp::Ordering;

use yapzm_lib::download::Downloaded;
use yapzm_lib::fmt;
use yapzm_lib::pz::compat;
use yapzm_lib::pz::item::Layout;
use yapzm_lib::pz::probe::GameVersionInfo;
use yapzm_lib::pz::version;
use yapzm_lib::registry::Registry;
use yapzm_lib::scanner::workshop::{ModEntry, WorkshopItem};
use yapzm_lib::steam::SteamPaths;
use yapzm_lib::store::{
    GameInstance, InstallOutcome, PackedModpack, Source, StoredGame, StoredMod,
};

/// Render the workshop inventory from `scan`: per item, one row per
/// bundled mod with its layout, Mod ID, declared version range, and
/// declared dependencies. External sources show declared constraints
/// only — folder-name hints are a store-listing concern.
pub fn print_scan(items: &[WorkshopItem]) {
    let mut tally = LayoutTally::default();
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
            println!("{}", mod_row(m, name_width, id_width, &declared_range(m)));
        }
    }

    println!(
        "\n{} workshop item{}, {} mod{} — {}",
        items.len(),
        plural(items.len()),
        tally.total(),
        plural(tally.total()),
        tally.breakdown()
    );
}

/// Render the stored workshop items grouped by author: the `author=`
/// each bundled mod declares in its mod.info. An item lands in the
/// group of its first mod's author (mods are name-sorted, so the
/// choice is deterministic); items whose manifests declare none share
/// the unknown group, printed last.
pub fn print_stored_mods(items: &[StoredMod]) {
    let mut tally = LayoutTally::default();
    let mut inferred = 0;
    for (author, group) in group_by_author(items) {
        println!("\n{}", author.unwrap_or("(unknown author)"));
        for item in group {
            let mut header = format!("  {}  updated {}", item.id, updated_text(item.time_updated));
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
                println!("    (no mods recorded)");
            }
            for m in &item.mods {
                tally.add(m.layout);
                if is_inferred(m) {
                    inferred += 1;
                }
                println!("  {}", mod_row(m, name_width, id_width, &version_range(m)));
            }
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

/// Group stored items by the author their first bundled mod declares,
/// sorted case-insensitively with the unknown group last. Items keep
/// their id order within a group.
fn group_by_author(items: &[StoredMod]) -> Vec<(Option<&str>, Vec<&StoredMod>)> {
    let mut groups: Vec<(Option<&str>, Vec<&StoredMod>)> = Vec::new();
    for item in items {
        let author = item.mods.first().and_then(|m| m.author.as_deref());
        if let Some((_, group)) = groups.iter_mut().find(|(key, _)| *key == author) {
            group.push(item);
        } else {
            groups.push((author, vec![item]));
        }
    }
    groups.sort_by(|(a, _), (b, _)| match (a, b) {
        (Some(a), Some(b)) => a.to_lowercase().cmp(&b.to_lowercase()),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
    groups
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

/// Legend for structural coverage, shown only when any was displayed.
fn inferred_legend(count: usize) -> String {
    if count > 0 {
        " (~ = would load, not author-declared)".to_owned()
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
/// declared dependencies. The range is computed by the caller — the
/// tiering differs between external sources and the store.
fn mod_row(mod_entry: &ModEntry, name_width: usize, id_width: usize, range: &str) -> String {
    let mut row = format!(
        "  {:name_width$}  {:<6}  {:id_width$}  {}",
        mod_entry.name,
        layout_tag(mod_entry.layout),
        mod_id_text(mod_entry),
        range
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

/// The declared version coverage of a mod, as displayed — the intervals
/// where an author-declared range makes the mod available. External
/// sources (workshop) show declared constraints only; folder-structure
/// coverage is a store-listing concern.
fn declared_range(mod_entry: &ModEntry) -> String {
    let intervals = compat::available_intervals(mod_entry);
    let declared: Vec<&compat::Availability> = intervals
        .iter()
        .filter(|interval| interval.tier == compat::Tier::Declared)
        .collect();
    if !declared.is_empty() {
        return render_intervals(&declared);
    }
    // No B42 availability: a B41-only mod's root manifest range is all
    // it declares.
    if mod_entry
        .root
        .as_ref()
        .is_some_and(|root| root.min.is_some() || root.max.is_some())
    {
        let root = mod_entry.root.as_ref().unwrap();
        return ends_range(root.min.as_deref(), root.max.as_deref());
    }
    "-".to_owned()
}

/// The version range of a stored mod, as displayed: every availability
/// interval, with structural coverage (the game would load it, but no
/// manifest declares it) prefixed with ~ — a tier below a declared
/// constraint.
fn version_range(mod_entry: &ModEntry) -> String {
    let intervals = compat::available_intervals(mod_entry);
    if !intervals.is_empty() {
        let all: Vec<&compat::Availability> = intervals.iter().collect();
        return render_intervals(&all);
    }
    if mod_entry
        .root
        .as_ref()
        .is_some_and(|root| root.min.is_some() || root.max.is_some())
    {
        let root = mod_entry.root.as_ref().unwrap();
        return ends_range(root.min.as_deref(), root.max.as_deref());
    }
    "-".to_owned()
}

fn render_intervals(intervals: &[&compat::Availability]) -> String {
    intervals
        .iter()
        .map(|interval| {
            let range = match interval.end {
                Some(end) if interval.start == end => version::int_text(interval.start),
                Some(end) => format!(
                    "{} - {}",
                    version::int_text(interval.start),
                    version::int_text(end)
                ),
                None => format!("{}+", version::int_text(interval.start)),
            };
            if interval.tier == compat::Tier::Structural {
                format!("~{range}")
            } else {
                range
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn ends_range(min: Option<&str>, max: Option<&str>) -> String {
    match (min, max) {
        (Some(min), Some(max)) if version::game_int(min) == version::game_int(max) => {
            min.to_owned()
        }
        (Some(min), Some(max)) => format!("{min} - {max}"),
        (Some(min), None) => format!("{min}+"),
        (None, Some(max)) => format!("- {max}"),
        (None, None) => "-".to_owned(),
    }
}

/// True when any of the mod's availability rests on folder structure
/// rather than an author-declared range.
fn is_inferred(mod_entry: &ModEntry) -> bool {
    compat::available_intervals(mod_entry)
        .iter()
        .any(|interval| interval.tier == compat::Tier::Structural)
}

/// Render the outcome of an install into an instance's profile: the
/// copied and skipped mods, then the activation artifacts.
/// Render the result of packing an instance's mods into a modpack.
pub fn print_packed(packed: &PackedModpack) {
    println!(
        "\nPacked {} mod{} ({}) into {}",
        packed.mods.len(),
        plural(packed.mods.len()),
        fmt::format_bytes(packed.bytes),
        packed.path.display()
    );
    for name in &packed.mods {
        println!("  {name}");
    }
}

pub fn print_installed_mods(instance_name: &str, outcome: &InstallOutcome) {
    let done = outcome.mods.iter().filter(|m| m.dir.is_some()).count();
    println!(
        "\n{} mod{} installed into instance {}'s profile",
        done,
        plural(done),
        instance_name
    );
    for m in outcome.mods.iter().filter(|m| m.dir.is_some()) {
        let unsafe_tag = if m.unsafe_install { "  [unsafe]" } else { "" };
        match &m.mod_id {
            Some(id) => println!("  {}{unsafe_tag}  (id={id})", m.name),
            None => println!("  {}{unsafe_tag}", m.name),
        }
    }
    for m in outcome.mods.iter().filter(|m| m.dir.is_none()) {
        println!(
            "  skipped {}  — {}",
            m.name,
            m.skipped
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default()
        );
    }

    let activation = &outcome.activation;
    if activation.mod_ids.is_empty() {
        println!("\nNo mods activated.");
    } else {
        println!(
            "\n{} mod{} activated, load order:\n  {}",
            activation.mod_ids.len(),
            plural(activation.mod_ids.len()),
            activation.mod_ids.join(", ")
        );
    }
    for warning in &activation.warnings {
        println!("  warning: {warning}");
    }
    if !outcome.agent_files.is_empty() {
        println!(
            "\nZombieBuddy agent installed: {}",
            outcome.agent_files.join(", ")
        );
        println!("  (the agent is loaded automatically at launch)");
    }
    println!("  client list:   {}", activation.default_txt.display());
    println!("  server config: {}", activation.server_ini.display());
    println!("  (the game rewrites these on exit — install while it is closed)");
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
