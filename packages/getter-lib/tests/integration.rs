// Integration tests for the getter library
use std::collections::HashMap;

// Test that all packages can be imported and used together through the main library
#[tokio::test]
async fn test_core_integration() {
    use getter::Core;
    
    let core = Core::new();
    let apps = core.list_apps().await;
    assert!(apps.is_ok(), "Core should be able to list apps");
}

#[tokio::test]
async fn test_appmanager_integration() {
    use getter::get_app_manager;
    
    let manager = get_app_manager();
    let apps = manager.list_apps().await;
    assert!(apps.is_ok(), "AppManager should be able to list apps");
}

#[tokio::test]
async fn test_config_integration() {
    use getter::RuleList;
    
    let mut rule_list = RuleList::new();
    
    let app_data = HashMap::from([
        ("owner".to_string(), "rust-lang".to_string()),
        ("repo".to_string(), "rust".to_string()),
    ]);
    let hub_data = HashMap::new();

    let added = rule_list.add_tracked_app(
        "rust_lang_rust".to_string(),
        "github".to_string(),
        app_data,
        hub_data,
    );

    assert!(added, "Should be able to add a tracked app");
    assert_eq!(rule_list.tracked_apps.len(), 1);
}

#[tokio::test]
async fn test_provider_integration() {
    use getter::{GitHubProvider, ProviderManager};
    
    let mut provider_manager = ProviderManager::new();
    provider_manager.register_provider(Box::new(GitHubProvider::new()));

    let provider = provider_manager.get_provider("github");
    assert!(provider.is_some(), "GitHub provider should be available");
}

#[tokio::test]
async fn test_cache_integration() {
    use getter::cache::{CacheBackend, CacheManager};
    use std::error::Error;

    struct TestBackend;
    
    #[async_trait::async_trait]
    impl CacheBackend for TestBackend {
        async fn get(&self, _key: &str) -> Result<Option<String>, Box<dyn Error>> {
            Ok(Some("test_value".to_string()))
        }
        
        async fn set(&self, _key: &str, _value: &str) -> Result<(), Box<dyn Error>> {
            Ok(())
        }
        
        async fn remove(&self, _key: &str) -> Result<(), Box<dyn Error>> {
            Ok(())
        }
        
        async fn clear(&self) -> Result<(), Box<dyn Error>> {
            Ok(())
        }
    }

    let cache_manager = CacheManager::new(Box::new(TestBackend));
    let result = cache_manager.get("test_key").await;
    assert!(result.is_ok(), "Cache should be able to get values");
    assert_eq!(result.unwrap(), Some("test_value".to_string()));
}

#[tokio::test]
async fn test_utils_integration() {
    use getter::utils::versioning::Version;

    let v1 = Version::new("1.0.0".to_string());
    let v2 = Version::new("1.1.0".to_string());

    assert!(v1.is_valid(), "Version 1.0.0 should be valid");
    assert!(v2.is_valid(), "Version 1.1.0 should be valid");
}

#[tokio::test]
async fn test_rpc_integration() {
    use getter::GetterRpcClient;

    let _client = GetterRpcClient::new_http("http://localhost:8080");
    // Client is created successfully if this point is reached
}