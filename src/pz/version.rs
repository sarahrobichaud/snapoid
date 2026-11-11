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
}
