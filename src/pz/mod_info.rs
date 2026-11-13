//! mod.info manifests: identity, dependencies, and the game version
//! range a mod supports.
//!
//! Parsing replicates the game's own loader, verified at the bytecode
//! level (`javap` on `ChooseGameInfo.readModInfoAux` — CFR could not
//! decompile the method; see docs/research/mod-resolution.md §4):
//! keys are checked in a fixed order per line, the first match wins
//! the line, and the match method is per key — `id=`, `versionMin=`
//! and `versionMax=` use `startsWith` (they collide with prose and
//! URLs), everything else uses `contains`. Values are the line with
//! every occurrence of the key removed, trimmed; blank values never
//! overwrite. `require`/`incompatible`/`loadMod*` values have all
//! backslashes stripped before comma-splitting.

use std::fs;
use std::path::Path;

use super::version::Version;

/// The fields of one mod.info file yapzm keeps. Unknown keys are
/// ignored.
#[derive(Debug, Default, PartialEq)]
pub struct ModInfo {
    /// `id=` — the Mod ID the game keys activation and dependencies on.
    pub id: Option<String>,
    /// `name=` — display name.
    pub name: Option<String>,
    /// `author=`.
    pub author: Option<String>,
    pub version_min: Option<Version>,
    pub version_max: Option<Version>,
    /// `require=` — Mod IDs that must be active.
    pub requires: Vec<String>,
    /// `incompatible=` — Mod IDs that must not be.
    pub incompatible: Vec<String>,
    /// `loadModBefore=` — load-order override.
    pub load_before: Vec<String>,
    /// `loadModAfter=` — load-order override.
    pub load_after: Vec<String>,
}

/// What a matched key means. `Ignored` keys are matched anyway so a
/// line carrying one is not mistaken for a later key (the game reads
/// them; yapzm just does not keep them).
#[derive(Debug, Clone, Copy)]
enum Field {
    Id,
    Name,
    Author,
    Requires,
    Incompatible,
    LoadBefore,
    LoadAfter,
    VersionMin,
    VersionMax,
    Ignored,
}

/// How a key matches a line. The collision-prone keys — `id=` appears
/// in workshop URLs, `versionMin=`/`versionMax=` in prose — match by
/// exact prefix; the rest match anywhere in the line.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Match {
    Contains,
    StartsWith,
}

/// Keys checked per line, in the game's check order: the first key
/// that matches wins the line.
const KEYS: &[(&str, Field, Match)] = &[
    ("name=", Field::Name, Match::Contains),
    ("poster=", Field::Ignored, Match::Contains),
    ("description=", Field::Ignored, Match::Contains),
    ("require=", Field::Requires, Match::Contains),
    ("incompatible=", Field::Incompatible, Match::Contains),
    ("loadModAfter=", Field::LoadAfter, Match::Contains),
    ("loadModBefore=", Field::LoadBefore, Match::Contains),
    ("id=", Field::Id, Match::StartsWith),
    ("author=", Field::Author, Match::Contains),
    ("modversion=", Field::Ignored, Match::Contains),
    ("icon=", Field::Ignored, Match::Contains),
    ("category=", Field::Ignored, Match::Contains),
    ("url=", Field::Ignored, Match::Contains),
    ("pack=", Field::Ignored, Match::Contains),
    ("type=", Field::Ignored, Match::Contains),
    ("tiledef=", Field::Ignored, Match::Contains),
    ("versionMax=", Field::VersionMax, Match::StartsWith),
    ("versionMin=", Field::VersionMin, Match::StartsWith),
];

/// Parse a mod.info file. Missing or unreadable files yield defaults.
pub fn parse(path: &Path) -> ModInfo {
    match fs::read_to_string(path) {
        Ok(text) => parse_str(&text),
        Err(_) => ModInfo::default(),
    }
}

fn parse_str(text: &str) -> ModInfo {
    let mut info = ModInfo::default();
    for line in text.lines() {
        let Some((field, key)) = match_line(line) else {
            continue;
        };
        apply(&mut info, field, key, line);
    }
    info
}

/// The first key in check order that matches the line.
fn match_line(line: &str) -> Option<(Field, &'static str)> {
    for (key, field, match_mode) in KEYS {
        let matched = match match_mode {
            Match::Contains => line.contains(key),
            Match::StartsWith => line.starts_with(key),
        };
        if matched {
            return Some((*field, key));
        }
    }
    None
}

fn apply(info: &mut ModInfo, field: Field, key: &'static str, line: &str) {
    // The game strips every occurrence of the key from the line and
    // trims; blank values never overwrite.
    let value = line.replace(key, "");
    let value = value.trim();
    match field {
        Field::Id => set_str(&mut info.id, value),
        Field::Name => set_str(&mut info.name, value),
        Field::Author => set_str(&mut info.author, value),
        Field::Requires => info.requires = split_ids(value),
        Field::Incompatible => info.incompatible = split_ids(value),
        Field::LoadBefore => info.load_before = split_ids(value),
        Field::LoadAfter => info.load_after = split_ids(value),
        Field::VersionMin => set_version(&mut info.version_min, value),
        Field::VersionMax => set_version(&mut info.version_max, value),
        Field::Ignored => {}
    }
}

fn set_str(slot: &mut Option<String>, value: &str) {
    if !value.is_empty() {
        *slot = Some(value.to_owned());
    }
}

fn set_version(slot: &mut Option<Version>, value: &str) {
    if !value.is_empty() {
        *slot = Some(Version::new(value));
    }
}

/// The tag an unsafe install appends to a mod's display name, so the
/// game's mod list shows what was forced past its declared range.
pub const UNSAFE_TAG: &str = " [unsafe]";

/// The mod.info text for an unsafe install: the declared range becomes
/// exactly `version` (`versionMin=` set to it, `versionMax=` dropped)
/// and the display name gains [`UNSAFE_TAG`]. Line matching mirrors
/// the game's loader, including its last-value-wins per key — so the
/// rewrite tags the last `name=` line, the one whose value the game
/// would keep, and replaces every `versionMin=`/`versionMax=` line.
pub fn unsafe_text(text: &str, fallback_name: &str, version: &str) -> String {
    let name = parse_str(text)
        .name
        .unwrap_or_else(|| fallback_name.to_owned());
    // The name line the game's parse keeps is the last one.
    let name_line = text
        .lines()
        .enumerate()
        .filter(|(_, line)| matches!(match_line(line), Some((Field::Name, _))))
        .map(|(index, _)| index)
        .last();
    let mut out = String::new();
    let mut bounded = false;
    for (index, line) in text.lines().enumerate() {
        if line.starts_with("versionMin=") {
            out.push_str(&format!("versionMin={version}\n"));
            bounded = true;
        } else if line.starts_with("versionMax=") {
            // Dropped: the forced range is unbounded above.
        } else if Some(index) == name_line {
            out.push_str(&format!("name={name}{UNSAFE_TAG}\n"));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !bounded {
        out.push_str(&format!("versionMin={version}\n"));
    }
    if name_line.is_none() {
        out.push_str(&format!("name={name}{UNSAFE_TAG}\n"));
    }
    out
}

/// `require=`-style values: the game strips all backslashes before
/// comma-splitting — that is why guides write `require=\ZombieBuddy`
/// (docs/research/mod-resolution.md §4).
fn split_ids(value: &str) -> Vec<String> {
    value
        .chars()
        .filter(|c| *c != '\\')
        .collect::<String>()
        .split(',')
        .map(|part| part.trim().to_owned())
        .filter(|part| !part.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(text: &str) -> ModInfo {
        parse_str(text)
    }

    #[test]
    fn parses_identity_and_versions() {
        let parsed = info(
            "name=Test Mod\nid=TM\nauthor=Someone\nversionMin=42.16.0\nversionMax=42.99\n",
        );
        assert_eq!(parsed.id.as_deref(), Some("TM"));
        assert_eq!(parsed.name.as_deref(), Some("Test Mod"));
        assert_eq!(parsed.author.as_deref(), Some("Someone"));
        assert_eq!(
            parsed.version_min.as_ref().map(Version::text),
            Some("42.16.0")
        );
        assert_eq!(parsed.version_max.as_ref().map(Version::text), Some("42.99"));
    }

    #[test]
    fn parses_dependency_lists() {
        let parsed = info(
            "require=\\ZombieBuddy, Other Mod\nincompatible=Bad Mod\n\
             loadModBefore=Early\nloadModAfter=\\Late One, Later\n",
        );
        assert_eq!(parsed.requires, vec!["ZombieBuddy", "Other Mod"]);
        assert_eq!(parsed.incompatible, vec!["Bad Mod"]);
        assert_eq!(parsed.load_before, vec!["Early"]);
        assert_eq!(parsed.load_after, vec!["Late One", "Later"]);
    }

    /// The Project Viewpoint manifest: a workshop URL containing
    /// `?id=3809306528` must not hijack the Mod ID — `id=` matches by
    /// prefix, and `url=` is consumed first anyway.
    #[test]
    fn url_lines_do_not_hijack_the_mod_id() {
        let parsed = info(
            "name=Viewpoint\nid=Viewpoint\n\
             url=https://steamcommunity.com/sharedfiles/filedetails/?id=3809306528\n",
        );
        assert_eq!(parsed.id.as_deref(), Some("Viewpoint"));
    }

    #[test]
    fn matches_contains_keys_anywhere_in_the_line() {
        // description is checked before id, so prose mentioning id= is
        // a description — and id= itself only matches by prefix.
        let parsed = info("description=Read the id= field on the wiki\n");
        assert_eq!(parsed.id, None);

        let parsed = info("name=My id=42 mod\n");
        assert_eq!(parsed.name.as_deref(), Some("My id=42 mod"));
        assert_eq!(parsed.id, None);
    }

    #[test]
    fn values_strip_every_key_occurrence() {
        // The game replaces all occurrences of the key in the line.
        let parsed = info("name=My name=x\n");
        assert_eq!(parsed.name.as_deref(), Some("My x"));
    }

    #[test]
    fn empty_values_are_absent() {
        let parsed = info("name=\nid=TM\nversionMin=\n");
        assert_eq!(parsed.name, None);
        assert_eq!(parsed.id.as_deref(), Some("TM"));
        assert_eq!(parsed.version_min, None);
    }

    #[test]
    fn unknown_and_missing_keys_are_ignored() {
        let parsed = info("poster=p.png\nsomething=x\n\nno equals sign\n");
        assert_eq!(parsed, ModInfo::default());
    }

    #[test]
    fn missing_file_yields_defaults() {
        let parsed = parse(Path::new("nonexistent/mod.info"));
        assert_eq!(parsed, ModInfo::default());
    }

    #[test]
    fn unsafe_text_pins_the_range_and_tags_the_name() {
        let unsafe_text = unsafe_text(
            "name=Bounded\nid=BM\nversionMin=42.16\nversionMax=42.19\n",
            "Bounded Mod",
            "42.22.2",
        );
        let parsed = info(&unsafe_text);
        assert_eq!(parsed.name.as_deref(), Some("Bounded [unsafe]"));
        assert_eq!(parsed.id.as_deref(), Some("BM"));
        assert_eq!(parsed.version_min.as_ref().map(Version::text), Some("42.22.2"));
        assert_eq!(parsed.version_max, None);
    }

    #[test]
    fn unsafe_text_adds_missing_lines_and_keeps_the_rest() {
        let unsafe_text = unsafe_text(
            "id=BM\nposter=p.png\nrequire=Other\n",
            "Bounded Mod",
            "42.22.2",
        );
        let parsed = info(&unsafe_text);
        assert_eq!(parsed.name.as_deref(), Some("Bounded Mod [unsafe]"));
        assert_eq!(parsed.version_min.as_ref().map(Version::text), Some("42.22.2"));
        assert_eq!(parsed.requires, vec!["Other"]);
        assert!(unsafe_text.contains("poster=p.png"));
        assert!(unsafe_text.contains("versionMin=42.22.2\n"));
    }

    #[test]
    fn unsafe_text_tags_the_name_line_the_game_keeps() {
        // The game's parse keeps the last value per key, so the
        // rewrite tags the last name= line and leaves earlier ones
        // (which it overwrites anyway) untouched.
        let unsafe_text = unsafe_text(
            "name=First\nname=Second\nversionMax=1.0\n",
            "X",
            "42.22.2",
        );
        assert_eq!(info(&unsafe_text).name.as_deref(), Some("Second [unsafe]"));
        assert!(unsafe_text.contains("name=First\n"));
        assert!(!unsafe_text.contains("versionMax"));
    }
}
