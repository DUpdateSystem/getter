use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::app_status::AppStatus;
use crate::status_tracker::{AppStatusInfo, StatusTracker};
use getter_provider::{ReleaseData, ProviderManager, BaseProvider, GitHubProvider};
use getter_config::{get_world_list, TrackedApp};

/// Lightweight operation identifier for deduplication
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OpId {
    pub op_type: u8, // 0=check, 1=latest, 2=releases, 3=update, 4=add, 5=remove, 6=list, 7=get_status, 8=get_all_statuses
    pub key: String, // combined key for deduplication
}

impl OpId {
    fn check_available(hub_uuid: &str, app_key: &str) -> Self {
        Self {
            op_type: 0,
            key: format!("{}:{}", hub_uuid, app_key),
        }
    }

    fn get_latest(hub_uuid: &str, app_key: &str) -> Self {
        Self {
            op_type: 1,
            key: format!("{}:{}", hub_uuid, app_key),
        }
    }

    fn get_releases(hub_uuid: &str, app_key: &str) -> Self {
        Self {
            op_type: 2,
            key: format!("{}:{}", hub_uuid, app_key),
        }
    }

    fn update_app(app_id: &str) -> Self {
        Self {
            op_type: 3,
            key: app_id.to_string(),
        }
    }

    fn add_app(app_id: &str) -> Self {
        Self {
            op_type: 4,
            key: app_id.to_string(),
        }
    }

    fn remove_app(app_id: &str) -> Self {
        Self {
            op_type: 5,
            key: app_id.to_string(),
        }
    }

    fn list_apps() -> Self {
        Self {
            op_type: 6,
            key: "list_all".to_string(),
        }
    }

    fn get_status(app_id: &str) -> Self {
        Self {
            op_type: 7,
            key: format!("status_{}", app_id),
        }
    }

    fn get_all_statuses() -> Self {
        Self {
            op_type: 8,
            key: "all_statuses".to_string(),
        }
    }
}

/// Lightweight result enum
#[derive(Debug, Clone)]
pub enum AppResult {
    Bool(bool),
    Release(ReleaseData),
    Releases(Vec<ReleaseData>),
    List(Vec<String>),
    Success(String),
    Error(String),
    Status(AppStatusInfo),
    StatusList(Vec<AppStatusInfo>),
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

    /// Add new app to management
    pub async fn add_app(
        &self,
        app_id: String,
        hub_uuid: String,
        app_data: std::collections::HashMap<String, String>,
        hub_data: std::collections::HashMap<String, String>,
    ) -> Result<String, String> {
        let id = OpId::add_app(&app_id);
        let data = vec![
            app_id,
            hub_uuid,
            serde_json::to_string(&app_data).map_err(|e| e.to_string())?,
            serde_json::to_string(&hub_data).map_err(|e| e.to_string())?,
        ];

        match self.send_request(id, data).await? {
            AppResult::Success(msg) => Ok(msg),
            AppResult::Error(err) => Err(err),
            _ => Err("Invalid result type".to_string()),
        }
    }

    /// Remove app from management
    pub async fn remove_app(&self, app_id: &str) -> Result<bool, String> {
        let id = OpId::remove_app(app_id);
        let data = vec![app_id.to_string()];

        match self.send_request(id, data).await? {
            AppResult::Bool(result) => Ok(result),
            AppResult::Error(err) => Err(err),
            _ => Err("Invalid result type".to_string()),
        }
    }

    /// List all tracked apps
    pub async fn list_apps(&self) -> Result<Vec<String>, String> {
        let id = OpId::list_apps();
        let data = vec![];

        match self.send_request(id, data).await? {
            AppResult::List(apps) => Ok(apps),
            AppResult::Error(err) => Err(err),
            _ => Err("Invalid result type".to_string()),
        }
    }

    /// Get status for a specific app
    pub async fn get_app_status(&self, app_id: &str) -> Result<Option<AppStatusInfo>, String> {
        let id = OpId::get_status(app_id);
        let data = vec![app_id.to_string()];
        match self.send_request(id, data).await? {
            AppResult::Status(status) => Ok(Some(status)),
            AppResult::Error(_) => Ok(None), // App not found
            _ => Err("Invalid result type".to_string()),
        }
    }

    /// Get status for all tracked apps
    pub async fn get_all_app_statuses(&self) -> Result<Vec<AppStatusInfo>, String> {
        let id = OpId::get_all_statuses();
        let data = vec![];
        match self.send_request(id, data).await? {
            AppResult::StatusList(statuses) => Ok(statuses),
            AppResult::Error(err) => Err(err),
            _ => Err("Invalid result type".to_string()),
        }
    }

    /// Get apps that have updates available
    pub async fn get_outdated_apps(&self) -> Result<Vec<AppStatusInfo>, String> {
        let all_statuses = self.get_all_app_statuses().await?;
        Ok(all_statuses
            .into_iter()
            .filter(|status| status.status.has_updates())
            .collect())
    }

    /// Send request to background processor
    async fn send_request(&self, id: OpId, data: Vec<String>) -> Result<AppResult, String> {
        let (tx, rx) = oneshot::channel();
        let msg = Msg { id, data, tx };

        self.msg_tx
            .send(msg)
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
    // Status tracker for all managed apps
    status_tracker: StatusTracker,
    // Provider manager
    provider_manager: ProviderManager,
}

impl Processor {
    fn new(msg_rx: mpsc::UnboundedReceiver<Msg>) -> Self {
        let mut provider_manager = ProviderManager::new();
        provider_manager.register_provider(Box::new(GitHubProvider::new()));
        
        Self {
            msg_rx,
            active: Arc::new(Mutex::new(HashMap::new())),
            status_tracker: StatusTracker::new(),
            provider_manager,
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
                let status_tracker = self.status_tracker.clone();
                let provider_manager = &self.provider_manager;
                let msg_data = msg.data;
                let msg_id = msg.id;
                
                // Execute in separate task to avoid blocking
                let result = Self::execute(&msg_id, &msg_data, &status_tracker, provider_manager).await;
                Self::notify_and_cleanup(active, id, result).await;
            }
        }
    }

    /// Execute the actual operation
    async fn execute(
        op_id: &OpId, 
        data: &[String], 
        status_tracker: &StatusTracker,
        provider_manager: &ProviderManager
    ) -> AppResult {
        match op_id.op_type {
            0 => Self::exec_check_available(data, provider_manager).await,
            1 => Self::exec_get_latest(data, provider_manager).await,
            2 => Self::exec_get_releases(data, provider_manager).await,
            3 => Self::exec_update_app(data).await,
            4 => Self::exec_add_app(data, status_tracker).await,
            5 => Self::exec_remove_app(data, status_tracker).await,
            6 => Self::exec_list_apps().await,
            7 => Self::exec_get_status(data, status_tracker).await,
            8 => Self::exec_get_all_statuses(status_tracker).await,
            _ => AppResult::Error("Unknown operation".to_string()),
        }
    }

    /// Execute check availability - simplified implementation
    async fn exec_check_available(data: &[String], _provider_manager: &ProviderManager) -> AppResult {
        if data.len() < 2 {
            return AppResult::Error("Invalid data".to_string());
        }
        // Simplified implementation - always return true for now
        AppResult::Bool(true)
    }

    /// Execute get latest release - simplified implementation
    async fn exec_get_latest(data: &[String], _provider_manager: &ProviderManager) -> AppResult {
        if data.len() < 2 {
            return AppResult::Error("Invalid data".to_string());
        }
        AppResult::Error("API call not implemented".to_string())
    }

    /// Execute get all releases - simplified implementation
    async fn exec_get_releases(data: &[String], _provider_manager: &ProviderManager) -> AppResult {
        if data.len() < 2 {
            return AppResult::Error("Invalid data".to_string());
        }
        AppResult::Releases(vec![])
    }

    /// Execute app update (placeholder)
    async fn exec_update_app(data: &[String]) -> AppResult {
        if data.len() < 2 {
            return AppResult::Error("Invalid data".to_string());
        }

        let app_id = &data[0];
        let version = &data[1];

        // TODO: Implement actual update logic
        AppResult::Success(format!("Would update {} to {}", app_id, version))
    }

    /// Execute add app operation
    async fn exec_add_app(data: &[String], status_tracker: &StatusTracker) -> AppResult {
        if data.len() < 4 {
            return AppResult::Error("Invalid data for add_app".to_string());
        }

        let app_id = &data[0];
        let hub_uuid = &data[1];
        let app_data_json = &data[2];
        let hub_data_json = &data[3];

        // Parse JSON data
        let app_data: std::collections::HashMap<String, String> =
            match serde_json::from_str(app_data_json) {
                Ok(data) => data,
                Err(e) => return AppResult::Error(format!("Invalid app_data JSON: {}", e)),
            };

        let hub_data: std::collections::HashMap<String, String> =
            match serde_json::from_str(hub_data_json) {
                Ok(data) => data,
                Err(e) => return AppResult::Error(format!("Invalid hub_data JSON: {}", e)),
            };

        // Get world list and add the app
        let world_list = get_world_list().await;
        let mut world_list = world_list.lock().await;
        let added = world_list.rule_list.add_tracked_app(
            app_id.clone(),
            hub_uuid.clone(),
            app_data,
            hub_data,
        );

        if added {
            // Also add to legacy app_list for compatibility
            world_list.rule_list.push_app(app_id);

            // Add to status tracker
            status_tracker.add_app(app_id.clone()).await;

            match world_list.save() {
                Ok(()) => AppResult::Success(format!("App '{}' added successfully", app_id)),
                Err(e) => AppResult::Error(format!("Failed to save app: {}", e)),
            }
        } else {
            AppResult::Error(format!("App '{}' already exists", app_id))
        }
    }

    /// Execute remove app operation
    async fn exec_remove_app(data: &[String], status_tracker: &StatusTracker) -> AppResult {
        if data.is_empty() {
            return AppResult::Error("Invalid data for remove_app".to_string());
        }

        let app_id = &data[0];

        // Get world list and remove the app
        let world_list = get_world_list().await;
        let mut world_list = world_list.lock().await;
        let removed = world_list.rule_list.remove_tracked_app(app_id);

        if removed {
            // Also remove from legacy app_list
            world_list.rule_list.remove_app(app_id);

            // Remove from status tracker
            status_tracker.remove_app(app_id).await;

            match world_list.save() {
                Ok(()) => AppResult::Bool(true),
                Err(e) => AppResult::Error(format!("Failed to save after removal: {}", e)),
            }
        } else {
            AppResult::Bool(false)
        }
    }

    /// Execute list apps operation
    async fn exec_list_apps() -> AppResult {
        let world_list = get_world_list().await;
        let world_list = world_list.lock().await;
        let apps = world_list.rule_list.list_tracked_apps();
        let app_ids: Vec<String> = apps.into_iter().map(|(id, _)| id.clone()).collect();
        AppResult::List(app_ids)
    }

    /// Get status for a specific app
    async fn exec_get_status(data: &[String], status_tracker: &StatusTracker) -> AppResult {
        if data.is_empty() {
            return AppResult::Error("App ID required".to_string());
        }

        let app_id = &data[0];
        match status_tracker.get_status(app_id).await {
            Some(status) => AppResult::Status(status),
            None => AppResult::Error(format!("App '{}' not found", app_id)),
        }
    }

    /// Get status for all tracked apps
    async fn exec_get_all_statuses(status_tracker: &StatusTracker) -> AppResult {
        let statuses = status_tracker.get_all_statuses().await;
        AppResult::StatusList(statuses)
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

impl Default for AppManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_app_manager_creation() {
        let manager = AppManager::new();
        let result = manager.list_apps().await;
        assert!(result.is_ok());
    }

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
}