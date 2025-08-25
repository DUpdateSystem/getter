//! Getter - A unified library for application update management
//!
//! This library provides a complete solution for managing application updates
//! across different platforms and package managers.

pub use getter_appmanager as appmanager;
pub use getter_cache as cache;
pub use getter_config as config;
pub use getter_core as core;
pub use getter_provider as provider;
pub use getter_rpc as rpc;
pub use getter_utils as utils;

// Re-export commonly used types for convenience
pub use getter_core::{Core, AppStatus, AppStatusInfo};
pub use getter_appmanager::{get_app_manager, AppManager};
pub use getter_rpc::{GetterRpcClient, GetterRpcServer};
pub use getter_config::{RuleList, TrackedApp, get_world_list};
pub use getter_provider::{ProviderManager, GitHubProvider, ReleaseData, AssetData};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_library_integration() {
        // Test that all major components can be accessed through the library
        let core = Core::new();
        let manager = get_app_manager();
        
        // Test core operations
        let apps = core.list_apps().await;
        assert!(apps.is_ok(), "Core should be able to list apps");
        
        // Test manager operations
        let apps = manager.list_apps().await;
        assert!(apps.is_ok(), "AppManager should be able to list apps");
        
        // Test RPC client creation
        let _client = GetterRpcClient::new_http("http://localhost:8080");
        
        // Test provider creation
        let _provider = GitHubProvider::new();
        
        // Test config creation
        let _rule_list = RuleList::new();
    }
}