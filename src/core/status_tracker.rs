use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::core::app_status::AppStatus;
use crate::utils::versioning::Version;
use crate::websdk::repo::data::release::ReleaseData;

/// Information about a tracked app's current state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStatusInfo {
    pub app_id: String,
    pub status: AppStatus,
    pub local_version: Option<String>,
    pub latest_version: Option<String>,
    pub last_checked: Option<chrono::DateTime<chrono::Utc>>,
    pub error_message: Option<String>,
}

impl AppStatusInfo {
    pub fn new(app_id: String) -> Self {
        Self {
            app_id,
            status: AppStatus::AppInactive,
            local_version: None,
            latest_version: None,
            last_checked: None,
            error_message: None,
        }
    }

    /// Update status with latest release information
    pub fn update_with_release(&mut self, release: Option<ReleaseData>) {
        self.last_checked = Some(chrono::Utc::now());
        self.error_message = None;

        match release {
            Some(release_data) => {
                self.latest_version = Some(release_data.version_number.clone());
                self.status = self.calculate_status();
            }
            None => {
                self.status = AppStatus::NetworkError;
                self.error_message = Some("No release data available".to_string());
            }
        }
    }

    /// Update status with error information
    pub fn update_with_error(&mut self, error: String) {
        self.last_checked = Some(chrono::Utc::now());
        self.status = AppStatus::NetworkError;
        self.error_message = Some(error);
    }

    /// Set the local version
    pub fn set_local_version(&mut self, version: Option<String>) {
        self.local_version = version;
        self.status = self.calculate_status();
    }

    /// Mark as pending (checking for updates)
    pub fn set_pending(&mut self) {
        self.status = AppStatus::AppPending;
        self.error_message = None;
    }

    /// Calculate the current status based on version comparison
    fn calculate_status(&self) -> AppStatus {
        match (&self.local_version, &self.latest_version) {
            (None, None) => AppStatus::AppInactive,
            (None, Some(_)) => AppStatus::AppNoLocal,
            (Some(_), None) => AppStatus::NetworkError,
            (Some(local), Some(latest)) => {
                let local_version = Version::new(local.clone());
                let latest_version = Version::new(latest.clone());

                match local_version.partial_cmp(&latest_version) {
                    Some(std::cmp::Ordering::Less) => AppStatus::AppOutdated,
                    Some(std::cmp::Ordering::Equal) => AppStatus::AppLatest,
                    Some(std::cmp::Ordering::Greater) => AppStatus::AppLatest, // Local is newer
                    None => AppStatus::NetworkError, // Version comparison failed
                }
            }
        }
    }
}

/// Global status tracker for all managed applications
#[derive(Debug, Clone)]
pub struct StatusTracker {
    statuses: Arc<RwLock<HashMap<String, AppStatusInfo>>>,
}

impl StatusTracker {
    pub fn new() -> Self {
        Self {
            statuses: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a new app to tracking
    pub async fn add_app(&self, app_id: String) {
        let mut statuses = self.statuses.write().await;
        statuses.insert(app_id.clone(), AppStatusInfo::new(app_id));
    }

    /// Remove an app from tracking
    pub async fn remove_app(&self, app_id: &str) -> bool {
        let mut statuses = self.statuses.write().await;
        statuses.remove(app_id).is_some()
    }

    /// Get status for a specific app
    pub async fn get_status(&self, app_id: &str) -> Option<AppStatusInfo> {
        let statuses = self.statuses.read().await;
        statuses.get(app_id).cloned()
    }

    /// Get statuses for all apps
    pub async fn get_all_statuses(&self) -> Vec<AppStatusInfo> {
        let statuses = self.statuses.read().await;
        statuses.values().cloned().collect()
    }

    /// Get apps with specific status
    pub async fn get_apps_with_status(&self, status: AppStatus) -> Vec<AppStatusInfo> {
        let statuses = self.statuses.read().await;
        statuses
            .values()
            .filter(|info| info.status == status)
            .cloned()
            .collect()
    }

    /// Mark app as pending (checking for updates)
    pub async fn set_pending(&self, app_id: &str) {
        let mut statuses = self.statuses.write().await;
        if let Some(info) = statuses.get_mut(app_id) {
            info.set_pending();
        }
    }

    /// Update app status with latest release
    pub async fn update_with_release(&self, app_id: &str, release: Option<ReleaseData>) {
        let mut statuses = self.statuses.write().await;
        if let Some(info) = statuses.get_mut(app_id) {
            info.update_with_release(release);
        }
    }

    /// Update app status with error
    pub async fn update_with_error(&self, app_id: &str, error: String) {
        let mut statuses = self.statuses.write().await;
        if let Some(info) = statuses.get_mut(app_id) {
            info.update_with_error(error);
        }
    }

    /// Set local version for an app
    pub async fn set_local_version(&self, app_id: &str, version: Option<String>) {
        let mut statuses = self.statuses.write().await;
        if let Some(info) = statuses.get_mut(app_id) {
            info.set_local_version(version);
        }
    }

    /// Get count of apps by status
    pub async fn get_status_counts(&self) -> HashMap<AppStatus, usize> {
        let statuses = self.statuses.read().await;
        let mut counts = HashMap::new();

        for info in statuses.values() {
            *counts.entry(info.status).or_insert(0) += 1;
        }

        counts
    }

    /// Get apps that need updates
    pub async fn get_outdated_apps(&self) -> Vec<AppStatusInfo> {
        self.get_apps_with_status(AppStatus::AppOutdated).await
    }

    /// Get apps with errors
    pub async fn get_error_apps(&self) -> Vec<AppStatusInfo> {
        self.get_apps_with_status(AppStatus::NetworkError).await
    }

    /// Clear all statuses
    pub async fn clear(&self) {
        let mut statuses = self.statuses.write().await;
        statuses.clear();
    }
}

impl Default for StatusTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_status_info_new() {
        let info = AppStatusInfo::new("test-app".to_string());
        assert_eq!(info.app_id, "test-app");
        assert_eq!(info.status, AppStatus::AppInactive);
        assert!(info.local_version.is_none());
        assert!(info.latest_version.is_none());
        assert!(info.last_checked.is_none());
        assert!(info.error_message.is_none());
    }

    #[test]
    fn test_app_status_info_calculate_status() {
        let mut info = AppStatusInfo::new("test-app".to_string());

        // No versions
        assert_eq!(info.status, AppStatus::AppInactive);

        // Only latest version
        info.latest_version = Some("1.0.0".to_string());
        info.status = info.calculate_status();
        assert_eq!(info.status, AppStatus::AppNoLocal);

        // Only local version
        info.latest_version = None;
        info.local_version = Some("1.0.0".to_string());
        info.status = info.calculate_status();
        assert_eq!(info.status, AppStatus::NetworkError);

        // Same versions
        info.latest_version = Some("1.0.0".to_string());
        info.status = info.calculate_status();
        assert_eq!(info.status, AppStatus::AppLatest);

        // Outdated local version
        info.local_version = Some("0.9.0".to_string());
        info.status = info.calculate_status();
        assert_eq!(info.status, AppStatus::AppOutdated);
    }

    #[tokio::test]
    async fn test_status_tracker_basic_operations() {
        let tracker = StatusTracker::new();

        // Add app
        tracker.add_app("test-app".to_string()).await;
        let status = tracker.get_status("test-app").await;
        assert!(status.is_some());
        assert_eq!(status.unwrap().app_id, "test-app");

        // Remove app
        let removed = tracker.remove_app("test-app").await;
        assert!(removed);

        let status = tracker.get_status("test-app").await;
        assert!(status.is_none());
    }

    #[tokio::test]
    async fn test_status_tracker_status_updates() {
        let tracker = StatusTracker::new();
        tracker.add_app("test-app".to_string()).await;

        // Set pending
        tracker.set_pending("test-app").await;
        let status = tracker.get_status("test-app").await.unwrap();
        assert_eq!(status.status, AppStatus::AppPending);

        // Update with error
        tracker
            .update_with_error("test-app", "Network error".to_string())
            .await;
        let status = tracker.get_status("test-app").await.unwrap();
        assert_eq!(status.status, AppStatus::NetworkError);
        assert_eq!(status.error_message, Some("Network error".to_string()));

        // Set local version
        tracker
            .set_local_version("test-app", Some("1.0.0".to_string()))
            .await;
        let status = tracker.get_status("test-app").await.unwrap();
        assert_eq!(status.local_version, Some("1.0.0".to_string()));
    }

    #[tokio::test]
    async fn test_status_tracker_filtering() {
        let tracker = StatusTracker::new();

        // Add multiple apps with different statuses
        tracker.add_app("app1".to_string()).await;
        tracker.add_app("app2".to_string()).await;
        tracker.add_app("app3".to_string()).await;

        tracker.set_pending("app1").await;
        tracker.update_with_error("app2", "Error".to_string()).await;

        // Get apps by status
        let pending_apps = tracker.get_apps_with_status(AppStatus::AppPending).await;
        assert_eq!(pending_apps.len(), 1);
        assert_eq!(pending_apps[0].app_id, "app1");

        let error_apps = tracker.get_error_apps().await;
        assert_eq!(error_apps.len(), 1);
        assert_eq!(error_apps[0].app_id, "app2");

        // Get status counts
        let counts = tracker.get_status_counts().await;
        assert_eq!(counts.get(&AppStatus::AppPending), Some(&1));
        assert_eq!(counts.get(&AppStatus::NetworkError), Some(&1));
        assert_eq!(counts.get(&AppStatus::AppInactive), Some(&1));
    }
}
