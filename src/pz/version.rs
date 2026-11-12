//! Dotted version strings with component-wise ordering.

use std::cmp::Ordering;

/// A dotted version like "42.22.2". Orders component-wise; missing
/// components count as 0, so "42.22" equals "42.22.0".
#[derive(Debug, Clone)]
pub struct Version {
    text: String,
    components: Vec<u64>,
}

impl Version {
    /// Parse a version string; non-numeric components count as 0.
    pub fn new(text: &str) -> Self {
        Self {
            components: text
                .split('.')
                .map(|part| part.trim().parse().unwrap_or(0))
                .collect(),
            text: text.to_owned(),
        }
    }

    /// The version as originally written.
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// The game's version key: `major * 1000 + min(minor, 999)`, split on
/// '.'. The third component and any suffix are ignored — 42.22.2 ≡
/// 42.22, and a `versionMax=42.21.999` caps at 42.21
/// (docs/research/mod-resolution.md §3). Non-numeric parts count as 0.
pub fn game_int(text: &str) -> u64 {
    let mut parts = text.trim().split('.');
    let major: u64 = parts
        .next()
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or(0);
    let minor: u64 = parts
        .next()
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or(0);
    major * 1000 + minor.min(999)
}

/// The game's own rendering of a version int (`GameVersion.toString`):
/// `major.minor`, e.g. 42015 → "42.15".
pub fn int_text(version: u64) -> String {
    format!("{}.{}", version / 1000, version % 1000)
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        for index in 0..self.components.len().max(other.components.len()) {
            let left = self.components.get(index).copied().unwrap_or(0);
            let right = other.components.get(index).copied().unwrap_or(0);
            if left != right {
                return left.cmp(&right);
            }
        }
        Ordering::Equal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_component_wise() {
        assert_eq!(Version::new("42.22"), Version::new("42.22.0"));
        assert!(Version::new("42.22.2") > Version::new("42.22"));
        assert!(Version::new("42.9") < Version::new("42.10"));
        assert!(Version::new("43.0") > Version::new("42.99.99"));
    }

    #[test]
    fn game_key_ignores_the_third_component() {
        assert_eq!(game_int("42.22.2"), game_int("42.22"));
        assert_eq!(game_int("42.21.999"), 42_021);
        assert_eq!(game_int("42"), 42_000);
        assert!(game_int("42.22") > game_int("42.21.999"));
    }
}
