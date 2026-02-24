use std::collections::HashMap;

use super::version::{Version, VersionInfo, VersionWrapper};
use crate::websdk::repo::data::release::ReleaseData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubStatus {
    Renewing,
    Error,
    /// Got latest release only (single entry per hub)
    Single,
    /// Got full release list
    Full,
}

/// In-memory version data for a single app, keyed by VersionInfo.
///
/// Mirrors Kotlin's `VersionMap`.
#[derive(Debug)]
pub struct VersionMap {
    pub invalid_version_regex: Option<String>,
    pub include_version_regex: Option<String>,
    /// Aggregated releases, grouped by normalized VersionInfo
    entries: HashMap<VersionInfo, Vec<VersionWrapper>>,
    pub hub_status: HashMap<String, HubStatus>,
    /// Cached sorted list, invalidated on mutation
    sorted_cache: Option<Vec<Version>>,
}

impl VersionMap {
    pub fn new(invalid_regex: Option<String>, include_regex: Option<String>) -> Self {
        Self {
            invalid_version_regex: invalid_regex,
            include_version_regex: include_regex,
            entries: HashMap::new(),
            hub_status: HashMap::new(),
            sorted_cache: None,
        }
    }

    pub fn is_renewing(&self) -> bool {
        self.hub_status.values().any(|s| *s == HubStatus::Renewing)
    }

    pub fn mark_renewing(&mut self, hub_uuid: &str) {
        self.hub_status
            .insert(hub_uuid.to_string(), HubStatus::Renewing);
        self.sorted_cache = None;
    }

    pub fn set_error(&mut self, hub_uuid: &str) {
        self.hub_status
            .insert(hub_uuid.to_string(), HubStatus::Error);
    }

    pub fn add_release_list(&mut self, hub_uuid: &str, releases: Vec<ReleaseData>) {
        for (rel_idx, release) in releases.iter().enumerate() {
            let info = self.make_version_info(&release.version_number);
            let wrapper = VersionWrapper {
                hub_uuid: hub_uuid.to_string(),
                release: release.clone(),
                asset_indices: (0..release.assets.len()).map(|i| (rel_idx, i)).collect(),
            };
            self.entries.entry(info).or_default().push(wrapper);
        }
        self.hub_status
            .insert(hub_uuid.to_string(), HubStatus::Full);
        self.sorted_cache = None;
    }

    pub fn add_single_release(&mut self, hub_uuid: &str, release: ReleaseData) {
        let info = self.make_version_info(&release.version_number);
        let wrapper = VersionWrapper {
            hub_uuid: hub_uuid.to_string(),
            asset_indices: (0..release.assets.len()).map(|i| (0, i)).collect(),
            release,
        };
        self.entries.entry(info).or_default().push(wrapper);
        self.hub_status
            .insert(hub_uuid.to_string(), HubStatus::Single);
        self.sorted_cache = None;
    }

    /// Returns versions sorted descending (newest first).
    pub fn get_version_list(&mut self) -> &[Version] {
        if self.sorted_cache.is_none() {
            let mut versions: Vec<Version> = self
                .entries
                .iter()
                .filter(|(info, _)| !info.name.is_empty())
                .map(|(info, wrappers)| Version {
                    version_info: info.clone(),
                    wrappers: wrappers.clone(),
                })
                .collect();
            versions.sort_by(|a, b| b.version_info.cmp(&a.version_info));
            self.sorted_cache = Some(versions);
        }
        self.sorted_cache.as_deref().unwrap()
    }

    fn make_version_info(&self, raw: &str) -> VersionInfo {
        VersionInfo::new(
            raw,
            self.invalid_version_regex.as_deref(),
            self.include_version_regex.as_deref(),
            HashMap::new(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::websdk::repo::data::release::AssetData;

    fn make_release(version: &str) -> ReleaseData {
        ReleaseData {
            version_number: version.to_string(),
            changelog: String::new(),
            assets: vec![AssetData {
                file_name: "app.apk".to_string(),
                file_type: "apk".to_string(),
                download_url: "https://example.com".to_string(),
            }],
            extra: None,
        }
    }

    #[test]
    fn test_add_and_sort() {
        let mut vm = VersionMap::new(None, None);
        vm.add_release_list(
            "hub1",
            vec![
                make_release("1.0.0"),
                make_release("2.0.0"),
                make_release("1.5.0"),
            ],
        );
        let list = vm.get_version_list();
        assert_eq!(list[0].version_info.name, "2.0.0");
        assert_eq!(list[1].version_info.name, "1.5.0");
        assert_eq!(list[2].version_info.name, "1.0.0");
    }

    #[test]
    fn test_single_release() {
        let mut vm = VersionMap::new(None, None);
        vm.add_single_release("hub1", make_release("3.0.0"));
        let list = vm.get_version_list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].version_info.name, "3.0.0");
        assert_eq!(vm.hub_status["hub1"], HubStatus::Single);
    }

    #[test]
    fn test_hub_status_error() {
        let mut vm = VersionMap::new(None, None);
        vm.set_error("hub1");
        assert_eq!(vm.hub_status["hub1"], HubStatus::Error);
    }

    #[test]
    fn test_is_renewing() {
        let mut vm = VersionMap::new(None, None);
        assert!(!vm.is_renewing());
        vm.mark_renewing("hub1");
        assert!(vm.is_renewing());
        vm.set_error("hub1");
        assert!(!vm.is_renewing());
    }

    #[test]
    fn test_dedup_versions_across_hubs() {
        let mut vm = VersionMap::new(None, None);
        vm.add_single_release("hub1", make_release("1.0.0"));
        vm.add_single_release("hub2", make_release("1.0.0"));
        // Same version from two hubs → merged under one VersionInfo key
        let list = vm.get_version_list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].wrappers.len(), 2);
    }

    #[test]
    fn test_regex_filtering() {
        let mut vm = VersionMap::new(Some("^v".to_string()), None);
        vm.add_single_release("hub1", make_release("v1.2.3"));
        let list = vm.get_version_list();
        assert_eq!(list[0].version_info.name, "1.2.3");
    }
}
