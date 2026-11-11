//! mod.info manifests: the game version range a mod supports.

use std::fs;
use std::path::Path;

use super::version::Version;

/// The versionMin/versionMax fields of one mod.info file. Unknown keys
/// are ignored.
#[derive(Debug, Default)]
pub struct VersionRange {
    pub min: Option<Version>,
    pub max: Option<Version>,
}

/// Parse a mod.info file. Missing or unreadable files yield an empty
/// range.
pub fn parse(path: &Path) -> VersionRange {
    let Ok(text) = fs::read_to_string(path) else {
        return VersionRange::default();
    };
    let mut range = VersionRange::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = Version::new(value.trim());
        match key.trim() {
            "versionMin" => range.min = Some(value),
            "versionMax" => range.max = Some(value),
            _ => {}
        }
    }
    range
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_fields() -> anyhow::Result<()> {
        let dir = std::env::temp_dir().join(format!("snapoid-modinfo-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("mod.info");
        std::fs::write(&path, "name=Test\nversionMin=42.16.0\nversionMax=42.99\n")?;

        let range = parse(&path);
        assert_eq!(range.min.as_ref().map(Version::text), Some("42.16.0"));
        assert_eq!(range.max.as_ref().map(Version::text), Some("42.99"));

        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }
}
