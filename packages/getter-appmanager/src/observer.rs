use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::app_status::AppStatus;

/// Observer trait for app manager events
#[async_trait]
pub trait AppManagerObserver: Send + Sync {
    /// Called when an app is added
    async fn on_app_added(&self, app_id: &str);

    /// Called when an app is removed
    async fn on_app_removed(&self, app_id: &str);

    /// Called when an app status is updated
    async fn on_app_updated(&self, app_id: &str, status: AppStatus);
}

/// Update notification types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateNotification {
    AppAdded(String),
    AppRemoved(String),
    AppUpdated(String, AppStatus),
}

/// Observable manager for app events
#[derive(Clone)]
pub struct ObserverManager {
    observers: Arc<Mutex<Vec<Arc<dyn AppManagerObserver>>>>,
}

impl ObserverManager {
    pub fn new() -> Self {
        Self {
            observers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Register an observer
    pub async fn register(&self, observer: Arc<dyn AppManagerObserver>) {
        let mut observers = self.observers.lock().await;
        observers.push(observer);
    }

    /// Unregister all observers
    pub async fn clear(&self) {
        let mut observers = self.observers.lock().await;
        observers.clear();
    }

    /// Notify all observers of an app addition
    pub async fn notify_app_added(&self, app_id: &str) {
        let observers = self.observers.lock().await;
        for observer in observers.iter() {
            observer.on_app_added(app_id).await;
        }
    }

    /// Notify all observers of an app removal
    pub async fn notify_app_removed(&self, app_id: &str) {
        let observers = self.observers.lock().await;
        for observer in observers.iter() {
            observer.on_app_removed(app_id).await;
        }
    }

    /// Notify all observers of an app update
    pub async fn notify_app_updated(&self, app_id: &str, status: AppStatus) {
        let observers = self.observers.lock().await;
        for observer in observers.iter() {
            observer.on_app_updated(app_id, status).await;
        }
    }

    /// Get the number of registered observers
    pub async fn observer_count(&self) -> usize {
        let observers = self.observers.lock().await;
        observers.len()
    }
}

impl Default for ObserverManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    async fn test_observer_notifications() {
        let manager = ObserverManager::new();
        let observer = Arc::new(TestObserver::new());

        manager.register(observer.clone()).await;
        assert_eq!(manager.observer_count().await, 1);

        // Test notifications
        manager.notify_app_added("app1").await;
        assert_eq!(observer.add_count.load(Ordering::SeqCst), 1);

        manager.notify_app_removed("app1").await;
        assert_eq!(observer.remove_count.load(Ordering::SeqCst), 1);

        manager
            .notify_app_updated("app1", AppStatus::AppLatest)
            .await;
        assert_eq!(observer.update_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_multiple_observers() {
        let manager = ObserverManager::new();
        let observer1 = Arc::new(TestObserver::new());
        let observer2 = Arc::new(TestObserver::new());

        manager.register(observer1.clone()).await;
        manager.register(observer2.clone()).await;
        assert_eq!(manager.observer_count().await, 2);

        manager.notify_app_added("app1").await;
        assert_eq!(observer1.add_count.load(Ordering::SeqCst), 1);
        assert_eq!(observer2.add_count.load(Ordering::SeqCst), 1);

        manager.clear().await;
        assert_eq!(manager.observer_count().await, 0);
    }

    #[tokio::test]
    async fn test_concurrent_notifications() {
        let manager = Arc::new(ObserverManager::new());
        let observer = Arc::new(TestObserver::new());

        manager.register(observer.clone()).await;

        let mut handles = vec![];

        for i in 0..10 {
            let mgr = Arc::clone(&manager);
            let handle = tokio::spawn(async move {
                let app_id = format!("app{}", i);
                mgr.notify_app_added(&app_id).await;
                mgr.notify_app_updated(&app_id, AppStatus::AppLatest).await;
            });
            handles.push(handle);
        }

        for handle in handles {
            let _ = handle.await;
        }

        assert_eq!(observer.add_count.load(Ordering::SeqCst), 10);
        assert_eq!(observer.update_count.load(Ordering::SeqCst), 10);
    }
}
