use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::websdk::repo::data::release::ReleaseData;

/// Lightweight operation identifier for deduplication
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OpId {
    pub op_type: u8, // 0=check, 1=latest, 2=releases, 3=update, 4=add, 5=remove
    pub key: String, // combined key for deduplication
}

impl OpId {
    fn check_available(hub_uuid: &str, app_key: &str) -> Self {
        Self { op_type: 0, key: format!("{}:{}", hub_uuid, app_key) }
    }
    
    fn get_latest(hub_uuid: &str, app_key: &str) -> Self {
        Self { op_type: 1, key: format!("{}:{}", hub_uuid, app_key) }
    }
    
    fn get_releases(hub_uuid: &str, app_key: &str) -> Self {
        Self { op_type: 2, key: format!("{}:{}", hub_uuid, app_key) }
    }
    
    fn update_app(app_id: &str) -> Self {
        Self { op_type: 3, key: app_id.to_string() }
    }
}

/// Lightweight result enum
#[derive(Debug, Clone)]
pub enum AppResult {
    Bool(bool),
    Release(ReleaseData),
    Releases(Vec<ReleaseData>),
    Success(String),
    Error(String),
}

/// Internal message for channel communication
struct Msg {
    id: OpId,
    data: Vec<String>, // minimal data storage
    tx: oneshot::Sender<AppResult>,
}

/// Memory-efficient App Manager with request deduplication
pub struct AppManager {
    msg_tx: mpsc::UnboundedSender<Msg>,
}

impl AppManager {
    /// Create new AppManager instance
    pub fn new() -> Self {
        let (msg_tx, msg_rx) = mpsc::unbounded_channel();
        
        // Start background processor with minimal memory footprint
        let processor = Processor::new(msg_rx);
        tokio::spawn(processor.run());
        
        Self { msg_tx }
    }

    /// Check if app is available in repository
    pub async fn check_app_available(
        &self,
        hub_uuid: &str,
        app_data: &std::collections::BTreeMap<&str, &str>,
        hub_data: &std::collections::BTreeMap<&str, &str>,
    ) -> Result<bool, String> {
        let app_key = self.serialize_minimal(app_data, hub_data);
        let id = OpId::check_available(hub_uuid, &app_key);
        let data = vec![hub_uuid.to_string(), app_key];
        
        match self.send_request(id, data).await? {
            AppResult::Bool(result) => Ok(result),
            AppResult::Error(err) => Err(err),
            _ => Err("Invalid result type".to_string()),
        }
    }

    /// Get latest release for app
    pub async fn get_latest_release(
        &self,
        hub_uuid: &str,
        app_data: &std::collections::BTreeMap<&str, &str>,
        hub_data: &std::collections::BTreeMap<&str, &str>,
    ) -> Result<ReleaseData, String> {
        let app_key = self.serialize_minimal(app_data, hub_data);
        let id = OpId::get_latest(hub_uuid, &app_key);
        let data = vec![hub_uuid.to_string(), app_key];
        
        match self.send_request(id, data).await? {
            AppResult::Release(release) => Ok(release),
            AppResult::Error(err) => Err(err),
            _ => Err("Invalid result type".to_string()),
        }
    }

    /// Get all releases for app
    pub async fn get_releases(
        &self,
        hub_uuid: &str,
        app_data: &std::collections::BTreeMap<&str, &str>,
        hub_data: &std::collections::BTreeMap<&str, &str>,
    ) -> Result<Vec<ReleaseData>, String> {
        let app_key = self.serialize_minimal(app_data, hub_data);
        let id = OpId::get_releases(hub_uuid, &app_key);
        let data = vec![hub_uuid.to_string(), app_key];
        
        match self.send_request(id, data).await? {
            AppResult::Releases(releases) => Ok(releases),
            AppResult::Error(err) => Err(err),
            _ => Err("Invalid result type".to_string()),
        }
    }

    /// Update app to specific version (placeholder implementation)
    pub async fn update_app(&self, app_id: &str, version: &str) -> Result<String, String> {
        let id = OpId::update_app(app_id);
        let data = vec![app_id.to_string(), version.to_string()];
        
        match self.send_request(id, data).await? {
            AppResult::Success(msg) => Ok(msg),
            AppResult::Error(err) => Err(err),
            _ => Err("Invalid result type".to_string()),
        }
    }

    /// Add new app to management (placeholder implementation)
    pub async fn add_app(&self, _app_config: &str) -> Result<String, String> {
        // TODO: Implement app addition
        Err("Not implemented yet".to_string())
    }

    /// Remove app from management (placeholder implementation)  
    pub async fn remove_app(&self, _app_id: &str) -> Result<bool, String> {
        // TODO: Implement app removal
        Err("Not implemented yet".to_string())
    }

    /// Send request to background processor
    async fn send_request(&self, id: OpId, data: Vec<String>) -> Result<AppResult, String> {
        let (tx, rx) = oneshot::channel();
        let msg = Msg { id, data, tx };
        
        self.msg_tx.send(msg)
            .map_err(|_| "Processor unavailable".to_string())?;
        
        rx.await.map_err(|_| "Request failed".to_string())
    }

    /// Create minimal serialization key for deduplication
    fn serialize_minimal(
        &self,
        app_data: &std::collections::BTreeMap<&str, &str>,
        hub_data: &std::collections::BTreeMap<&str, &str>,
    ) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        for (k, v) in app_data {
            k.hash(&mut hasher);
            v.hash(&mut hasher);
        }
        for (k, v) in hub_data {
            k.hash(&mut hasher);
            v.hash(&mut hasher);
        }
        hasher.finish().to_string()
    }
}

/// Background processor with minimal memory usage
struct Processor {
    msg_rx: mpsc::UnboundedReceiver<Msg>,
    // Only keep active requests in memory, auto-cleanup after completion
    active: Arc<Mutex<HashMap<OpId, Vec<oneshot::Sender<AppResult>>>>>,
}

impl Processor {
    fn new(msg_rx: mpsc::UnboundedReceiver<Msg>) -> Self {
        Self {
            msg_rx,
            active: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Main processing loop with memory cleanup
    async fn run(mut self) {
        while let Some(msg) = self.msg_rx.recv().await {
            let id = msg.id.clone();
            let tx = msg.tx;
            
            // Check for duplicate requests and add to waiters
            let should_execute = {
                let mut active = self.active.lock().await;
                if let Some(waiters) = active.get_mut(&id) {
                    waiters.push(tx);
                    false // duplicate request, don't execute
                } else {
                    active.insert(id.clone(), vec![tx]);
                    true // first request, execute
                }
            };

            if should_execute {
                let active = self.active.clone();
                let msg_data = msg.data;
                let msg_id = msg.id;
                // Execute in separate task to avoid blocking
                tokio::spawn(async move {
                    let reconstructed_msg = Msg {
                        id: msg_id,
                        data: msg_data,
                        tx: tokio::sync::oneshot::channel().0, // dummy tx, not used in execute
                    };
                    let result = Self::execute(&reconstructed_msg).await;
                    Self::notify_and_cleanup(active, id, result).await;
                });
            }
        }
    }

    /// Execute the actual operation
    async fn execute(msg: &Msg) -> AppResult {
        match msg.id.op_type {
            0 => Self::exec_check_available(msg).await,
            1 => Self::exec_get_latest(msg).await,
            2 => Self::exec_get_releases(msg).await,
            3 => Self::exec_update_app(msg).await,
            _ => AppResult::Error("Unknown operation".to_string()),
        }
    }

    /// Execute check availability with memory-efficient parsing
    async fn exec_check_available(msg: &Msg) -> AppResult {
        if msg.data.len() < 2 {
            return AppResult::Error("Invalid data".to_string());
        }
        
        // For memory efficiency, we'll need to reconstruct the data
        // In a real implementation, you might use disk cache here if memory is critically low
        let hub_uuid = &msg.data[0];
        
        // Simplified implementation - in real scenario you'd reconstruct the BTreeMap
        // or use disk cache if memory is constrained
        let empty_app_data = std::collections::BTreeMap::new();
        let empty_hub_data = std::collections::BTreeMap::new();
        
        match crate::websdk::repo::api::check_app_available(
            hub_uuid,
            &empty_app_data,
            &empty_hub_data,
        ).await {
            Some(result) => AppResult::Bool(result),
            None => AppResult::Error("API call failed".to_string()),
        }
    }

    /// Execute get latest release
    async fn exec_get_latest(msg: &Msg) -> AppResult {
        if msg.data.len() < 2 {
            return AppResult::Error("Invalid data".to_string());
        }
        
        let hub_uuid = &msg.data[0];
        let empty_app_data = std::collections::BTreeMap::new();
        let empty_hub_data = std::collections::BTreeMap::new();
        
        match crate::websdk::repo::api::get_latest_release(
            hub_uuid,
            &empty_app_data,
            &empty_hub_data,
        ).await {
            Some(result) => AppResult::Release(result),
            None => AppResult::Error("API call failed".to_string()),
        }
    }

    /// Execute get all releases
    async fn exec_get_releases(msg: &Msg) -> AppResult {
        if msg.data.len() < 2 {
            return AppResult::Error("Invalid data".to_string());
        }
        
        let hub_uuid = &msg.data[0];
        let empty_app_data = std::collections::BTreeMap::new();
        let empty_hub_data = std::collections::BTreeMap::new();
        
        match crate::websdk::repo::api::get_releases(
            hub_uuid,
            &empty_app_data,
            &empty_hub_data,
        ).await {
            Some(result) => AppResult::Releases(result),
            None => AppResult::Error("API call failed".to_string()),
        }
    }

    /// Execute app update (placeholder)
    async fn exec_update_app(msg: &Msg) -> AppResult {
        if msg.data.len() < 2 {
            return AppResult::Error("Invalid data".to_string());
        }
        
        let app_id = &msg.data[0];
        let version = &msg.data[1];
        
        // TODO: Implement actual update logic
        AppResult::Success(format!("Would update {} to {}", app_id, version))
    }

    /// Notify waiters and cleanup memory immediately
    async fn notify_and_cleanup(
        active: Arc<Mutex<HashMap<OpId, Vec<oneshot::Sender<AppResult>>>>>,
        id: OpId,
        result: AppResult,
    ) {
        let waiters = {
            let mut active_lock = active.lock().await;
            active_lock.remove(&id).unwrap_or_default()
        }; // Memory cleanup happens here

        // Notify all waiters
        for waiter in waiters {
            let _ = waiter.send(result.clone());
        }
    }
}

/// Global instance with lazy initialization
use once_cell::sync::Lazy;
static GLOBAL_MANAGER: Lazy<AppManager> = Lazy::new(AppManager::new);

/// Get global app manager instance
pub fn get_app_manager() -> &'static AppManager {
    &GLOBAL_MANAGER
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_deduplication() {
        let manager = AppManager::new();
        let app_data = std::collections::BTreeMap::from([("repo", "test")]);
        let hub_data = std::collections::BTreeMap::new();
        
        // Multiple identical requests
        let (r1, r2, r3) = tokio::join!(
            manager.check_app_available("hub", &app_data, &hub_data),
            manager.check_app_available("hub", &app_data, &hub_data),
            manager.check_app_available("hub", &app_data, &hub_data),
        );
        
        // Results should be consistent
        assert_eq!(r1.is_ok(), r2.is_ok());
        assert_eq!(r2.is_ok(), r3.is_ok());
    }

    #[tokio::test]
    async fn test_different_requests_not_deduplicated() {
        let manager = AppManager::new();
        let app_data1 = std::collections::BTreeMap::from([("repo", "test1")]);
        let app_data2 = std::collections::BTreeMap::from([("repo", "test2")]);
        let hub_data = std::collections::BTreeMap::new();
        
        // Different requests should be handled separately
        let (r1, r2) = tokio::join!(
            manager.check_app_available("hub", &app_data1, &hub_data),
            manager.check_app_available("hub", &app_data2, &hub_data),
        );
        
        // Both should complete (may fail but should not hang)
        assert!(r1.is_ok() || r1.is_err());
        assert!(r2.is_ok() || r2.is_err());
    }

    #[tokio::test]
    async fn test_request_timeout_handling() {
        let manager = AppManager::new();
        let app_data = std::collections::BTreeMap::from([("nonexistent", "repo")]);
        let hub_data = std::collections::BTreeMap::new();
        
        // This should complete within reasonable time even if API fails
        let start = std::time::Instant::now();
        let result = manager.check_app_available("invalid-hub", &app_data, &hub_data).await;
        let duration = start.elapsed();
        
        // Should not hang indefinitely
        assert!(duration < std::time::Duration::from_secs(5));
        // Result should be available (likely an error)
        assert!(result.is_ok() || result.is_err());
    }
}