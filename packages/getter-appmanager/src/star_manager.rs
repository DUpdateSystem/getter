use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Manager for app star/favorite status
#[derive(Clone)]
pub struct StarManager {
    starred_apps: Arc<Mutex<HashSet<String>>>,
}

impl StarManager {
    pub fn new() -> Self {
        Self {
            starred_apps: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Set star status for an app
    pub async fn set_star(&self, app_id: String, star: bool) -> bool {
        let mut starred = self.starred_apps.lock().await;
        if star {
            starred.insert(app_id)
        } else {
            starred.remove(&app_id)
        }
    }

    /// Check if an app is starred
    pub async fn is_starred(&self, app_id: &str) -> bool {
        let starred = self.starred_apps.lock().await;
        starred.contains(app_id)
    }

    /// Get all starred app IDs
    pub async fn get_starred_apps(&self) -> Vec<String> {
        let starred = self.starred_apps.lock().await;
        starred.iter().cloned().collect()
    }

    /// Clear all stars
    pub async fn clear_all_stars(&self) {
        let mut starred = self.starred_apps.lock().await;
        starred.clear();
    }
}

impl Default for StarManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_star_manager() {
        let manager = StarManager::new();

        // Test setting star
        assert!(manager.set_star("app1".to_string(), true).await);
        assert!(manager.is_starred("app1").await);

        // Test removing star
        assert!(manager.set_star("app1".to_string(), false).await);
        assert!(!manager.is_starred("app1").await);

        // Test multiple stars
        manager.set_star("app2".to_string(), true).await;
        manager.set_star("app3".to_string(), true).await;

        let starred = manager.get_starred_apps().await;
        assert_eq!(starred.len(), 2);
        assert!(starred.contains(&"app2".to_string()));
        assert!(starred.contains(&"app3".to_string()));

        // Test clear all
        manager.clear_all_stars().await;
        let starred = manager.get_starred_apps().await;
        assert!(starred.is_empty());
    }

    #[tokio::test]
    async fn test_concurrent_star_operations() {
        let manager = Arc::new(StarManager::new());
        let mut handles = vec![];

        // Concurrent star operations
        for i in 0..10 {
            let mgr = Arc::clone(&manager);
            let handle = tokio::spawn(async move {
                let app_id = format!("app{}", i);
                mgr.set_star(app_id, i % 2 == 0).await
            });
            handles.push(handle);
        }

        for handle in handles {
            let _ = handle.await;
        }

        // Check results
        for i in 0..10 {
            let app_id = format!("app{}", i);
            let expected = i % 2 == 0;
            assert_eq!(manager.is_starred(&app_id).await, expected);
        }
    }
}
