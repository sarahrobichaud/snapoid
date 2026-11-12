//! Mod compatibility with a game version, replicating the game's own
//! resolution — verified against the decompiled 42.22.2 classes (see
//! docs/research/mod-resolution.md):
//!
//! - `ZomboidFileSystem.getModVersionDirName` selects the **highest**
//!   version folder with `42.0 ≤ folder ≤ game version`;
//! - `ChooseGameInfo.Mod.isAvailableSelf` then checks **that folder's**
//!   manifest range (`versionMin > game` or `versionMax < game` →
//!   unavailable);
//! - with no selectable folder the game falls back to `"42.0"`, which
//!   does not exist, so only a `common/` manifest can carry the mod.

use std::fmt;

use super::item::{FolderRange, Layout, ModEntry};
use super::version;

/// Whether a mod loads on a game version, as far as snapoid can tell
/// from its manifests and layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Compatibility {
    /// The game would make the mod available on this version.
    Compatible,
    /// The game would not make the mod available on this version.
    Incompatible(Incompatibility),
    /// Nothing to base a verdict on (no bundled mods at all).
    Unknown,
}

/// Which manifest declared an excluding range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Manifest {
    /// A version folder, by name.
    Folder(String),
    Common,
    Root,
}

/// Why a mod does not load on a game version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incompatibility {
    /// The selected manifest's declared range excludes the version.
    DeclaredRange {
        manifest: Manifest,
        min: Option<String>,
        max: Option<String>,
    },
    /// The mod ships only the other build's layout, which this game's
    /// loader never reads.
    LayoutOnly { layout: Layout },
    /// No version folder the game could select and no `common/`
    /// fallback.
    NoFolder { lowest: Option<String> },
}

impl fmt::Display for Incompatibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Incompatibility::DeclaredRange { manifest, min, max } => {
                let source = match manifest {
                    Manifest::Folder(name) => format!("folder {name}"),
                    Manifest::Common => "common manifest".to_owned(),
                    Manifest::Root => "root manifest".to_owned(),
                };
                match (min, max) {
                    (Some(min), Some(max)) => write!(f, "{source} declares {min} - {max}"),
                    (Some(min), None) => write!(f, "{source} declares {min}+"),
                    (None, Some(max)) => write!(f, "{source} declares up to {max}"),
                    (None, None) => write!(f, "{source} declares an empty range"),
                }
            }
            Incompatibility::LayoutOnly { layout } => match layout {
                Layout::B41 => write!(f, "B41-only layout (not loaded by B42)"),
                Layout::B42 | Layout::Hybrid => {
                    write!(f, "B42-only layout (not loaded by B41)")
                }
            },
            Incompatibility::NoFolder { lowest } => match lowest {
                Some(lowest) => {
                    write!(f, "needs {lowest} or newer (no matching version folder)")
                }
                None => write!(f, "no version folder for this build"),
            },
        }
    }
}

/// The compatibility of one bundled mod with `game_version`.
pub fn mod_compatibility(mod_entry: &ModEntry, game_version: &str) -> Compatibility {
    let game = version::game_int(game_version);
    let floor = 42 * 1000;

    if game < floor {
        // B41 game: the B41 loader reads only the root manifest.
        return match &mod_entry.root {
            Some(root) => declared_verdict(
                root.min.as_deref(),
                root.max.as_deref(),
                game,
                Manifest::Root,
            ),
            // B42-only layout: the B41 loader never sees it.
            None => Compatibility::Incompatible(Incompatibility::LayoutOnly {
                layout: Layout::B42,
            }),
        };
    }

    // B42 game: a B41-only mod is invisible — the loader never reads a
    // root mod.info.
    if mod_entry.layout == Layout::B41 {
        return Compatibility::Incompatible(Incompatibility::LayoutOnly {
            layout: Layout::B41,
        });
    }

    // The game selects the highest version folder ≤ the running
    // version and checks that folder's declared range.
    let selected = mod_entry
        .folders
        .iter()
        .filter(|f| f.version >= floor && f.version <= game)
        .max_by_key(|f| f.version);
    if let Some(folder) = selected {
        return declared_verdict(
            folder.min.as_deref(),
            folder.max.as_deref(),
            game,
            Manifest::Folder(folder.name.clone()),
        );
    }

    // No selectable folder: the game's fallback folder "42.0" does not
    // exist, so only a common/ manifest can carry the mod.
    match &mod_entry.common {
        Some(common) => declared_verdict(
            common.min.as_deref(),
            common.max.as_deref(),
            game,
            Manifest::Common,
        ),
        None => Compatibility::Incompatible(Incompatibility::NoFolder {
            lowest: lowest_folder_name(mod_entry),
        }),
    }
}

fn declared_verdict(
    min: Option<&str>,
    max: Option<&str>,
    game: u64,
    manifest: Manifest,
) -> Compatibility {
    // isAvailableSelf: versionMin > game or versionMax < game →
    // unavailable; missing ends are unbounded.
    let above_min = min
        .map(|min| game >= version::game_int(min))
        .unwrap_or(true);
    let below_max = max
        .map(|max| game <= version::game_int(max))
        .unwrap_or(true);
    if above_min && below_max {
        Compatibility::Compatible
    } else {
        Compatibility::Incompatible(Incompatibility::DeclaredRange {
            manifest,
            min: min.map(str::to_owned),
            max: max.map(str::to_owned),
        })
    }
}

fn lowest_folder_name(mod_entry: &ModEntry) -> Option<String> {
    super::item::lowest_folder(mod_entry).map(|f| f.name.clone())
}

/// Whether an availability interval rests on an author-declared range
/// or on folder structure alone — the display tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// A manifest declared a range covering this interval.
    Declared,
    /// The game would load the mod here, but no manifest declares it.
    Structural,
}

/// One availability interval; ends inclusive, a `None` end unbounded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Availability {
    pub start: u64,
    pub end: Option<u64>,
    pub tier: Tier,
}

/// The game-version intervals on which the mod would be available,
/// computed by simulating the game's folder selection across the B42
/// version space. Each folder is selected from its own version until
/// the next one, so a folder's declared range only applies within its
/// selection domain; adjacent same-tier intervals are merged. Each
/// interval carries its tier: an interval is `Declared` when the
/// manifest selected there declares a versionMin or versionMax, and
/// `Structural` when the game would load the mod without any declared
/// range.
pub fn available_intervals(mod_entry: &ModEntry) -> Vec<Availability> {
    let floor = 42 * 1000;
    let mut folders: Vec<&FolderRange> = mod_entry
        .folders
        .iter()
        .filter(|f| f.version >= floor)
        .collect();
    folders.sort_by_key(|f| f.version);

    let mut out: Vec<Availability> = Vec::new();

    // Below the lowest folder the game falls back to "42.0", which
    // does not exist, so only a common/ manifest carries the mod there
    // (and with no folders at all, everywhere).
    let common_domain_end = folders.first().map(|f| f.version - 1);
    if let Some(common) = &mod_entry.common {
        let start = common
            .min
            .as_deref()
            .map(version::game_int)
            .unwrap_or(floor)
            .max(floor);
        let end = match (&common.max, common_domain_end) {
            (Some(max), Some(domain)) => Some(version::game_int(max).min(domain)),
            (Some(max), None) => Some(version::game_int(max)),
            (None, Some(domain)) => Some(domain),
            (None, None) => None,
        };
        let tier = declared_tier(common.min.is_some() || common.max.is_some());
        if end.is_none_or(|end| end >= start) {
            out.push(Availability { start, end, tier });
        }
    }

    // Each folder is selected from its own version until the next one.
    for (index, folder) in folders.iter().enumerate() {
        let domain_start = folder.version;
        let domain_end = folders.get(index + 1).map(|next| next.version - 1);
        let start = folder
            .min
            .as_deref()
            .map(version::game_int)
            .unwrap_or(domain_start)
            .max(domain_start);
        let end = match (&folder.max, domain_end) {
            (Some(max), Some(domain)) => Some(version::game_int(max).min(domain)),
            (Some(max), None) => Some(version::game_int(max)),
            (None, Some(domain)) => Some(domain),
            (None, None) => None,
        };
        let tier = declared_tier(folder.min.is_some() || folder.max.is_some());
        if end.is_none_or(|end| end >= start) {
            out.push(Availability { start, end, tier });
        }
    }

    merge_adjacent(out)
}

fn declared_tier(declares: bool) -> Tier {
    if declares {
        Tier::Declared
    } else {
        Tier::Structural
    }
}

fn merge_adjacent(intervals: Vec<Availability>) -> Vec<Availability> {
    let mut sorted = intervals;
    sorted.sort_by_key(|interval| interval.start);
    let mut merged: Vec<Availability> = Vec::new();
    for interval in sorted {
        match merged.last_mut() {
            // Adjacent intervals merge only within a tier: the boundary
            // between declared and structural coverage is information.
            Some(last)
                if last.tier == interval.tier
                    && last.end.is_none_or(|last| interval.start <= last + 1) =>
            {
                if interval.end.is_none() {
                    last.end = None;
                } else if let Some(end) = interval.end {
                    match last.end {
                        Some(last_end) => last.end = Some(last_end.max(end)),
                        None => last.end = None,
                    }
                }
            }
            _ => merged.push(interval),
        }
    }
    merged
}

/// The compatibility of a workshop item: installable when at least one
/// bundled mod loads on the game version.
pub fn item_compatibility(mods: &[ModEntry], game_version: &str) -> Compatibility {
    let mut saw_unknown = false;
    let mut first_incompatible = None;
    for mod_entry in mods {
        match mod_compatibility(mod_entry, game_version) {
            Compatibility::Compatible => return Compatibility::Compatible,
            Compatibility::Unknown => saw_unknown = true,
            Compatibility::Incompatible(reason) => {
                if first_incompatible.is_none() {
                    first_incompatible = Some(reason);
                }
            }
        }
    }
    if saw_unknown {
        return Compatibility::Unknown;
    }
    match first_incompatible {
        Some(reason) => Compatibility::Incompatible(reason),
        None => Compatibility::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pz::item::ManifestRange;

    fn folder(name: &str, min: Option<&str>, max: Option<&str>) -> FolderRange {
        FolderRange {
            version: version::game_int(name),
            name: name.to_owned(),
            min: min.map(str::to_owned),
            max: max.map(str::to_owned),
        }
    }

    fn entry(
        folders: Vec<FolderRange>,
        common: Option<ManifestRange>,
        root: Option<ManifestRange>,
        layout: Layout,
    ) -> ModEntry {
        ModEntry {
            name: "M".to_owned(),
            mod_id: None,
            title: None,
            author: None,
            folders,
            common,
            root,
            requires: Vec::new(),
            incompatible: Vec::new(),
            load_before: Vec::new(),
            load_after: Vec::new(),
            layout,
        }
    }

    /// The shape of workshop item 3723726293 (Neat Rocco's UI): folder
    /// 42 declares 42.15 - 42.19, folder 42.20 declares 42.20+ (no
    /// max), common/ and a root manifest ship without version fields.
    #[test]
    fn per_folder_ranges_follow_the_games_selection() {
        let m = entry(
            vec![
                folder("42", Some("42.15"), Some("42.19")),
                folder("42.20", Some("42.20"), None),
            ],
            Some(ManifestRange { min: None, max: None }),
            Some(ManifestRange { min: None, max: None }),
            Layout::Hybrid,
        );

        // The game selects the highest folder ≤ the running version:
        // 42.20 on 42.22, whose manifest has no max.
        assert_eq!(mod_compatibility(&m, "42.22.2"), Compatibility::Compatible);
        // 42 on 42.19: its max 42.19 is not below the game version.
        assert_eq!(mod_compatibility(&m, "42.19"), Compatibility::Compatible);
        // 42 on 42.14: its min 42.15 is above the game version.
        assert_eq!(
            mod_compatibility(&m, "42.14"),
            Compatibility::Incompatible(Incompatibility::DeclaredRange {
                manifest: Manifest::Folder("42".to_owned()),
                min: Some("42.15".to_owned()),
                max: Some("42.19".to_owned()),
            })
        );

        // Available on 42.15 and everything above it — the two
        // folders' domains merge into one declared interval.
        assert_eq!(
            available_intervals(&m),
            vec![Availability {
                start: 42_015,
                end: None,
                tier: Tier::Declared,
            }]
        );
    }

    /// The shape of a mod with declared and fieldless folders side by
    /// side: structural coverage is marked, never passed off as
    /// declared.
    #[test]
    fn structural_coverage_is_tagged_apart_from_declared() {
        let m = entry(
            vec![
                folder("42", None, None),
                folder("42.13", Some("42.13"), Some("42.15.3")),
                folder("42.16", None, None),
            ],
            None,
            None,
            Layout::B42,
        );

        assert_eq!(
            available_intervals(&m),
            vec![
                Availability {
                    start: 42_000,
                    end: Some(42_012),
                    tier: Tier::Structural,
                },
                Availability {
                    start: 42_013,
                    end: Some(42_015),
                    tier: Tier::Declared,
                },
                Availability {
                    start: 42_016,
                    end: None,
                    tier: Tier::Structural,
                },
            ]
        );
    }

    #[test]
    fn a_declared_max_on_the_highest_folder_bounds_from_above() {
        let m = entry(
            vec![
                folder("42", Some("42.15"), None),
                folder("42.20", Some("42.20"), Some("42.22")),
            ],
            None,
            None,
            Layout::B42,
        );

        assert_eq!(mod_compatibility(&m, "42.22"), Compatibility::Compatible);
        assert_eq!(
            mod_compatibility(&m, "42.25"),
            Compatibility::Incompatible(Incompatibility::DeclaredRange {
                manifest: Manifest::Folder("42.20".to_owned()),
                min: Some("42.20".to_owned()),
                max: Some("42.22".to_owned()),
            })
        );
        // Folder 42's missing max is clipped to its selection domain.
        assert_eq!(
            available_intervals(&m),
            vec![Availability {
                start: 42_015,
                end: Some(42_022),
                tier: Tier::Declared,
            }]
        );
    }

    #[test]
    fn common_manifest_covers_below_the_lowest_folder() {
        let m = entry(
            vec![folder("42.16", None, None)],
            Some(ManifestRange { min: None, max: None }),
            None,
            Layout::B42,
        );
        // No folder ≤ 42.10: the game falls back to common/, which
        // declares nothing → available.
        assert_eq!(mod_compatibility(&m, "42.10"), Compatibility::Compatible);
        assert_eq!(mod_compatibility(&m, "42.16"), Compatibility::Compatible);

        // Without common/ the mod is not even listed down there.
        let m = entry(vec![folder("42.16", None, None)], None, None, Layout::B42);
        assert_eq!(
            mod_compatibility(&m, "42.10"),
            Compatibility::Incompatible(Incompatibility::NoFolder {
                lowest: Some("42.16".to_owned()),
            })
        );
    }

    #[test]
    fn layout_mismatch_excludes_the_mod() {
        let b41 = entry(Vec::new(), None, Some(ManifestRange { min: None, max: None }), Layout::B41);
        assert_eq!(
            mod_compatibility(&b41, "42.22.2"),
            Compatibility::Incompatible(Incompatibility::LayoutOnly { layout: Layout::B41 })
        );
        let b42 = entry(vec![folder("42", None, None)], None, None, Layout::B42);
        assert_eq!(
            mod_compatibility(&b42, "41.5"),
            Compatibility::Incompatible(Incompatibility::LayoutOnly { layout: Layout::B42 })
        );
        // A B41 mod on a B41 game loads; its range is just unknown.
        assert_eq!(mod_compatibility(&b41, "41.5"), Compatibility::Compatible);
    }

    #[test]
    fn item_is_installable_when_any_mod_loads() {
        let good = entry(
            vec![folder("42", Some("42.0"), None)],
            None,
            None,
            Layout::B42,
        );
        let bad = entry(
            vec![folder("43", Some("43.0"), None)],
            None,
            None,
            Layout::B42,
        );
        assert_eq!(
            item_compatibility(&[good.clone(), bad.clone()], "42.22"),
            Compatibility::Compatible
        );
        assert!(matches!(
            item_compatibility(&[bad], "42.22"),
            Compatibility::Incompatible(_)
        ));
        assert_eq!(item_compatibility(&[], "42.22"), Compatibility::Unknown);
    }
}
