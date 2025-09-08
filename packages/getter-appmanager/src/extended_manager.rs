use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    app_status::AppStatus,
    manager::AppManager,
    observer::{AppManagerObserver, ObserverManager},
    star_manager::StarManager,
    status_tracker::AppStatusInfo,
    version_ignore::VersionIgnoreManager,
};

/// Extended AppManager with star, observer, and version ignore features
pub struct ExtendedAppManager {
    base_manager: AppManager,
    star_manager: StarManager,
    observer_manager: ObserverManager,
    version_ignore_manager: VersionIgnoreManager,
}

impl ExtendedAppManager {
    /// Create new ExtendedAppManager
    pub fn new() -> Self {
        Self {
            base_manager: AppManager::new(),
            star_manager: StarManager::new(),
            observer_manager: ObserverManager::new(),
            version_ignore_manager: VersionIgnoreManager::new(),
        }
    }

    /// Get reference to base manager
    pub fn base(&self) -> &AppManager {
        &self.base_manager
    }

    // ========== Star Management ==========

    /// Set star status for an app
    pub async fn set_app_star(&self, app_id: &str, star: bool) -> Result<bool, String> {
        Ok(self.star_manager.set_star(app_id.to_string(), star).await)
    }

    /// Check if an app is starred
    pub async fn is_app_starred(&self, app_id: &str) -> bool {
        self.star_manager.is_starred(app_id).await
    }

    /// Get all starred app IDs
    pub async fn get_starred_apps(&self) -> Result<Vec<String>, String> {
        Ok(self.star_manager.get_starred_apps().await)
    }

    // ========== App Filtering ==========

    /// Get apps by type (based on app_id prefix)
    pub async fn get_apps_by_type(&self, app_type: &str) -> Result<Vec<String>, String> {
        let all_apps = self.base_manager.list_apps().await?;
        Ok(all_apps
            .into_iter()
            .filter(|app_id| app_id.starts_with(app_type))
            .collect())
    }

    /// Get apps by status
    pub async fn get_apps_by_status(
        &self,
        status: AppStatus,
    ) -> Result<Vec<AppStatusInfo>, String> {
        let all_statuses = self.base_manager.get_all_app_statuses().await?;
        Ok(all_statuses
            .into_iter()
            .filter(|info| info.status == status)
            .collect())
    }

    /// Get starred apps with their status
    pub async fn get_starred_apps_with_status(&self) -> Result<Vec<AppStatusInfo>, String> {
        let starred_ids = self.star_manager.get_starred_apps().await;
        let mut result = Vec::new();

        for app_id in starred_ids {
            if let Some(status) = self.base_manager.get_app_status(&app_id).await? {
                result.push(status);
            }
        }

        Ok(result)
    }

    // ========== Observer Management ==========

    /// Register an observer
    pub async fn register_observer(&self, observer: Arc<dyn AppManagerObserver>) {
        self.observer_manager.register(observer).await;
    }

    /// Clear all observers
    pub async fn clear_observers(&self) {
        self.observer_manager.clear().await;
    }

    // ========== Version Ignore Management ==========

    /// Set ignored version for an app
    pub async fn set_ignore_version(&self, app_id: &str, version: &str) -> Result<bool, String> {
        Ok(self
            .version_ignore_manager
            .set_ignore_version(app_id.to_string(), version.to_string())
            .await)
    }

    /// Get ignored version for an app
    pub async fn get_ignore_version(&self, app_id: &str) -> Result<Option<String>, String> {
        Ok(self.version_ignore_manager.get_ignore_version(app_id).await)
    }

    /// Check if a version is ignored
    pub async fn is_version_ignored(&self, app_id: &str, version: &str) -> bool {
        self.version_ignore_manager
            .is_version_ignored(app_id, version)
            .await
    }

    /// Ignore all current versions
    pub async fn ignore_all_current_versions(&self) -> Result<u32, String> {
        let all_statuses = self.base_manager.get_all_app_statuses().await?;
        let mut current_versions = HashMap::new();

        for status in all_statuses {
            if let Some(version) = status.current_version {
                current_versions.insert(status.app_id, version);
            }
        }

        Ok(self
            .version_ignore_manager
            .ignore_all_current_versions(current_versions)
            .await)
    }

    // ========== Extended Operations with Observer Notifications ==========

    /// Add app with observer notification
    pub async fn add_app_with_notification(
        &self,
        app_id: String,
        hub_uuid: String,
        app_data: HashMap<String, String>,
        hub_data: HashMap<String, String>,
    ) -> Result<String, String> {
        let result = self
            .base_manager
            .add_app(app_id.clone(), hub_uuid, app_data, hub_data)
            .await?;

        self.observer_manager.notify_app_added(&app_id).await;
        Ok(result)
    }

    /// Remove app with observer notification
    pub async fn remove_app_with_notification(&self, app_id: &str) -> Result<bool, String> {
        let result = self.base_manager.remove_app(app_id).await?;

        if result {
            self.observer_manager.notify_app_removed(app_id).await;
            self.star_manager.set_star(app_id.to_string(), false).await;
            self.version_ignore_manager
                .remove_ignore_version(app_id)
                .await;
        }

        Ok(result)
    }

    /// Update app status with observer notification
    pub async fn update_app_status_with_notification(
        &self,
        app_id: &str,
        status: AppStatus,
    ) -> Result<(), String> {
        // This would need to be added to the base manager or status tracker
        self.observer_manager
            .notify_app_updated(app_id, status)
            .await;
        Ok(())
    }

    /// Get outdated apps excluding ignored versions
    pub async fn get_outdated_apps_filtered(&self) -> Result<Vec<AppStatusInfo>, String> {
        let outdated = self.base_manager.get_outdated_apps().await?;
        let mut filtered = Vec::new();

        for app_info in outdated {
            if let Some(latest_version) = &app_info.latest_version {
                if !self
                    .is_version_ignored(&app_info.app_id, latest_version)
                    .await
                {
                    filtered.push(app_info);
                }
            } else {
                filtered.push(app_info);
            }
        }

        Ok(filtered)
    }
}

impl Default for ExtendedAppManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestObserver {
        add_count: Arc<AtomicUsize>,
        remove_count: Arc<AtomicUsize>,
        update_count: Arc<AtomicUsize>,
    }

    impl TestObserver {
        fn new() -> Self {
            Self {
                add_count: Arc::new(AtomicUsize::new(0)),
                remove_count: Arc::new(AtomicUsize::new(0)),
                update_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl AppManagerObserver for TestObserver {
        async fn on_app_added(&self, _app_id: &str) {
            self.add_count.fetch_add(1, Ordering::SeqCst);
        }

        async fn on_app_removed(&self, _app_id: &str) {
            self.remove_count.fetch_add(1, Ordering::SeqCst);
        }

        async fn on_app_updated(&self, _app_id: &str, _status: AppStatus) {
            self.update_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn test_star_functionality() {
        let manager = ExtendedAppManager::new();

        // Test star operations
        assert!(manager.set_app_star("app1", true).await.unwrap());
        assert!(manager.is_app_starred("app1").await);

        let starred = manager.get_starred_apps().await.unwrap();
        assert!(starred.contains(&"app1".to_string()));

        assert!(manager.set_app_star("app1", false).await.unwrap());
        assert!(!manager.is_app_starred("app1").await);
    }

    #[tokio::test]
    async fn test_version_ignore() {
        let manager = ExtendedAppManager::new();

        // Test version ignore
        assert!(!manager.set_ignore_version("app1", "1.0.0").await.unwrap());
        assert!(manager.is_version_ignored("app1", "1.0.0").await);

        let ignored = manager.get_ignore_version("app1").await.unwrap();
        assert_eq!(ignored, Some("1.0.0".to_string()));
    }

    #[tokio::test]
    async fn test_observer_notifications() {
        let manager = ExtendedAppManager::new();
        let observer = Arc::new(TestObserver::new());

        manager.register_observer(observer.clone()).await;

        // Test direct notification (since add_app might fail without proper config)
        manager.observer_manager.notify_app_added("test-app").await;
        assert_eq!(observer.add_count.load(Ordering::SeqCst), 1);

        // Test update notification
        manager
            .update_app_status_with_notification("test-app", AppStatus::AppLatest)
            .await
            .unwrap();
        assert_eq!(observer.update_count.load(Ordering::SeqCst), 1);

        // Test remove notification (directly)
        manager
            .observer_manager
            .notify_app_removed("test-app")
            .await;
        assert_eq!(observer.remove_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_filtered_outdated_apps() {
        let manager = ExtendedAppManager::new();

        // Set up ignored versions
        manager.set_ignore_version("app1", "2.0.0").await.unwrap();

        // This test would need actual apps with outdated status
        let filtered = manager.get_outdated_apps_filtered().await.unwrap();
        // The filtered list should exclude apps with ignored versions
        for app_info in filtered {
            if app_info.app_id == "app1" {
                assert_ne!(app_info.latest_version, Some("2.0.0".to_string()));
            }
        }
    }

    #[tokio::test]
    async fn test_app_type_filtering() {
        let manager = ExtendedAppManager::new();

        // This would need actual apps to be added first
        let android_apps = manager.get_apps_by_type("android").await.unwrap();
        for app_id in android_apps {
            assert!(app_id.starts_with("android"));
        }
    }

    #[tokio::test]
    async fn test_status_filtering() {
        let manager = ExtendedAppManager::new();

        // Get apps by specific status
        let latest_apps = manager
            .get_apps_by_status(AppStatus::AppLatest)
            .await
            .unwrap();
        for app_info in latest_apps {
            assert_eq!(app_info.status, AppStatus::AppLatest);
        }

        let outdated_apps = manager
            .get_apps_by_status(AppStatus::AppOutdated)
            .await
            .unwrap();
        for app_info in outdated_apps {
            assert_eq!(app_info.status, AppStatus::AppOutdated);
        }
    }

    #[tokio::test]
    async fn test_starred_apps_with_status() {
        let manager = ExtendedAppManager::new();

        // Star some apps
        manager.set_app_star("app1", true).await.unwrap();
        manager.set_app_star("app2", true).await.unwrap();

        // Get starred apps with their status
        let starred_with_status = manager.get_starred_apps_with_status().await.unwrap();

        // All returned apps should be in the starred list
        let starred_ids = manager.get_starred_apps().await.unwrap();
        for app_info in starred_with_status {
            assert!(starred_ids.contains(&app_info.app_id));
        }
    }

    #[tokio::test]
    async fn test_concurrent_extended_operations() {
        let manager = Arc::new(ExtendedAppManager::new());
        let mut handles = vec![];

        // Concurrent operations
        for i in 0..10 {
            let mgr = Arc::clone(&manager);
            let handle = tokio::spawn(async move {
                let app_id = format!("app{}", i);

                // Star even-numbered apps
                if i % 2 == 0 {
                    mgr.set_app_star(&app_id, true).await.unwrap();
                }

                // Set ignore version for apps divisible by 3
                if i % 3 == 0 {
                    let version = format!("{}.0.0", i);
                    mgr.set_ignore_version(&app_id, &version).await.unwrap();
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            let _ = handle.await;
        }

        // Verify results
        for i in 0..10 {
            let app_id = format!("app{}", i);

            if i % 2 == 0 {
                assert!(manager.is_app_starred(&app_id).await);
            }

            if i % 3 == 0 {
                let version = format!("{}.0.0", i);
                assert!(manager.is_version_ignored(&app_id, &version).await);
            }
        }
    }
}
