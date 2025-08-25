// Core module that ties everything together
use std::sync::Arc;
use tokio::sync::Mutex;

pub use getter_appmanager::{get_app_manager, AppManager, AppStatus, AppStatusInfo};
pub use getter_cache::{CacheManager, LegacyCacheManager};
pub use getter_config::{get_world_list, init_world_list, RuleList, TrackedApp, get_data_path};
pub use getter_provider::{ProviderManager, GitHubProvider, ReleaseData, AssetData};
pub use getter_rpc::{GetterRpcServer, GetterRpcClient};
pub use getter_utils::{http, versioning, time};

pub struct Core {
    app_manager: &'static AppManager,
}

impl Core {
    pub fn new() -> Self {
        Self {
            app_manager: get_app_manager(),
        }
    }

    pub async fn add_app(
        &self,
        app_id: String,
        hub_uuid: String,
        app_data: std::collections::HashMap<String, String>,
        hub_data: std::collections::HashMap<String, String>,
    ) -> Result<String, String> {
        self.app_manager.add_app(app_id, hub_uuid, app_data, hub_data).await
    }

    pub async fn remove_app(&self, app_id: &str) -> Result<bool, String> {
        self.app_manager.remove_app(app_id).await
    }

    pub async fn list_apps(&self) -> Result<Vec<String>, String> {
        self.app_manager.list_apps().await
    }

    pub async fn update_app(&self, app_id: &str, version: &str) -> Result<String, String> {
        self.app_manager.update_app(app_id, version).await
    }

    pub async fn get_app_status(&self, app_id: &str) -> Result<Option<AppStatusInfo>, String> {
        self.app_manager.get_app_status(app_id).await
    }

    pub async fn get_all_app_statuses(&self) -> Result<Vec<AppStatusInfo>, String> {
        self.app_manager.get_all_app_statuses().await
    }

    pub async fn get_outdated_apps(&self) -> Result<Vec<AppStatusInfo>, String> {
        self.app_manager.get_outdated_apps().await
    }

    pub async fn check_app_available(
        &self,
        hub_uuid: &str,
        app_data: &std::collections::BTreeMap<&str, &str>,
        hub_data: &std::collections::BTreeMap<&str, &str>,
    ) -> Result<bool, String> {
        self.app_manager.check_app_available(hub_uuid, app_data, hub_data).await
    }

    pub async fn get_latest_release(
        &self,
        hub_uuid: &str,
        app_data: &std::collections::BTreeMap<&str, &str>,
        hub_data: &std::collections::BTreeMap<&str, &str>,
    ) -> Result<ReleaseData, String> {
        self.app_manager.get_latest_release(hub_uuid, app_data, hub_data).await
    }

    pub async fn get_releases(
        &self,
        hub_uuid: &str,
        app_data: &std::collections::BTreeMap<&str, &str>,
        hub_data: &std::collections::BTreeMap<&str, &str>,
    ) -> Result<Vec<ReleaseData>, String> {
        self.app_manager.get_releases(hub_uuid, app_data, hub_data).await
    }
}

impl Default for Core {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_core_creation() {
        let core = Core::new();
        let result = core.list_apps().await;
        assert!(result.is_ok());
    }
}