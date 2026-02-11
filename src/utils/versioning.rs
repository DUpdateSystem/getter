use std::cmp::Ordering;

use libversion_sys;

use once_cell::sync::Lazy;
use regex::Regex;

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
        VERSION_NUMBER_STRICT_MATCH_REGEX
            .find(&self.string)
            .or_else(|| VERSION_NUMBER_MATCH_REGEX.find(&self.string))
            .map(|match_str| match_str.as_str().to_string())
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        match (self.get_valid_version(), other.get_valid_version()) {
            (Some(v1), Some(v2)) => libversion_sys::compare(&v1, &v2) == Ordering::Equal,
            _ => false,
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self.get_valid_version(), other.get_valid_version()) {
            (Some(v1), Some(v2)) => Some(libversion_sys::compare(&v1, &v2)),
            _ => None,
        }
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
        let chinese_suffix_version = Version {
            string: "版本1.0.0 天行健".to_string(),
        };
        assert!(chinese_suffix_version.is_valid());
    }

    #[test]
    fn test_version_is_invalid() {
        let version = Version {
            string: "xxx".to_string(),
        };
        assert!(!version.is_valid());
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

        let chinese_version = Version {
            string: "版本1.0.0".to_string(),
        };
        assert_eq!(version, chinese_version);
    }

    #[test]
    fn test_version_ne() {
        let version = Version {
            string: "1.0.0".to_string(),
        };
        let other_version = Version {
            string: "1.0.1".to_string(),
        };
        assert_ne!(version, other_version);
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

    #[test]
    fn test_version_gt() {
        let version = Version {
            string: "1.0.1".to_string(),
        };
        let other_version = Version {
            string: "1.0.1-alpha".to_string(),
        };
        assert!(version > other_version);
    }

    #[test]
    fn test_version_get_valid_version() {
        let version = Version {
            string: "1.0.0 123123".to_string(),
        };
        assert_eq!(
            version.get_valid_version(),
            Some("1.0.0 123123".to_string())
        );
        let version = Version {
            string: "1.0.0-alpha 版本".to_string(),
        };
        assert_eq!(version.get_valid_version(), Some("1.0.0-alpha".to_string()));
        let version = Version {
            string: "版本1.0.0".to_string(),
        };
        assert_eq!(version.get_valid_version(), Some("1.0.0".to_string()));
        let chinese_suffix_version = Version {
            string: "版本1.0.0 天行健".to_string(),
        };
        assert_eq!(
            chinese_suffix_version.get_valid_version(),
            Some("1.0.0".to_string())
        );

        let version = Version {
            string: "xxx".to_string(),
        };
        assert_eq!(version.get_valid_version(), None);

        let version = Version {
            string: "1.0-alpha 版本 123123".to_string(),
        };
        assert_eq!(version.get_valid_version(), Some("1.0-alpha".to_string()));
    }
}
