use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Manager for ignored app versions
#[derive(Clone)]
pub struct VersionIgnoreManager {
    ignored_versions: Arc<Mutex<HashMap<String, String>>>,
}

impl VersionIgnoreManager {
    pub fn new() -> Self {
        Self {
            ignored_versions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Set ignored version for an app
    pub async fn set_ignore_version(&self, app_id: String, version: String) -> bool {
        let mut ignored = self.ignored_versions.lock().await;
        ignored.insert(app_id, version).is_some()
    }

    /// Remove ignored version for an app
    pub async fn remove_ignore_version(&self, app_id: &str) -> bool {
        let mut ignored = self.ignored_versions.lock().await;
        ignored.remove(app_id).is_some()
    }

    /// Get ignored version for an app
    pub async fn get_ignore_version(&self, app_id: &str) -> Option<String> {
        let ignored = self.ignored_versions.lock().await;
        ignored.get(app_id).cloned()
    }

    /// Check if a version is ignored for an app
    pub async fn is_version_ignored(&self, app_id: &str, version: &str) -> bool {
        let ignored = self.ignored_versions.lock().await;
        ignored.get(app_id).is_some_and(|v| v == version)
    }

    /// Ignore all current versions for all apps
    /// Returns the number of apps whose versions were ignored
    pub async fn ignore_all_current_versions(
        &self,
        current_versions: HashMap<String, String>,
    ) -> u32 {
        let mut ignored = self.ignored_versions.lock().await;
        let mut count = 0;

        for (app_id, version) in current_versions {
            if ignored.insert(app_id, version).is_none() {
                count += 1;
            }
        }

        count
    }

    /// Get all ignored versions
    pub async fn get_all_ignored(&self) -> HashMap<String, String> {
        let ignored = self.ignored_versions.lock().await;
        ignored.clone()
    }

    /// Clear all ignored versions
    pub async fn clear_all(&self) {
        let mut ignored = self.ignored_versions.lock().await;
        ignored.clear();
    }

    /// Get the count of ignored versions
    pub async fn count(&self) -> usize {
        let ignored = self.ignored_versions.lock().await;
        ignored.len()
    }
}

impl Default for VersionIgnoreManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_version_ignore_basic() {
        let manager = VersionIgnoreManager::new();

        // Test setting ignored version
        assert!(
            !manager
                .set_ignore_version("app1".to_string(), "1.0.0".to_string())
                .await
        );
        assert!(manager.is_version_ignored("app1", "1.0.0").await);
        assert!(!manager.is_version_ignored("app1", "1.1.0").await);

        // Test getting ignored version
        let version = manager.get_ignore_version("app1").await;
        assert_eq!(version, Some("1.0.0".to_string()));

        // Test removing ignored version
        assert!(manager.remove_ignore_version("app1").await);
        assert!(!manager.is_version_ignored("app1", "1.0.0").await);
        assert!(!manager.remove_ignore_version("app1").await);
    }

    #[tokio::test]
    async fn test_ignore_all_current_versions() {
        let manager = VersionIgnoreManager::new();

        let mut current_versions = HashMap::new();
        current_versions.insert("app1".to_string(), "1.0.0".to_string());
        current_versions.insert("app2".to_string(), "2.0.0".to_string());
        current_versions.insert("app3".to_string(), "3.0.0".to_string());

        let count = manager.ignore_all_current_versions(current_versions).await;
        assert_eq!(count, 3);

        // Verify all versions are ignored
        assert!(manager.is_version_ignored("app1", "1.0.0").await);
        assert!(manager.is_version_ignored("app2", "2.0.0").await);
        assert!(manager.is_version_ignored("app3", "3.0.0").await);

        assert_eq!(manager.count().await, 3);
    }

    #[tokio::test]
    async fn test_get_all_ignored() {
        let manager = VersionIgnoreManager::new();

        manager
            .set_ignore_version("app1".to_string(), "1.0.0".to_string())
            .await;
        manager
            .set_ignore_version("app2".to_string(), "2.0.0".to_string())
            .await;

        let all_ignored = manager.get_all_ignored().await;
        assert_eq!(all_ignored.len(), 2);
        assert_eq!(all_ignored.get("app1"), Some(&"1.0.0".to_string()));
        assert_eq!(all_ignored.get("app2"), Some(&"2.0.0".to_string()));
    }

    #[tokio::test]
    async fn test_clear_all() {
        let manager = VersionIgnoreManager::new();

        manager
            .set_ignore_version("app1".to_string(), "1.0.0".to_string())
            .await;
        manager
            .set_ignore_version("app2".to_string(), "2.0.0".to_string())
            .await;

        assert_eq!(manager.count().await, 2);

        manager.clear_all().await;
        assert_eq!(manager.count().await, 0);
        assert!(manager.get_ignore_version("app1").await.is_none());
        assert!(manager.get_ignore_version("app2").await.is_none());
    }

    #[tokio::test]
    async fn test_concurrent_operations() {
        let manager = Arc::new(VersionIgnoreManager::new());
        let mut handles = vec![];

        // Concurrent set operations
        for i in 0..10 {
            let mgr = Arc::clone(&manager);
            let handle = tokio::spawn(async move {
                let app_id = format!("app{}", i);
                let version = format!("{}.0.0", i);
                mgr.set_ignore_version(app_id, version).await
            });
            handles.push(handle);
        }

        for handle in handles {
            let _ = handle.await;
        }

        // Verify all versions are set
        assert_eq!(manager.count().await, 10);

        for i in 0..10 {
            let app_id = format!("app{}", i);
            let version = format!("{}.0.0", i);
            assert!(manager.is_version_ignored(&app_id, &version).await);
        }
    }

    #[tokio::test]
    async fn test_update_ignored_version() {
        let manager = VersionIgnoreManager::new();

        // Set initial version
        manager
            .set_ignore_version("app1".to_string(), "1.0.0".to_string())
            .await;
        assert!(manager.is_version_ignored("app1", "1.0.0").await);

        // Update to new version
        assert!(
            manager
                .set_ignore_version("app1".to_string(), "2.0.0".to_string())
                .await
        );
        assert!(!manager.is_version_ignored("app1", "1.0.0").await);
        assert!(manager.is_version_ignored("app1", "2.0.0").await);
    }
}
