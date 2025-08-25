use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use serde::{Deserialize, Serialize};

use crate::app_status::AppStatus;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStatusInfo {
    pub app_id: String,
    pub status: AppStatus,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub last_checked: Option<u64>,
}

#[derive(Clone)]
pub struct StatusTracker {
    statuses: Arc<Mutex<HashMap<String, AppStatusInfo>>>,
}

impl StatusTracker {
    pub fn new() -> Self {
        Self {
            statuses: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn add_app(&self, app_id: String) {
        let mut statuses = self.statuses.lock().await;
        let info = AppStatusInfo {
            app_id: app_id.clone(),
            status: AppStatus::AppInactive,
            current_version: None,
            latest_version: None,
            last_checked: None,
        };
        statuses.insert(app_id, info);
    }

    pub async fn remove_app(&self, app_id: &str) {
        let mut statuses = self.statuses.lock().await;
        statuses.remove(app_id);
    }

    pub async fn update_status(&self, app_id: &str, status: AppStatus) {
        let mut statuses = self.statuses.lock().await;
        if let Some(info) = statuses.get_mut(app_id) {
            info.status = status;
            info.last_checked = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            );
        }
    }

    pub async fn set_versions(&self, app_id: &str, current: Option<String>, latest: Option<String>) {
        let mut statuses = self.statuses.lock().await;
        if let Some(info) = statuses.get_mut(app_id) {
            info.current_version = current;
            info.latest_version = latest;
        }
    }

    pub async fn get_status(&self, app_id: &str) -> Option<AppStatusInfo> {
        let statuses = self.statuses.lock().await;
        statuses.get(app_id).cloned()
    }

    pub async fn get_all_statuses(&self) -> Vec<AppStatusInfo> {
        let statuses = self.statuses.lock().await;
        statuses.values().cloned().collect()
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

    #[tokio::test]
    async fn test_status_tracker() {
        let tracker = StatusTracker::new();

        // Add an app
        tracker.add_app("test-app".to_string()).await;

        // Check initial status
        let status = tracker.get_status("test-app").await;
        assert!(status.is_some());
        let status = status.unwrap();
        assert_eq!(status.app_id, "test-app");
        assert_eq!(status.status, AppStatus::AppInactive);

        // Update status
        tracker.update_status("test-app", AppStatus::AppLatest).await;
        let status = tracker.get_status("test-app").await.unwrap();
        assert_eq!(status.status, AppStatus::AppLatest);
        assert!(status.last_checked.is_some());

        // Set versions
        tracker
            .set_versions(
                "test-app",
                Some("1.0.0".to_string()),
                Some("1.1.0".to_string()),
            )
            .await;
        let status = tracker.get_status("test-app").await.unwrap();
        assert_eq!(status.current_version.as_deref(), Some("1.0.0"));
        assert_eq!(status.latest_version.as_deref(), Some("1.1.0"));

        // Remove app
        tracker.remove_app("test-app").await;
        assert!(tracker.get_status("test-app").await.is_none());
    }

    #[tokio::test]
    async fn test_get_all_statuses() {
        let tracker = StatusTracker::new();

        tracker.add_app("app1".to_string()).await;
        tracker.add_app("app2".to_string()).await;

        let all_statuses = tracker.get_all_statuses().await;
        assert_eq!(all_statuses.len(), 2);

        let app_ids: Vec<String> = all_statuses.iter().map(|s| s.app_id.clone()).collect();
        assert!(app_ids.contains(&"app1".to_string()));
        assert!(app_ids.contains(&"app2".to_string()));
    }
}