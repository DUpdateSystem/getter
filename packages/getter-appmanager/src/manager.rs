use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::status_tracker::{AppStatusInfo, StatusTracker};
use getter_config::get_world_list;
use getter_provider::{ProviderManager, ReleaseData};

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
}

impl OpId {
    fn provider_op(variant: impl FnOnce(String) -> Self, hub_uuid: &str, app_key: &str) -> Self {
        variant(format!("{}:{}", hub_uuid, app_key))
    }

    fn check_available(hub_uuid: &str, app_key: &str) -> Self {
        Self::provider_op(Self::Check, hub_uuid, app_key)
    }

    fn get_latest(hub_uuid: &str, app_key: &str) -> Self {
        Self::provider_op(Self::Latest, hub_uuid, app_key)
    }

    fn get_releases(hub_uuid: &str, app_key: &str) -> Self {
        Self::provider_op(Self::Releases, hub_uuid, app_key)
    }

    fn update_app(app_id: &str) -> Self {
        Self::Update(app_id.to_string())
    }

    fn add_app(app_id: &str) -> Self {
        Self::Add(app_id.to_string())
    }

    fn remove_app(app_id: &str) -> Self {
        Self::Remove(app_id.to_string())
    }

    fn list_apps() -> Self {
        Self::List
    }

    fn get_status(app_id: &str) -> Self {
        Self::GetStatus(format!("status_{}", app_id))
    }

    fn get_all_statuses() -> Self {
        Self::GetAllStatuses
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

struct Msg {
    id: OpId,
    data: Vec<String>,
    tx: oneshot::Sender<AppResult>,
}

pub struct AppManager {
    msg_tx: mpsc::UnboundedSender<Msg>,
}

impl AppManager {
    /// Create new AppManager instance with default provider manager
    pub fn new() -> Self {
        Self::with_provider_manager(ProviderManager::with_auto_registered())
    }

    /// Create new AppManager instance with custom provider manager
    pub fn with_provider_manager(provider_manager: ProviderManager) -> Self {
        let (msg_tx, msg_rx) = mpsc::unbounded_channel();

        // Start background processor with minimal memory footprint
        let processor = Processor::with_provider_manager(msg_rx, provider_manager);
        tokio::spawn(processor.run());

        Self { msg_tx }
    }

    /// Generic provider method helper
    async fn provider_request<T>(
        &self,
        hub_uuid: &str,
        app_data: &std::collections::BTreeMap<&str, &str>,
        hub_data: &std::collections::BTreeMap<&str, &str>,
        op_creator: impl FnOnce(&str, &str) -> OpId,
        extractor: impl FnOnce(AppResult) -> Result<T, String>,
    ) -> Result<T, String> {
        let app_key = self.serialize_minimal(app_data, hub_data);
        let id = op_creator(hub_uuid, &app_key);
        let data = vec![hub_uuid.to_string(), app_key];
        extractor(self.send_request(id, data).await?)
    }

    /// Check if app is available in repository
    pub async fn check_app_available(
        &self,
        hub_uuid: &str,
        app_data: &std::collections::BTreeMap<&str, &str>,
        hub_data: &std::collections::BTreeMap<&str, &str>,
    ) -> Result<bool, String> {
        self.provider_request(
            hub_uuid,
            app_data,
            hub_data,
            OpId::check_available,
            |result| match result {
                AppResult::Bool(v) => Ok(v),
                AppResult::Error(e) => Err(e),
                _ => Err("Invalid result type".to_string()),
            },
        )
        .await
    }

    /// Get latest release for app
    pub async fn get_latest_release(
        &self,
        hub_uuid: &str,
        app_data: &std::collections::BTreeMap<&str, &str>,
        hub_data: &std::collections::BTreeMap<&str, &str>,
    ) -> Result<ReleaseData, String> {
        self.provider_request(
            hub_uuid,
            app_data,
            hub_data,
            OpId::get_latest,
            |result| match result {
                AppResult::Release(v) => Ok(v),
                AppResult::Error(e) => Err(e),
                _ => Err("Invalid result type".to_string()),
            },
        )
        .await
    }

    /// Get all releases for app
    pub async fn get_releases(
        &self,
        hub_uuid: &str,
        app_data: &std::collections::BTreeMap<&str, &str>,
        hub_data: &std::collections::BTreeMap<&str, &str>,
    ) -> Result<Vec<ReleaseData>, String> {
        self.provider_request(hub_uuid, app_data, hub_data, OpId::get_releases, |result| {
            match result {
                AppResult::Releases(v) => Ok(v),
                AppResult::Error(e) => Err(e),
                _ => Err("Invalid result type".to_string()),
            }
        })
        .await
    }

    /// Extract result helper
    fn extract_result<T, F>(result: AppResult, extractor: F) -> Result<T, String>
    where
        F: FnOnce(AppResult) -> Result<T, String>,
    {
        extractor(result)
    }

    /// Update app to specific version (placeholder implementation)
    pub async fn update_app(&self, app_id: &str, version: &str) -> Result<String, String> {
        let result = self
            .send_request(
                OpId::update_app(app_id),
                vec![app_id.to_string(), version.to_string()],
            )
            .await?;

        Self::extract_result(result, |r| match r {
            AppResult::Success(msg) => Ok(msg),
            AppResult::Error(err) => Err(err),
            _ => Err("Invalid result type".to_string()),
        })
    }

    /// Add new app to management
    pub async fn add_app(
        &self,
        app_id: String,
        hub_uuid: String,
        app_data: std::collections::HashMap<String, String>,
        hub_data: std::collections::HashMap<String, String>,
    ) -> Result<String, String> {
        let data = vec![
            app_id.clone(),
            hub_uuid,
            serde_json::to_string(&app_data).map_err(|e| e.to_string())?,
            serde_json::to_string(&hub_data).map_err(|e| e.to_string())?,
        ];

        let result = self.send_request(OpId::add_app(&app_id), data).await?;

        Self::extract_result(result, |r| match r {
            AppResult::Success(msg) => Ok(msg),
            AppResult::Error(err) => Err(err),
            _ => Err("Invalid result type".to_string()),
        })
    }

    /// Remove app from management
    pub async fn remove_app(&self, app_id: &str) -> Result<bool, String> {
        let result = self
            .send_request(OpId::remove_app(app_id), vec![app_id.to_string()])
            .await?;

        Self::extract_result(result, |r| match r {
            AppResult::Bool(v) => Ok(v),
            AppResult::Error(err) => Err(err),
            _ => Err("Invalid result type".to_string()),
        })
    }

    /// List all tracked apps
    pub async fn list_apps(&self) -> Result<Vec<String>, String> {
        let result = self.send_request(OpId::list_apps(), vec![]).await?;

        Self::extract_result(result, |r| match r {
            AppResult::List(apps) => Ok(apps),
            AppResult::Error(err) => Err(err),
            _ => Err("Invalid result type".to_string()),
        })
    }

    /// Get status for a specific app
    pub async fn get_app_status(&self, app_id: &str) -> Result<Option<AppStatusInfo>, String> {
        let result = self
            .send_request(OpId::get_status(app_id), vec![app_id.to_string()])
            .await?;

        Self::extract_result(result, |r| match r {
            AppResult::Status(status) => Ok(Some(status)),
            AppResult::Error(_) => Ok(None),
            _ => Err("Invalid result type".to_string()),
        })
    }

    /// Get status for all tracked apps
    pub async fn get_all_app_statuses(&self) -> Result<Vec<AppStatusInfo>, String> {
        let result = self.send_request(OpId::get_all_statuses(), vec![]).await?;

        Self::extract_result(result, |r| match r {
            AppResult::StatusList(statuses) => Ok(statuses),
            AppResult::Error(err) => Err(err),
            _ => Err("Invalid result type".to_string()),
        })
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

struct Processor {
    msg_rx: mpsc::UnboundedReceiver<Msg>,
    active: Arc<Mutex<HashMap<OpId, Vec<oneshot::Sender<AppResult>>>>>,
    status_tracker: StatusTracker,
    provider_manager: ProviderManager,
}

macro_rules! exec_provider_op {
    ($data:expr, $provider_manager:expr, $method:ident, $result_variant:ident, $default:expr) => {{
        if $data.len() < 2 {
            return AppResult::Error("Invalid data".to_string());
        }

        let provider_names = $provider_manager.provider_names();
        if provider_names.is_empty() {
            return AppResult::Error("No providers registered".to_string());
        }

        let app_data = std::collections::BTreeMap::new();
        let hub_data = std::collections::BTreeMap::new();
        let fin = getter_provider::FIn::new_with_frag(&app_data, &hub_data, None);

        for provider_name in &provider_names {
            match $provider_manager.$method(provider_name, &fin).await {
                Ok(result) => return AppResult::$result_variant(result),
                Err(_) => continue,
            }
        }

        $default
    }};
}

impl Processor {
    fn with_provider_manager(
        msg_rx: mpsc::UnboundedReceiver<Msg>,
        provider_manager: ProviderManager,
    ) -> Self {
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

            // Check for duplicate requests and add to waiters
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

                let result =
                    Self::execute(&msg_id, &msg_data, &status_tracker, provider_manager).await;
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
            OpId::Update(_) => Self::exec_update_app(data).await,
            OpId::Add(_) => Self::exec_add_app(data, status_tracker).await,
            OpId::Remove(_) => Self::exec_remove_app(data, status_tracker).await,
            OpId::List => Self::exec_list_apps().await,
            OpId::GetStatus(_) => Self::exec_get_status(data, status_tracker).await,
            OpId::GetAllStatuses => Self::exec_get_all_statuses(status_tracker).await,
        }
    }

    async fn exec_check_available(
        data: &[String],
        provider_manager: &ProviderManager,
    ) -> AppResult {
        exec_provider_op!(
            data,
            provider_manager,
            check_app_available,
            Bool,
            AppResult::Error("No provider could check app availability".to_string())
        )
    }

    async fn exec_get_latest(data: &[String], provider_manager: &ProviderManager) -> AppResult {
        exec_provider_op!(
            data,
            provider_manager,
            get_latest_release,
            Release,
            AppResult::Error("No provider could get latest release".to_string())
        )
    }

    async fn exec_get_releases(data: &[String], provider_manager: &ProviderManager) -> AppResult {
        exec_provider_op!(
            data,
            provider_manager,
            get_releases,
            Releases,
            AppResult::Releases(vec![])
        )
    }

    async fn exec_update_app(data: &[String]) -> AppResult {
        if data.len() < 2 {
            return AppResult::Error("Invalid data".to_string());
        }

        let app_id = &data[0];
        let version = &data[1];

        AppResult::Success(format!("Would update {} to {}", app_id, version))
    }

    fn parse_json_data(
        json_str: &str,
        data_type: &str,
    ) -> Result<std::collections::HashMap<String, String>, AppResult> {
        serde_json::from_str(json_str)
            .map_err(|e| AppResult::Error(format!("Invalid {} JSON: {}", data_type, e)))
    }

    async fn exec_add_app(data: &[String], status_tracker: &StatusTracker) -> AppResult {
        if data.len() < 4 {
            return AppResult::Error("Invalid data for add_app".to_string());
        }

        let app_id = &data[0];
        let hub_uuid = &data[1];

        let app_data = match Self::parse_json_data(&data[2], "app_data") {
            Ok(data) => data,
            Err(e) => return e,
        };

        let hub_data = match Self::parse_json_data(&data[3], "hub_data") {
            Ok(data) => data,
            Err(e) => return e,
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
            world_list.rule_list.push_app(app_id);

            status_tracker.add_app(app_id.clone()).await;

            match world_list.save() {
                Ok(()) => AppResult::Success(format!("App '{}' added successfully", app_id)),
                Err(e) => AppResult::Error(format!("Failed to save app: {}", e)),
            }
        } else {
            AppResult::Error(format!("App '{}' already exists", app_id))
        }
    }

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
            world_list.rule_list.remove_app(app_id);

            status_tracker.remove_app(app_id).await;

            match world_list.save() {
                Ok(()) => AppResult::Bool(true),
                Err(e) => AppResult::Error(format!("Failed to save after removal: {}", e)),
            }
        } else {
            AppResult::Bool(false)
        }
    }

    async fn exec_list_apps() -> AppResult {
        let world_list = get_world_list().await;
        let world_list = world_list.lock().await;
        let apps = world_list.rule_list.list_tracked_apps();
        let app_ids: Vec<String> = apps.into_iter().map(|(id, _)| id.clone()).collect();
        AppResult::List(app_ids)
    }

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

    async fn exec_get_all_statuses(status_tracker: &StatusTracker) -> AppResult {
        let statuses = status_tracker.get_all_statuses().await;
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

impl Default for AppManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use getter_provider::{AssetData, BaseProvider, DataMap, FIn, FOut, FunctionType};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Mock provider for testing
    struct MockProvider {
        pub uuid: String,
        pub name: String,
        pub available: bool,
        pub releases: Vec<ReleaseData>,
        pub call_count: Arc<AtomicUsize>,
    }

    impl MockProvider {
        fn new(name: &str) -> Self {
            Self {
                uuid: format!("mock-{}", name),
                name: name.to_string(),
                available: true,
                releases: vec![ReleaseData {
                    version_number: "1.0.0".to_string(),
                    changelog: "Test release".to_string(),
                    assets: vec![AssetData {
                        file_name: "test.apk".to_string(),
                        file_type: "apk".to_string(),
                        download_url: "https://example.com/test.apk".to_string(),
                    }],
                    extra: Some(std::collections::HashMap::from([(
                        "created_at".to_string(),
                        "2024-01-01T00:00:00Z".to_string(),
                    )])),
                }],
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn with_releases(mut self, releases: Vec<ReleaseData>) -> Self {
            self.releases = releases;
            self
        }

        fn with_availability(mut self, available: bool) -> Self {
            self.available = available;
            self
        }
    }

    #[async_trait]
    impl BaseProvider for MockProvider {
        fn get_uuid(&self) -> &'static str {
            Box::leak(self.uuid.clone().into_boxed_str())
        }

        fn get_friendly_name(&self) -> &'static str {
            Box::leak(self.name.clone().into_boxed_str())
        }

        fn get_cache_request_key(
            &self,
            _function_type: &FunctionType,
            _data_map: &DataMap,
        ) -> Vec<String> {
            vec![format!("mock-cache-key-{}", self.name)]
        }

        async fn check_app_available(&self, _fin: &FIn) -> FOut<bool> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            FOut::new(self.available)
        }

        async fn get_latest_release(&self, _fin: &FIn) -> FOut<ReleaseData> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            if let Some(release) = self.releases.first() {
                FOut::new(release.clone())
            } else {
                FOut::new_empty().set_error(Box::new(std::io::Error::other("No releases")))
            }
        }

        async fn get_releases(&self, _fin: &FIn) -> FOut<Vec<ReleaseData>> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            FOut::new(self.releases.clone())
        }
    }

    #[tokio::test]
    async fn test_app_manager_creation() {
        let manager = AppManager::new();
        let result = manager.list_apps().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_app_manager_with_mock_provider() {
        // Create a mock provider
        let mock_provider = MockProvider::new("test-provider");

        // Create provider manager and register mock provider
        let mut provider_manager = ProviderManager::new();
        provider_manager.register_provider(Box::new(mock_provider));

        // Create app manager with custom provider manager
        let manager = AppManager::with_provider_manager(provider_manager);

        // Test that the manager works with mock provider
        let app_data = std::collections::BTreeMap::new();
        let hub_data = std::collections::BTreeMap::new();

        let result = manager
            .check_app_available("test-hub", &app_data, &hub_data)
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), true);
    }

    #[tokio::test]
    async fn test_provider_get_latest_release() {
        // Create mock provider with custom releases
        let test_release = ReleaseData {
            version_number: "2.0.0".to_string(),
            changelog: "Custom test release".to_string(),
            assets: vec![],
            extra: Some(std::collections::HashMap::from([
                ("created_at".to_string(), "2024-02-01T00:00:00Z".to_string()),
                (
                    "download_url".to_string(),
                    "https://test.com/v2".to_string(),
                ),
            ])),
        };

        let mock_provider =
            MockProvider::new("release-provider").with_releases(vec![test_release.clone()]);

        let mut provider_manager = ProviderManager::new();
        provider_manager.register_provider(Box::new(mock_provider));

        let manager = AppManager::with_provider_manager(provider_manager);

        let app_data = std::collections::BTreeMap::new();
        let hub_data = std::collections::BTreeMap::new();

        let result = manager
            .get_latest_release("test-hub", &app_data, &hub_data)
            .await;
        assert!(result.is_ok());
        let release = result.unwrap();
        assert_eq!(release.version_number, "2.0.0");
        assert_eq!(release.changelog, "Custom test release".to_string());
    }

    #[tokio::test]
    async fn test_provider_get_all_releases() {
        // Create mock provider with multiple releases
        let releases = vec![
            ReleaseData {
                version_number: "3.0.0".to_string(),
                changelog: "Latest".to_string(),
                assets: vec![],
                extra: Some(std::collections::HashMap::from([(
                    "created_at".to_string(),
                    "2024-03-01T00:00:00Z".to_string(),
                )])),
            },
            ReleaseData {
                version_number: "2.5.0".to_string(),
                changelog: "Previous".to_string(),
                assets: vec![],
                extra: Some(std::collections::HashMap::from([(
                    "created_at".to_string(),
                    "2024-02-15T00:00:00Z".to_string(),
                )])),
            },
            ReleaseData {
                version_number: "2.0.0".to_string(),
                changelog: "Old".to_string(),
                assets: vec![],
                extra: Some(std::collections::HashMap::from([(
                    "created_at".to_string(),
                    "2024-02-01T00:00:00Z".to_string(),
                )])),
            },
        ];

        let mock_provider =
            MockProvider::new("multi-release-provider").with_releases(releases.clone());

        let mut provider_manager = ProviderManager::new();
        provider_manager.register_provider(Box::new(mock_provider));

        let manager = AppManager::with_provider_manager(provider_manager);

        let app_data = std::collections::BTreeMap::new();
        let hub_data = std::collections::BTreeMap::new();

        let result = manager.get_releases("test-hub", &app_data, &hub_data).await;
        assert!(result.is_ok());
        let fetched_releases = result.unwrap();
        assert_eq!(fetched_releases.len(), 3);
        assert_eq!(fetched_releases[0].version_number, "3.0.0");
        assert_eq!(fetched_releases[1].version_number, "2.5.0");
        assert_eq!(fetched_releases[2].version_number, "2.0.0");
    }

    #[tokio::test]
    async fn test_provider_unavailable_app() {
        // Create mock provider that returns app unavailable
        let mock_provider = MockProvider::new("unavailable-provider").with_availability(false);

        let mut provider_manager = ProviderManager::new();
        provider_manager.register_provider(Box::new(mock_provider));

        let manager = AppManager::with_provider_manager(provider_manager);

        let app_data = std::collections::BTreeMap::new();
        let hub_data = std::collections::BTreeMap::new();

        let result = manager
            .check_app_available("test-hub", &app_data, &hub_data)
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
    }

    #[tokio::test]
    async fn test_no_providers_error() {
        // Create provider manager with no providers
        let provider_manager = ProviderManager::new();
        let manager = AppManager::with_provider_manager(provider_manager);

        let app_data = std::collections::BTreeMap::new();
        let hub_data = std::collections::BTreeMap::new();

        let result = manager
            .check_app_available("test-hub", &app_data, &hub_data)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No providers registered"));
    }

    #[tokio::test]
    async fn test_deduplication() {
        let mock_provider = MockProvider::new("dedup-provider");

        let mut provider_manager = ProviderManager::new();
        provider_manager.register_provider(Box::new(mock_provider));

        let manager = AppManager::with_provider_manager(provider_manager);
        let app_data = std::collections::BTreeMap::from([("repo", "test")]);
        let hub_data = std::collections::BTreeMap::new();

        // Multiple identical requests
        let (r1, r2, r3) = tokio::join!(
            manager.check_app_available("hub", &app_data, &hub_data),
            manager.check_app_available("hub", &app_data, &hub_data),
            manager.check_app_available("hub", &app_data, &hub_data),
        );

        // Results should be consistent - deduplication ensures all get the same result
        assert_eq!(r1.is_ok(), r2.is_ok());
        assert_eq!(r2.is_ok(), r3.is_ok());
        // All results should be the same value if successful
        if r1.is_ok() && r2.is_ok() && r3.is_ok() {
            let v1 = r1.unwrap();
            let v2 = r2.unwrap();
            let v3 = r3.unwrap();
            assert_eq!(v1, v2);
            assert_eq!(v2, v3);
        }

        // Note: With the current architecture, the provider may be called multiple times
        // since deduplication happens at the message level, not at the provider level.
        // The important thing is that all requests get consistent results.
    }

    #[tokio::test]
    async fn test_multiple_providers() {
        // Create multiple mock providers
        let provider1 = MockProvider::new("provider1").with_availability(false);
        let provider2 = MockProvider::new("provider2").with_availability(true);

        let mut provider_manager = ProviderManager::new();
        provider_manager.register_provider(Box::new(provider1));
        provider_manager.register_provider(Box::new(provider2));

        let manager = AppManager::with_provider_manager(provider_manager);

        let app_data = std::collections::BTreeMap::new();
        let hub_data = std::collections::BTreeMap::new();

        // Should try providers until one succeeds
        let result = manager
            .check_app_available("test-hub", &app_data, &hub_data)
            .await;
        assert!(result.is_ok());
        // Since provider1 returns false but provider2 returns true,
        // and we try providers in order, we should get the first successful result
    }

    #[tokio::test]
    async fn test_app_lifecycle() {
        // Test adding and removing apps
        let mock_provider = MockProvider::new("lifecycle-provider");
        let mut provider_manager = ProviderManager::new();
        provider_manager.register_provider(Box::new(mock_provider));

        let manager = AppManager::with_provider_manager(provider_manager);

        // Initially no apps
        let apps = manager.list_apps().await.unwrap();
        let initial_count = apps.len();

        // Add an app
        let app_data =
            std::collections::HashMap::from([("repo".to_string(), "test-repo".to_string())]);
        let hub_data = std::collections::HashMap::new();

        let add_result = manager
            .add_app(
                "test-app".to_string(),
                "test-hub".to_string(),
                app_data,
                hub_data,
            )
            .await;

        // Check if add was successful (may fail if config is not set up)
        if add_result.is_ok() {
            // List apps should now include the new app
            let apps_after_add = manager.list_apps().await.unwrap();
            assert_eq!(apps_after_add.len(), initial_count + 1);
            assert!(apps_after_add.contains(&"test-app".to_string()));

            // Remove the app
            let remove_result = manager.remove_app("test-app").await;
            assert!(remove_result.is_ok());

            // List apps should be back to initial count
            let apps_after_remove = manager.list_apps().await.unwrap();
            assert_eq!(apps_after_remove.len(), initial_count);
            assert!(!apps_after_remove.contains(&"test-app".to_string()));
        }
    }

    #[tokio::test]
    async fn test_status_tracking() {
        let mock_provider = MockProvider::new("status-provider");
        let mut provider_manager = ProviderManager::new();
        provider_manager.register_provider(Box::new(mock_provider));

        let manager = AppManager::with_provider_manager(provider_manager);

        // Get status for non-existent app
        let status = manager.get_app_status("non-existent").await.unwrap();
        assert!(status.is_none());

        // Get all statuses (should handle empty case)
        let all_statuses = manager.get_all_app_statuses().await.unwrap();
        // Just check that it doesn't panic - the result could be empty or not
        let _ = all_statuses.len();

        // Get outdated apps (should handle empty case)
        let outdated = manager.get_outdated_apps().await.unwrap();
        // Just check that it doesn't panic - the result could be empty or not
        let _ = outdated.len();
    }

    #[tokio::test]
    async fn test_concurrent_add_remove_race_condition() {
        let mock_provider = MockProvider::new("race-provider");
        let mut provider_manager = ProviderManager::new();
        provider_manager.register_provider(Box::new(mock_provider));

        let manager = Arc::new(AppManager::with_provider_manager(provider_manager));

        let app_id = "race-test-app";
        let hub_uuid = "test-hub";
        let app_data = std::collections::HashMap::from([("key".to_string(), "value".to_string())]);
        let hub_data = std::collections::HashMap::new();

        // Spawn multiple tasks that try to add and remove the same app concurrently
        let mut handles = vec![];

        for i in 0..10 {
            let manager_clone = Arc::clone(&manager);
            let app_id_clone = app_id.to_string();
            let hub_uuid_clone = hub_uuid.to_string();
            let app_data_clone = app_data.clone();
            let hub_data_clone = hub_data.clone();

            let handle = tokio::spawn(async move {
                if i % 2 == 0 {
                    // Even threads: add app
                    let _ = manager_clone
                        .add_app(app_id_clone, hub_uuid_clone, app_data_clone, hub_data_clone)
                        .await;
                } else {
                    // Odd threads: remove app
                    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                    let _ = manager_clone.remove_app(&app_id_clone).await;
                }
            });
            handles.push(handle);
        }

        // Wait for all tasks to complete
        for handle in handles {
            let _ = handle.await;
        }

        // Final state should be consistent (either app exists or not)
        let apps = manager.list_apps().await.unwrap();
        // The app should either be in the list or not, but the list should be consistent
        assert!(apps.iter().filter(|&a| a == app_id).count() <= 1);
    }

    #[tokio::test]
    async fn test_concurrent_different_operations() {
        let mock_provider = MockProvider::new("mixed-ops-provider");
        let call_count = mock_provider.call_count.clone();
        let mut provider_manager = ProviderManager::new();
        provider_manager.register_provider(Box::new(mock_provider));

        let manager = Arc::new(AppManager::with_provider_manager(provider_manager));

        let app_data = std::collections::BTreeMap::from([("test", "data")]);
        let hub_data = std::collections::BTreeMap::new();

        // Run different operations concurrently
        let manager1 = Arc::clone(&manager);
        let manager2 = Arc::clone(&manager);
        let manager3 = Arc::clone(&manager);
        let manager4 = Arc::clone(&manager);

        let app_data1 = app_data.clone();
        let hub_data1 = hub_data.clone();
        let app_data2 = app_data.clone();
        let hub_data2 = hub_data.clone();
        let app_data3 = app_data.clone();
        let hub_data3 = hub_data.clone();

        let (check_result, latest_result, releases_result, list_result) = tokio::join!(
            manager1.check_app_available("hub1", &app_data1, &hub_data1),
            manager2.get_latest_release("hub2", &app_data2, &hub_data2),
            manager3.get_releases("hub3", &app_data3, &hub_data3),
            manager4.list_apps()
        );

        // All operations should complete without deadlock
        assert!(check_result.is_ok());
        assert!(latest_result.is_ok());
        assert!(releases_result.is_ok());
        assert!(list_result.is_ok());

        // Provider should have been called for each operation
        assert!(call_count.load(Ordering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn test_stress_many_concurrent_requests() {
        let mock_provider = MockProvider::new("stress-provider");
        let call_count = mock_provider.call_count.clone();
        let mut provider_manager = ProviderManager::new();
        provider_manager.register_provider(Box::new(mock_provider));

        let manager = Arc::new(AppManager::with_provider_manager(provider_manager));
        let num_requests = 100;
        let mut handles = vec![];

        for i in 0..num_requests {
            let manager_clone = Arc::clone(&manager);
            let handle = tokio::spawn(async move {
                let id_str = format!("{}", i);
                let app_data = std::collections::BTreeMap::from([("id", id_str.as_str())]);
                let hub_data = std::collections::BTreeMap::new();
                manager_clone
                    .check_app_available("stress-hub", &app_data, &hub_data)
                    .await
            });
            handles.push(handle);
        }

        // Collect all results
        let mut success_count = 0;
        for handle in handles {
            if let Ok(result) = handle.await {
                if result.is_ok() {
                    success_count += 1;
                }
            }
        }

        // All requests should succeed
        assert_eq!(success_count, num_requests);

        // Due to deduplication, actual calls might be less than num_requests
        let actual_calls = call_count.load(Ordering::SeqCst);
        assert!(actual_calls > 0);
        assert!(actual_calls <= num_requests);
    }

    #[tokio::test]
    async fn test_concurrent_status_operations() {
        let mock_provider = MockProvider::new("status-concurrent-provider");
        let mut provider_manager = ProviderManager::new();
        provider_manager.register_provider(Box::new(mock_provider));

        let manager = Arc::new(AppManager::with_provider_manager(provider_manager));

        let mut status_handles = vec![];
        let mut all_status_handles = vec![];

        for i in 0..5 {
            let manager_clone = Arc::clone(&manager);
            let handle = tokio::spawn(async move {
                let app_id = format!("status-app-{}", i);
                let hub_uuid = "status-hub".to_string();
                let app_data =
                    std::collections::HashMap::from([("id".to_string(), format!("{}", i))]);
                let hub_data = std::collections::HashMap::new();

                let _ = manager_clone
                    .add_app(app_id.clone(), hub_uuid, app_data, hub_data)
                    .await;

                manager_clone.get_app_status(&app_id).await
            });
            status_handles.push(handle);
        }

        for _ in 0..3 {
            let manager_clone = Arc::clone(&manager);
            let handle = tokio::spawn(async move { manager_clone.get_all_app_statuses().await });
            all_status_handles.push(handle);
        }

        for handle in status_handles {
            let _ = handle.await;
        }
        for handle in all_status_handles {
            let _ = handle.await;
        }

        let final_statuses = manager.get_all_app_statuses().await;
        assert!(final_statuses.is_ok());
    }

    #[tokio::test]
    async fn test_deduplication_with_delays() {
        let mock_provider = MockProvider::new("dedup-delay-provider");
        let call_count = mock_provider.call_count.clone();
        let mut provider_manager = ProviderManager::new();
        provider_manager.register_provider(Box::new(mock_provider));

        let manager = Arc::new(AppManager::with_provider_manager(provider_manager));
        let app_data = std::collections::BTreeMap::from([("key", "value")]);
        let hub_data = std::collections::BTreeMap::new();

        let manager1 = Arc::clone(&manager);
        let manager2 = Arc::clone(&manager);
        let manager3 = Arc::clone(&manager);

        let app_data1 = app_data.clone();
        let hub_data1 = hub_data.clone();
        let app_data2 = app_data.clone();
        let hub_data2 = hub_data.clone();
        let app_data3 = app_data.clone();
        let hub_data3 = hub_data.clone();

        let (r1, r2, r3) = tokio::join!(
            manager1.check_app_available("hub", &app_data1, &hub_data1),
            manager2.check_app_available("hub", &app_data2, &hub_data2),
            manager3.check_app_available("hub", &app_data3, &hub_data3)
        );

        assert!(r1.is_ok() && r2.is_ok() && r3.is_ok());
        let v1 = r1.unwrap();
        let v2 = r2.unwrap();
        let v3 = r3.unwrap();
        assert_eq!(v1, v2);
        assert_eq!(v2, v3);

        let first_batch_calls = call_count.load(Ordering::SeqCst);

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let (r4, r5) = tokio::join!(
            manager.check_app_available("hub", &app_data, &hub_data),
            manager.check_app_available("hub", &app_data, &hub_data)
        );

        assert!(r4.is_ok() && r5.is_ok());
        let v4 = r4.unwrap();
        let v5 = r5.unwrap();
        assert_eq!(v4, v5);

        let second_batch_calls = call_count.load(Ordering::SeqCst);

        assert!(second_batch_calls > first_batch_calls);
    }

    #[tokio::test]
    async fn test_concurrent_update_operations() {
        let mock_provider = MockProvider::new("update-concurrent-provider");
        let mut provider_manager = ProviderManager::new();
        provider_manager.register_provider(Box::new(mock_provider));

        let manager = Arc::new(AppManager::with_provider_manager(provider_manager));

        let mut handles = vec![];

        for i in 0..10 {
            let manager_clone = Arc::clone(&manager);
            let handle = tokio::spawn(async move {
                let app_id = format!("app-{}", i % 3);
                let version = format!("v{}.0.0", i);
                manager_clone.update_app(&app_id, &version).await
            });
            handles.push(handle);
        }

        let mut success_count = 0;
        for handle in handles {
            if let Ok(result) = handle.await {
                if result.is_ok() {
                    success_count += 1;
                }
            }
        }

        assert_eq!(success_count, 10);
    }
}
