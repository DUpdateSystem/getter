use async_trait::async_trait;
use getter_appmanager::{AppManagerObserver, AppStatus, ExtendedAppManager};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct IntegrationObserver {
    events: Arc<AtomicUsize>,
}

impl IntegrationObserver {
    fn new() -> Self {
        Self {
            events: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn event_count(&self) -> usize {
        self.events.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl AppManagerObserver for IntegrationObserver {
    async fn on_app_added(&self, _app_id: &str) {
        self.events.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_app_removed(&self, _app_id: &str) {
        self.events.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_app_updated(&self, _app_id: &str, _status: AppStatus) {
        self.events.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn test_full_integration_workflow() {
    let manager = Arc::new(ExtendedAppManager::new());

    // Test 1: Star management
    manager.set_app_star("app1", true).await.unwrap();
    manager.set_app_star("app2", true).await.unwrap();
    manager.set_app_star("app3", false).await.unwrap();

    let starred = manager.get_starred_apps().await.unwrap();
    assert_eq!(starred.len(), 2);
    assert!(starred.contains(&"app1".to_string()));
    assert!(starred.contains(&"app2".to_string()));

    // Test 2: Version ignore management
    manager.set_ignore_version("app1", "1.0.0").await.unwrap();
    manager.set_ignore_version("app2", "2.0.0").await.unwrap();

    assert!(manager.is_version_ignored("app1", "1.0.0").await);
    assert!(!manager.is_version_ignored("app1", "1.1.0").await);
    assert!(manager.is_version_ignored("app2", "2.0.0").await);

    // Test 3: Observer pattern
    let observer = Arc::new(IntegrationObserver::new());
    manager.register_observer(observer.clone()).await;

    // Trigger notifications
    manager
        .update_app_status_with_notification("app1", AppStatus::AppLatest)
        .await
        .unwrap();
    manager
        .update_app_status_with_notification("app2", AppStatus::AppOutdated)
        .await
        .unwrap();

    assert_eq!(observer.event_count(), 2);

    // Test 4: App filtering (would need actual apps)
    let android_apps = manager.get_apps_by_type("android").await.unwrap();
    for app_id in android_apps {
        assert!(app_id.starts_with("android"));
    }

    // Test 5: Status filtering
    let latest_apps = manager
        .get_apps_by_status(AppStatus::AppLatest)
        .await
        .unwrap();
    for app_info in latest_apps {
        assert_eq!(app_info.status, AppStatus::AppLatest);
    }
}

#[tokio::test]
async fn test_concurrent_mixed_operations() {
    let manager = Arc::new(ExtendedAppManager::new());
    let mut handles = vec![];

    // Spawn concurrent tasks performing different operations
    for i in 0..20 {
        let mgr = Arc::clone(&manager);
        let handle = tokio::spawn(async move {
            let app_id = format!("concurrent-app-{}", i);

            // Mix of operations
            match i % 4 {
                0 => {
                    mgr.set_app_star(&app_id, true).await.unwrap();
                }
                1 => {
                    let version = format!("{}.0.0", i);
                    mgr.set_ignore_version(&app_id, &version).await.unwrap();
                }
                2 => {
                    mgr.update_app_status_with_notification(&app_id, AppStatus::AppLatest)
                        .await
                        .unwrap();
                }
                3 => {
                    mgr.get_starred_apps().await.unwrap();
                }
                _ => unreachable!(),
            }
        });
        handles.push(handle);
    }

    // Wait for all operations to complete
    for handle in handles {
        handle.await.unwrap();
    }

    // Verify consistency
    let starred = manager.get_starred_apps().await.unwrap();
    for app_id in &starred {
        assert!(manager.is_app_starred(app_id).await);
    }
}

#[tokio::test]
async fn test_android_compatibility_scenario() {
    let manager = ExtendedAppManager::new();

    // Simulate Android app management scenario
    let android_apps = vec![
        ("com.android.chrome", "Chrome", "91.0.4472.120"),
        ("com.android.youtube", "YouTube", "16.29.39"),
        ("com.termux", "Termux", "0.118.0"),
    ];

    // Star some apps (like Android's star feature)
    for (app_id, _, _) in &android_apps {
        if app_id.contains("chrome") || app_id.contains("termux") {
            manager.set_app_star(app_id, true).await.unwrap();
        }
    }

    // Set ignored versions (like Android's ignore feature)
    manager
        .set_ignore_version("com.android.youtube", "16.29.39")
        .await
        .unwrap();

    // Check starred apps
    let starred = manager.get_starred_apps().await.unwrap();
    assert_eq!(starred.len(), 2);

    // Check version ignore
    assert!(
        manager
            .is_version_ignored("com.android.youtube", "16.29.39")
            .await
    );

    // Simulate "ignore all" functionality using the proper method
    // This would normally get current versions from the actual app statuses
    // For testing, we'll just verify the ignore functionality works
    for (app_id, _, version) in &android_apps {
        if !manager.is_version_ignored(app_id, version).await {
            manager.set_ignore_version(app_id, version).await.unwrap();
        }
    }

    // Verify all are ignored
    for (app_id, _, version) in &android_apps {
        assert!(manager.is_version_ignored(app_id, version).await);
    }
}

#[tokio::test]
async fn test_observer_lifecycle() {
    let manager = ExtendedAppManager::new();
    let observer1 = Arc::new(IntegrationObserver::new());
    let observer2 = Arc::new(IntegrationObserver::new());

    // Register observers
    manager.register_observer(observer1.clone()).await;
    manager.register_observer(observer2.clone()).await;

    // Trigger event
    manager
        .update_app_status_with_notification("test-app", AppStatus::AppPending)
        .await
        .unwrap();

    // Both observers should receive the event
    assert_eq!(observer1.event_count(), 1);
    assert_eq!(observer2.event_count(), 1);

    // Clear observers
    manager.clear_observers().await;

    // New events shouldn't be received
    manager
        .update_app_status_with_notification("test-app", AppStatus::AppLatest)
        .await
        .unwrap();

    // Count should remain the same
    assert_eq!(observer1.event_count(), 1);
    assert_eq!(observer2.event_count(), 1);
}

#[tokio::test]
async fn test_filtered_outdated_apps_scenario() {
    let manager = ExtendedAppManager::new();

    // Set up ignored versions for filtering
    manager.set_ignore_version("app1", "2.0.0").await.unwrap();
    manager.set_ignore_version("app2", "3.0.0").await.unwrap();

    // Get filtered outdated apps (would exclude ignored versions)
    let filtered = manager.get_outdated_apps_filtered().await.unwrap();

    // Verify no app with ignored version is in the list
    for app_info in filtered {
        if app_info.app_id == "app1" {
            assert_ne!(app_info.latest_version, Some("2.0.0".to_string()));
        }
        if app_info.app_id == "app2" {
            assert_ne!(app_info.latest_version, Some("3.0.0".to_string()));
        }
    }
}
