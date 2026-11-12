use std::fs;
use std::path::Path;

use super::version::Version;

/// The fields of one mod.info file snapoid keeps. Unknown keys are
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

/// What a matched key means. `Ignored` keys are matched anyway so a line
/// carrying one is not mistaken for a later key (the game reads them;
/// snapoid just does not keep them).
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

/// Keys checked per line, in the game's check order: the first key whose
/// `key=` appears anywhere in the line wins it. `name` and `id` before
/// `description` is what keeps prose like "description=My mod id=42"
/// from being read as a description — the same way the game reads it.
const KEYS: &[(&str, Field)] = &[
    ("name=", Field::Name),
    ("id=", Field::Id),
    ("poster=", Field::Ignored),
    ("icon=", Field::Ignored),
    ("description=", Field::Ignored),
    ("url=", Field::Ignored),
    ("author=", Field::Author),
    ("modversion=", Field::Ignored),
    ("require=", Field::Requires),
    ("incompatible=", Field::Incompatible),
    ("loadModAfter=", Field::LoadAfter),
    ("loadModBefore=", Field::LoadBefore),
    ("minRequiredVersion=", Field::Ignored),
    ("onlyForGameMode=", Field::Ignored),
    ("pack=", Field::Ignored),
    ("tiledef=", Field::Ignored),
    ("versionMin=", Field::VersionMin),
    ("versionMax=", Field::VersionMax),
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
        let Some((field, value)) = match_line(line) else {
            continue;
        };
        apply(&mut info, field, value);
    }
    info
}

/// The first key in check order that appears anywhere in the line, with
/// everything after its first occurrence as the value.
fn match_line(line: &str) -> Option<(Field, &str)> {
    for (key, field) in KEYS {
        if let Some(position) = line.find(key) {
            return Some((*field, &line[position + key.len()..]));
        }
    }
    None
}

fn apply(info: &mut ModInfo, field: Field, value: &str) {
    match field {
        Field::Id => set_str(&mut info.id, value),
        Field::Name => set_str(&mut info.name, value),
        Field::Author => set_str(&mut info.author, value),
        Field::Requires => info.requires = split_ids(value),
        Field::Incompatible => info.incompatible = split_ids(value),
        Field::LoadBefore => info.load_before = split_ids(value),
        Field::LoadAfter => info.load_after = split_ids(value),
        Field::VersionMin => info.version_min = Some(Version::new(value.trim())),
        Field::VersionMax => info.version_max = Some(Version::new(value.trim())),
        Field::Ignored => {}
    }
}

fn set_str(slot: &mut Option<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        *slot = Some(value.to_owned());
    }
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
        let parsed =
            info("name=Test Mod\nid=TM\nauthor=Someone\nversionMin=42.16.0\nversionMax=42.99\n");
        assert_eq!(parsed.id.as_deref(), Some("TM"));
        assert_eq!(parsed.name.as_deref(), Some("Test Mod"));
        assert_eq!(parsed.author.as_deref(), Some("Someone"));
        assert_eq!(
            parsed.version_min.as_ref().map(Version::text),
            Some("42.16.0")
        );
        assert_eq!(
            parsed.version_max.as_ref().map(Version::text),
            Some("42.99")
        );
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

    #[test]
    fn matches_keys_anywhere_in_the_line() {
        // The game matches with contains(key=): a line mentioning a key
        // anywhere is read as that key, and the check order decides
        // which key wins.
        let parsed = info("description=Read the id= field on the wiki\n");
        assert_eq!(parsed.id.as_deref(), Some("field on the wiki"));

        // name is checked before id, so this line is a name.
        let parsed = info("name=My id=42 mod\n");
        assert_eq!(parsed.name.as_deref(), Some("My id=42 mod"));
        assert_eq!(parsed.id, None);
    }

    #[test]
    fn empty_values_are_absent() {
        let parsed = info("name=\nid=TM\n");
        assert_eq!(parsed.name, None);
        assert_eq!(parsed.id.as_deref(), Some("TM"));
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
}
