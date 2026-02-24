use super::app_status::AppStatus;
use super::version::VersionInfo;
use super::version_map::{HubStatus, VersionMap};
use std::collections::HashMap;

/// Determines the release status for an app given its version map and local version.
///
/// Mirrors Kotlin's `Updater.getReleaseStatus()`.
pub fn get_release_status(
    version_map: &mut VersionMap,
    local_version: Option<&str>,
    ignore_version: Option<&str>,
    is_saved: bool,
) -> AppStatus {
    let versions = version_map.get_version_list();

    if versions.is_empty() {
        if version_map.is_renewing() {
            return AppStatus::AppPending;
        }
        let all_error = !version_map.hub_status.is_empty()
            && version_map
                .hub_status
                .values()
                .all(|s| *s == HubStatus::Error);
        if all_error || is_saved {
            return AppStatus::NetworkError;
        }
        return AppStatus::AppInactive;
    }

    let latest_name = &versions[0].version_info.name;

    // If the latest version matches what the user chose to ignore
    if let Some(ignored) = ignore_version {
        if ignored == latest_name {
            return AppStatus::AppLatest;
        }
    }

    let effective_local = local_version.or(ignore_version);

    match effective_local {
        None => AppStatus::AppNoLocal,
        Some(local_str) => {
            let local_info = VersionInfo::new(local_str, None, None, HashMap::new());
            let latest_info = &versions[0].version_info;
            if is_latest(&local_info, latest_info) {
                AppStatus::AppLatest
            } else {
                AppStatus::AppOutdated
            }
        }
    }
}

fn is_latest(local: &VersionInfo, latest: &VersionInfo) -> bool {
    use std::cmp::Ordering;
    match local.compare(latest) {
        Some(Ordering::Greater) | Some(Ordering::Equal) => true,
        Some(Ordering::Less) => false,
        None => {
            // Fallback: string equality check
            local.name == latest.name
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::version_map::VersionMap;
    use crate::websdk::repo::data::release::{AssetData, ReleaseData};

    fn release(v: &str) -> ReleaseData {
        ReleaseData {
            version_number: v.to_string(),
            changelog: String::new(),
            assets: vec![AssetData {
                file_name: "app.apk".to_string(),
                file_type: "apk".to_string(),
                download_url: "https://x.com".to_string(),
            }],
            extra: None,
        }
    }

    fn vm_with(versions: &[&str]) -> VersionMap {
        let mut vm = VersionMap::new(None, None);
        vm.add_release_list("hub1", versions.iter().map(|v| release(v)).collect());
        vm
    }

    #[test]
    fn test_latest() {
        let mut vm = vm_with(&["2.0.0", "1.0.0"]);
        let status = get_release_status(&mut vm, Some("2.0.0"), None, true);
        assert_eq!(status, AppStatus::AppLatest);
    }

    #[test]
    fn test_outdated() {
        let mut vm = vm_with(&["2.0.0", "1.0.0"]);
        let status = get_release_status(&mut vm, Some("1.0.0"), None, true);
        assert_eq!(status, AppStatus::AppOutdated);
    }

    #[test]
    fn test_no_local() {
        let mut vm = vm_with(&["2.0.0"]);
        let status = get_release_status(&mut vm, None, None, true);
        assert_eq!(status, AppStatus::AppNoLocal);
    }

    #[test]
    fn test_ignored_version() {
        let mut vm = vm_with(&["2.0.0"]);
        let status = get_release_status(&mut vm, None, Some("2.0.0"), true);
        assert_eq!(status, AppStatus::AppLatest);
    }

    #[test]
    fn test_network_error() {
        let mut vm = VersionMap::new(None, None);
        vm.set_error("hub1");
        let status = get_release_status(&mut vm, Some("1.0.0"), None, true);
        assert_eq!(status, AppStatus::NetworkError);
    }

    #[test]
    fn test_pending() {
        let mut vm = VersionMap::new(None, None);
        vm.mark_renewing("hub1");
        let status = get_release_status(&mut vm, Some("1.0.0"), None, true);
        assert_eq!(status, AppStatus::AppPending);
    }

    #[test]
    fn test_inactive_unsaved() {
        let mut vm = VersionMap::new(None, None);
        let status = get_release_status(&mut vm, Some("1.0.0"), None, false);
        assert_eq!(status, AppStatus::AppInactive);
    }

    #[test]
    fn test_local_newer_than_remote() {
        let mut vm = vm_with(&["1.0.0"]);
        let status = get_release_status(&mut vm, Some("2.0.0"), None, true);
        assert_eq!(status, AppStatus::AppLatest);
    }
}
