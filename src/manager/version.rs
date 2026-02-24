use crate::utils::versioning::Version as VersionUtil;
use crate::websdk::repo::data::release::ReleaseData;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A parsed, comparable version identifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    /// Normalized version name (regex-filtered)
    pub name: String,
    /// Extra metadata (e.g. version_code from Android)
    pub extra: HashMap<String, serde_json::Value>,
}

impl VersionInfo {
    pub fn new(
        raw_name: &str,
        invalid_regex: Option<&str>,
        include_regex: Option<&str>,
        extra: HashMap<String, serde_json::Value>,
    ) -> Self {
        let name = normalize_version(raw_name, invalid_regex, include_regex);
        Self { name, extra }
    }

    /// Compare using libversion. Returns Some(Ordering) if both are parseable.
    pub fn compare(&self, other: &VersionInfo) -> Option<std::cmp::Ordering> {
        let v1 = VersionUtil::new(self.name.clone());
        let v2 = VersionUtil::new(other.name.clone());
        v1.partial_cmp(&v2)
    }
}

impl PartialEq for VersionInfo {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for VersionInfo {}

impl std::hash::Hash for VersionInfo {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl PartialOrd for VersionInfo {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VersionInfo {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.compare(other).unwrap_or(std::cmp::Ordering::Less)
    }
}

/// Strip unwanted parts from a version string using optional regex filters.
fn normalize_version(
    raw: &str,
    invalid_regex: Option<&str>,
    include_regex: Option<&str>,
) -> String {
    let mut result = raw.to_string();

    if let Some(pattern) = invalid_regex {
        if let Ok(re) = regex::Regex::new(pattern) {
            result = re.replace_all(&result, "").to_string();
        }
    }

    if let Some(pattern) = include_regex {
        if let Ok(re) = regex::Regex::new(pattern) {
            let matched: Vec<&str> = re.find_iter(&result).map(|m| m.as_str()).collect();
            result = matched.join("");
        }
    }

    result.trim().to_string()
}

/// A release from one hub, paired with its assets.
#[derive(Debug, Clone)]
pub struct VersionWrapper {
    pub hub_uuid: String,
    pub release: ReleaseData,
    /// (release_index, asset_index) pairs
    pub asset_indices: Vec<(usize, usize)>,
}

/// Snapshot of a single version with all hub-provided wrappers.
#[derive(Debug, Clone)]
pub struct Version {
    pub version_info: VersionInfo,
    pub wrappers: Vec<VersionWrapper>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_info_compare() {
        let v1 = VersionInfo::new("1.0.0", None, None, HashMap::new());
        let v2 = VersionInfo::new("1.0.1", None, None, HashMap::new());
        assert!(v1 < v2);
        assert!(v2 > v1);
    }

    #[test]
    fn test_version_info_equal() {
        let v1 = VersionInfo::new("1.0.0", None, None, HashMap::new());
        let v2 = VersionInfo::new("1.0.0", None, None, HashMap::new());
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_normalize_version_invalid_regex() {
        let name = normalize_version("v1.0.0", Some("^v"), None);
        assert_eq!(name, "1.0.0");
    }

    #[test]
    fn test_normalize_version_include_regex() {
        let name = normalize_version("Release 1.0.0 (stable)", None, Some(r"\d+\.\d+\.\d+"));
        assert_eq!(name, "1.0.0");
    }

    #[test]
    fn test_normalize_version_both_filters() {
        let name = normalize_version("v1.0.0-beta", Some(r"-beta"), Some(r"\d+\.\d+\.\d+"));
        assert_eq!(name, "1.0.0");
    }
}
