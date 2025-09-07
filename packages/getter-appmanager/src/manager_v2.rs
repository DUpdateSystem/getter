use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::status_tracker::{AppStatusInfo, StatusTracker};
use getter_config::{get_layered_config, AppConfig, AppIdentifier, HubConfig};
use getter_provider::{ProviderManager, ReleaseData};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OpId {
    Check(String),
    Latest(String),
    Releases(String),
    Update(String),
    Add(String),
    Remove(String),
    List,
    GetStatus(String),
    GetAllStatuses,
    GetOutdated,
}

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

struct Msg {
    id: OpId,
    data: Vec<String>,
    tx: oneshot::Sender<AppResult>,
}

/// Simplified AppManager as pure API layer
pub struct AppManager {
    msg_tx: mpsc::UnboundedSender<Msg>,
}

impl AppManager {
    pub fn new() -> Self {
        Self::with_provider_manager(ProviderManager::with_auto_registered())
    }

    pub fn with_provider_manager(provider_manager: ProviderManager) -> Self {
        let (msg_tx, msg_rx) = mpsc::unbounded_channel();
        let processor = Processor::new(msg_rx, provider_manager);
        tokio::spawn(processor.run());
        Self { msg_tx }
    }

    pub async fn check_app_available(&self, identifier: &str) -> Result<bool, String> {
        self.send_request(OpId::Check(identifier.to_string()), vec![identifier.to_string()])
            .await
            .and_then(|result| match result {
                AppResult::Bool(v) => Ok(v),
                AppResult::Error(e) => Err(e),
                _ => Err("Invalid result type".to_string()),
            })
    }

    pub async fn get_latest_release(&self, identifier: &str) -> Result<ReleaseData, String> {
        self.send_request(OpId::Latest(identifier.to_string()), vec![identifier.to_string()])
            .await
            .and_then(|result| match result {
                AppResult::Release(v) => Ok(v),
                AppResult::Error(e) => Err(e),
                _ => Err("Invalid result type".to_string()),
            })
    }

    pub async fn get_releases(&self, identifier: &str) -> Result<Vec<ReleaseData>, String> {
        self.send_request(OpId::Releases(identifier.to_string()), vec![identifier.to_string()])
            .await
            .and_then(|result| match result {
                AppResult::Releases(v) => Ok(v),
                AppResult::Error(e) => Err(e),
                _ => Err("Invalid result type".to_string()),
            })
    }

    pub async fn update_app(&self, identifier: &str, version: &str) -> Result<String, String> {
        self.send_request(
            OpId::Update(identifier.to_string()),
            vec![identifier.to_string(), version.to_string()],
        )
        .await
        .and_then(|result| match result {
            AppResult::Success(msg) => Ok(msg),
            AppResult::Error(e) => Err(e),
            _ => Err("Invalid result type".to_string()),
        })
    }

    pub async fn add_app(
        &self,
        identifier: &str,
        app_metadata: Option<HashMap<String, Value>>,
        hub_config: Option<HashMap<String, Value>>,
    ) -> Result<String, String> {
        let app_json = serde_json::to_string(&app_metadata).unwrap_or_default();
        let hub_json = serde_json::to_string(&hub_config).unwrap_or_default();
        
        self.send_request(
            OpId::Add(identifier.to_string()),
            vec![identifier.to_string(), app_json, hub_json],
        )
        .await
        .and_then(|result| match result {
            AppResult::Success(msg) => Ok(msg),
            AppResult::Error(e) => Err(e),
            _ => Err("Invalid result type".to_string()),
        })
    }

    pub async fn remove_app(&self, identifier: &str) -> Result<bool, String> {
        self.send_request(OpId::Remove(identifier.to_string()), vec![identifier.to_string()])
            .await
            .and_then(|result| match result {
                AppResult::Bool(v) => Ok(v),
                AppResult::Error(e) => Err(e),
                _ => Err("Invalid result type".to_string()),
            })
    }

    pub async fn list_apps(&self) -> Result<Vec<String>, String> {
        self.send_request(OpId::List, vec![])
            .await
            .and_then(|result| match result {
                AppResult::List(apps) => Ok(apps),
                AppResult::Error(e) => Err(e),
                _ => Err("Invalid result type".to_string()),
            })
    }

    pub async fn get_app_status(&self, identifier: &str) -> Result<Option<AppStatusInfo>, String> {
        self.send_request(OpId::GetStatus(identifier.to_string()), vec![identifier.to_string()])
            .await
            .and_then(|result| match result {
                AppResult::Status(status) => Ok(Some(status)),
                AppResult::Error(_) => Ok(None),
                _ => Err("Invalid result type".to_string()),
            })
    }

    pub async fn get_all_app_statuses(&self) -> Result<Vec<AppStatusInfo>, String> {
        self.send_request(OpId::GetAllStatuses, vec![])
            .await
            .and_then(|result| match result {
                AppResult::StatusList(statuses) => Ok(statuses),
                AppResult::Error(e) => Err(e),
                _ => Err("Invalid result type".to_string()),
            })
    }

    pub async fn get_outdated_apps(&self) -> Result<Vec<AppStatusInfo>, String> {
        self.send_request(OpId::GetOutdated, vec![])
            .await
            .and_then(|result| match result {
                AppResult::StatusList(statuses) => Ok(statuses),
                AppResult::Error(e) => Err(e),
                _ => Err("Invalid result type".to_string()),
            })
    }

    async fn send_request(&self, id: OpId, data: Vec<String>) -> Result<AppResult, String> {
        let (tx, rx) = oneshot::channel();
        let msg = Msg { id, data, tx };

        self.msg_tx
            .send(msg)
            .map_err(|_| "Processor unavailable".to_string())?;

        rx.await.map_err(|_| "Request failed".to_string())
    }
}

struct Processor {
    msg_rx: mpsc::UnboundedReceiver<Msg>,
    active: Arc<Mutex<HashMap<OpId, Vec<oneshot::Sender<AppResult>>>>>,
    status_tracker: StatusTracker,
    provider_manager: ProviderManager,
}

impl Processor {
    fn new(msg_rx: mpsc::UnboundedReceiver<Msg>, provider_manager: ProviderManager) -> Self {
        Self {
            msg_rx,
            active: Arc::new(Mutex::new(HashMap::new())),
            status_tracker: StatusTracker::new(),
            provider_manager,
        }
    }

    async fn run(mut self) {
        while let Some(msg) = self.msg_rx.recv().await {
            let id = msg.id.clone();
            let tx = msg.tx;

            let should_execute = {
                let mut active = self.active.lock().await;
                if let Some(waiters) = active.get_mut(&id) {
                    waiters.push(tx);
                    false
                } else {
                    active.insert(id.clone(), vec![tx]);
                    true
                }
            };

            if should_execute {
                let active = self.active.clone();
                let status_tracker = self.status_tracker.clone();
                let provider_manager = &self.provider_manager;
                let msg_data = msg.data;
                let msg_id = msg.id;

                let result = Self::execute(&msg_id, &msg_data, &status_tracker, provider_manager).await;
                Self::notify_and_cleanup(active, id, result).await;
            }
        }
    }

    async fn execute(
        op_id: &OpId,
        data: &[String],
        status_tracker: &StatusTracker,
        provider_manager: &ProviderManager,
    ) -> AppResult {
        match op_id {
            OpId::Check(_) => Self::exec_check_available(data, provider_manager).await,
            OpId::Latest(_) => Self::exec_get_latest(data, provider_manager).await,
            OpId::Releases(_) => Self::exec_get_releases(data, provider_manager).await,
            OpId::Update(_) => Self::exec_update_app(data, status_tracker).await,
            OpId::Add(_) => Self::exec_add_app(data, status_tracker).await,
            OpId::Remove(_) => Self::exec_remove_app(data, status_tracker).await,
            OpId::List => Self::exec_list_apps().await,
            OpId::GetStatus(_) => Self::exec_get_status(data, status_tracker).await,
            OpId::GetAllStatuses => Self::exec_get_all_statuses(status_tracker).await,
            OpId::GetOutdated => Self::exec_get_outdated_apps().await,
        }
    }

    async fn exec_check_available(data: &[String], provider_manager: &ProviderManager) -> AppResult {
        if data.is_empty() {
            return AppResult::Error("Invalid data".to_string());
        }

        let identifier = &data[0];
        let config = get_layered_config().await;
        let mut config = config.lock().await;
        
        match config.get_app_details(identifier) {
            Ok((app_config, hub_config)) => {
                // Convert configs to FIn format for provider
                let app_data = convert_to_btree(&app_config.metadata);
                let hub_data = convert_to_btree(&hub_config.config);
                let fin = getter_provider::FIn::new_with_frag(&app_data, &hub_data, None);
                
                // Try provider based on hub type
                match provider_manager.check_app_available(&hub_config.provider_type, &fin).await {
                    Ok(result) => AppResult::Bool(result),
                    Err(e) => AppResult::Error(format!("Provider error: {}", e)),
                }
            }
            Err(e) => AppResult::Error(format!("Config error: {}", e)),
        }
    }

    async fn exec_get_latest(data: &[String], provider_manager: &ProviderManager) -> AppResult {
        if data.is_empty() {
            return AppResult::Error("Invalid data".to_string());
        }

        let identifier = &data[0];
        let config = get_layered_config().await;
        let mut config = config.lock().await;
        
        match config.get_app_details(identifier) {
            Ok((app_config, hub_config)) => {
                let app_data = convert_to_btree(&app_config.metadata);
                let hub_data = convert_to_btree(&hub_config.config);
                let fin = getter_provider::FIn::new_with_frag(&app_data, &hub_data, None);
                
                match provider_manager.get_latest_release(&hub_config.provider_type, &fin).await {
                    Ok(release) => AppResult::Release(release),
                    Err(e) => AppResult::Error(format!("Provider error: {}", e)),
                }
            }
            Err(e) => AppResult::Error(format!("Config error: {}", e)),
        }
    }

    async fn exec_get_releases(data: &[String], provider_manager: &ProviderManager) -> AppResult {
        if data.is_empty() {
            return AppResult::Error("Invalid data".to_string());
        }

        let identifier = &data[0];
        let config = get_layered_config().await;
        let mut config = config.lock().await;
        
        match config.get_app_details(identifier) {
            Ok((app_config, hub_config)) => {
                let app_data = convert_to_btree(&app_config.metadata);
                let hub_data = convert_to_btree(&hub_config.config);
                let fin = getter_provider::FIn::new_with_frag(&app_data, &hub_data, None);
                
                match provider_manager.get_releases(&hub_config.provider_type, &fin).await {
                    Ok(releases) => AppResult::Releases(releases),
                    Err(_) => AppResult::Releases(vec![]),
                }
            }
            Err(e) => AppResult::Error(format!("Config error: {}", e)),
        }
    }

    async fn exec_update_app(data: &[String], status_tracker: &StatusTracker) -> AppResult {
        if data.len() < 2 {
            return AppResult::Error("Invalid data".to_string());
        }

        let identifier = &data[0];
        let version = &data[1];
        
        let config = get_layered_config().await;
        let mut config = config.lock().await;
        
        match config.update_version(identifier, Some(version.to_string()), None) {
            Ok(()) => {
                status_tracker.set_versions(identifier, Some(version.to_string()), None).await;
                AppResult::Success(format!("Updated {} to {}", identifier, version))
            }
            Err(e) => AppResult::Error(format!("Update failed: {}", e)),
        }
    }

    async fn exec_add_app(data: &[String], status_tracker: &StatusTracker) -> AppResult {
        if data.is_empty() {
            return AppResult::Error("Invalid data".to_string());
        }

        let identifier = &data[0];
        let app_metadata: Option<HashMap<String, Value>> = if data.len() > 1 && !data[1].is_empty() {
            serde_json::from_str(&data[1]).ok()
        } else {
            None
        };
        
        let hub_config_data: Option<HashMap<String, Value>> = if data.len() > 2 && !data[2].is_empty() {
            serde_json::from_str(&data[2]).ok()
        } else {
            None
        };

        let config = get_layered_config().await;
        let mut config = config.lock().await;
        
        // Parse identifier
        let app_id = match AppIdentifier::parse(identifier) {
            Ok(id) => id,
            Err(e) => return AppResult::Error(format!("Invalid identifier: {}", e)),
        };
        
        // Create configs if provided
        let app_config = app_metadata.map(|metadata| AppConfig {
            name: app_id.app_id.clone(),
            metadata,
        });
        
        let hub_config = hub_config_data.map(|config_data| HubConfig {
            name: app_id.hub_id.clone(),
            provider_type: app_id.hub_id.clone(), // Default to hub_id as provider type
            config: config_data,
        });
        
        match config.add_tracked_app(identifier, app_config, hub_config) {
            Ok(()) => {
                status_tracker.add_app(identifier.to_string()).await;
                AppResult::Success(format!("App '{}' added successfully", identifier))
            }
            Err(e) => AppResult::Error(format!("Failed to add app: {}", e)),
        }
    }

    async fn exec_remove_app(data: &[String], status_tracker: &StatusTracker) -> AppResult {
        if data.is_empty() {
            return AppResult::Error("Invalid data".to_string());
        }

        let identifier = &data[0];
        let config = get_layered_config().await;
        let mut config = config.lock().await;
        
        match config.remove_tracked_app(identifier) {
            Ok(removed) => {
                if removed {
                    status_tracker.remove_app(identifier).await;
                }
                AppResult::Bool(removed)
            }
            Err(e) => AppResult::Error(format!("Failed to remove app: {}", e)),
        }
    }

    async fn exec_list_apps() -> AppResult {
        let config = get_layered_config().await;
        let config = config.lock().await;
        AppResult::List(config.list_tracked_apps())
    }

    async fn exec_get_status(data: &[String], status_tracker: &StatusTracker) -> AppResult {
        if data.is_empty() {
            return AppResult::Error("App ID required".to_string());
        }

        let identifier = &data[0];
        match status_tracker.get_status(identifier).await {
            Some(status) => AppResult::Status(status),
            None => AppResult::Error(format!("App '{}' not found", identifier)),
        }
    }

    async fn exec_get_all_statuses(status_tracker: &StatusTracker) -> AppResult {
        let statuses = status_tracker.get_all_statuses().await;
        AppResult::StatusList(statuses)
    }

    async fn exec_get_outdated_apps() -> AppResult {
        let config = get_layered_config().await;
        let config = config.lock().await;
        
        let outdated = config.get_outdated_apps();
        let statuses: Vec<AppStatusInfo> = outdated
            .into_iter()
            .map(|(id, info)| AppStatusInfo {
                app_id: id,
                status: crate::app_status::AppStatus::AppOutdated,
                current_version: info.current_version,
                latest_version: info.latest_version,
                last_checked: info.last_checked,
            })
            .collect();
        
        AppResult::StatusList(statuses)
    }

    async fn notify_and_cleanup(
        active: Arc<Mutex<HashMap<OpId, Vec<oneshot::Sender<AppResult>>>>>,
        id: OpId,
        result: AppResult,
    ) {
        let waiters = {
            let mut active_lock = active.lock().await;
            active_lock.remove(&id).unwrap_or_default()
        };

        for waiter in waiters {
            let _ = waiter.send(result.clone());
        }
    }
}

fn convert_to_btree(map: &HashMap<String, Value>) -> std::collections::BTreeMap<&str, &str> {
    let mut btree = std::collections::BTreeMap::new();
    for (key, value) in map {
        if let Value::String(s) = value {
            btree.insert(key.as_str(), s.as_str());
        }
    }
    btree
}

impl Default for AppManager {
    fn default() -> Self {
        Self::new()
    }
}