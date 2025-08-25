use once_cell::sync::Lazy;
use regex::Regex;
use std::cmp::Ordering;
use version_compare;

static VERSION_NUMBER_STRICT_MATCH_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\d+(\.\d+)+([.|\-|+|_| ]*[A-Za-z0-9]+)*").unwrap());

static VERSION_NUMBER_MATCH_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\d+(\.\d+)*([.|\-|+|_| ]*[A-Za-z0-9]+)*").unwrap());

#[derive(Debug, Clone)]
pub struct Version {
    string: String,
}

impl Version {
    pub fn new(string: String) -> Self {
        Version { string }
    }

    pub fn is_valid(&self) -> bool {
        self.get_valid_version().is_some()
    }

    pub fn get_valid_version(&self) -> Option<String> {
        let version_string = VERSION_NUMBER_STRICT_MATCH_REGEX
            .find(&self.string)
            .or_else(|| VERSION_NUMBER_MATCH_REGEX.find(&self.string))
            .map(|match_str| match_str.as_str());
        version_string.and_then(|version_string| {
            version_compare::Version::from(version_string).map(|v| v.to_string())
        })
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        let version = version_compare::Version::from(self.string.as_str());
        let other_version = version_compare::Version::from(other.string.as_str());
        version == other_version
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let version = version_compare::Version::from(self.string.as_str());
        let other_version = version_compare::Version::from(other.string.as_str());
        version.partial_cmp(&other_version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_is_valid() {
        let version = Version {
            string: "1.0.0".to_string(),
        };
        assert!(version.is_valid());
        let version = Version {
            string: "1.0.0-alpha".to_string(),
        };
        assert!(version.is_valid());
        let version = Version {
            string: "版本1.0.0".to_string(),
        };
        assert!(version.is_valid());
    }

    #[test]
    fn test_version_eq() {
        let version = Version {
            string: "1.0.0".to_string(),
        };
        let other_version = Version {
            string: "1.0".to_string(),
        };
        assert_eq!(version, other_version);
    }

    #[test]
    fn test_version_lt() {
        let version = Version {
            string: "1.0".to_string(),
        };
        let other_version = Version {
            string: "1.0.1".to_string(),
        };
        assert!(version < other_version);
    }
}
