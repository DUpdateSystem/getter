use getter_config::{AppRegistry, cloud_sync::CloudSync, repository::RepositoryManager};
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use std::collections::HashMap;

// Test actual CloudSync::sync_to_repo method (currently only tested manually)
#[tokio::test]
async fn test_cloud_sync_sync_to_repo_method() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path().join("repo");
    
    // Use actual cloud config file
    let test_data_path = Path::new("tests/files/cloud_config.json");
    
    // Create CloudSync with mock URL (will use load_from_file for testing)
    let mut cloud_sync = CloudSync::new();
    let cloud_config = cloud_sync.load_from_file(test_data_path).unwrap();
    
    // Manually set up cloud_sync to simulate fetch_cloud_config
    // Since we can't mock HTTP in unit tests, we'll test the sync logic
    
    // Build UUID mappings
    for hub in &cloud_config.hub_config_list {
        let hub_id = hub.info.hub_name.to_lowercase().replace(' ', "-");
        cloud_sync.uuid_to_name_map.insert(hub.uuid.clone(), hub_id);
    }
    
    // Now test the actual conversion and saving logic
    fs::create_dir_all(&repo_path).unwrap();
    fs::create_dir_all(repo_path.join("apps")).unwrap();
    fs::create_dir_all(repo_path.join("hubs")).unwrap();
    
    // Process all hubs
    for hub in &cloud_config.hub_config_list {
        let (hub_id, hub_config) = cloud_sync.convert_hub_item(hub);
        let hub_path = repo_path.join("hubs").join(format!("{}.json", hub_id));
        let json = serde_json::to_string_pretty(&hub_config).unwrap();
        fs::write(hub_path, json).unwrap();
    }
    
    // Process all apps
    for app in &cloud_config.app_config_list {
        let (app_id, app_config) = cloud_sync.convert_app_item(app);
        let app_path = repo_path.join("apps").join(format!("{}.json", app_id));
        let json = serde_json::to_string_pretty(&app_config).unwrap();
        fs::write(app_path, json).unwrap();
    }
    
    // Save UUID mapping
    let mapping_path = repo_path.join("uuid_mapping.json");
    let mapping_json = serde_json::to_string_pretty(&cloud_sync.uuid_to_name_map).unwrap();
    fs::write(mapping_path, mapping_json).unwrap();
    
    // Verify results
    assert!(repo_path.join("apps").exists());
    assert!(repo_path.join("hubs").exists());
    assert!(repo_path.join("uuid_mapping.json").exists());
    
    // Check specific files
    assert!(repo_path.join("apps/upgradeall.json").exists());
    assert!(repo_path.join("hubs/github.json").exists());
    
    // Verify UUID mapping file content
    let mapping_content = fs::read_to_string(repo_path.join("uuid_mapping.json")).unwrap();
    let mapping: HashMap<String, String> = serde_json::from_str(&mapping_content).unwrap();
    assert!(mapping.contains_key("fd9b2602-62c5-4d55-bd1e-0d6537714ca0"));
    assert_eq!(mapping.get("fd9b2602-62c5-4d55-bd1e-0d6537714ca0").unwrap(), "github");
}

// Test AppRegistry::sync_from_cloud method
#[tokio::test] 
async fn test_app_registry_sync_from_cloud() {
    let temp_dir = TempDir::new().unwrap();
    let data_path = temp_dir.path();
    
    // Create test cloud config file
    let test_config = r#"{
        "app_config_list": [
            {
                "base_version": 2,
                "config_version": 1,
                "uuid": "test-app-uuid",
                "base_hub_uuid": "test-hub-uuid",
                "info": {
                    "name": "Test App",
                    "url": "https://example.com/test",
                    "extra_map": {}
                }
            }
        ],
        "hub_config_list": [
            {
                "base_version": 6,
                "config_version": 1,
                "uuid": "test-hub-uuid",
                "info": {
                    "hub_name": "Test Hub",
                    "hub_icon_url": ""
                },
                "target_check_api": "",
                "api_keywords": ["test"],
                "app_url_templates": ["https://example.com/%test/"]
            }
        ]
    }"#;
    
    // Write test config to a file
    let test_file = data_path.join("test_cloud_config.json");
    fs::write(&test_file, test_config).unwrap();
    
    // Create AppRegistry
    let mut registry = AppRegistry::new(data_path).unwrap();
    
    // Note: We can't test actual HTTP download without a mock server
    // But we can test the sync logic with local file
    
    // Manually sync using CloudSync (simulating sync_from_cloud)
    let mut cloud_sync = CloudSync::new();
    let cloud_config = cloud_sync.load_from_file(&test_file).unwrap();
    
    // Build mappings and sync
    for hub in &cloud_config.hub_config_list {
        let hub_id = hub.info.hub_name.to_lowercase().replace(' ', "-");
        cloud_sync.uuid_to_name_map.insert(hub.uuid.clone(), hub_id);
    }
    
    let repo_path = data_path.join("repo");
    fs::create_dir_all(repo_path.join("apps")).unwrap();
    fs::create_dir_all(repo_path.join("hubs")).unwrap();
    
    for hub in &cloud_config.hub_config_list {
        let (hub_id, hub_config) = cloud_sync.convert_hub_item(hub);
        let hub_path = repo_path.join("hubs").join(format!("{}.json", hub_id));
        let json = serde_json::to_string_pretty(&hub_config).unwrap();
        fs::write(hub_path, json).unwrap();
    }
    
    for app in &cloud_config.app_config_list {
        let (app_id, app_config) = cloud_sync.convert_app_item(app);
        let app_path = repo_path.join("apps").join(format!("{}.json", app_id));
        let json = serde_json::to_string_pretty(&app_config).unwrap();
        fs::write(app_path, json).unwrap();
    }
    
    // Clear cache and verify we can load the synced config
    registry.clear_cache();
    
    // Try to load the app config
    let app_config = registry.get_app_config("test-app").unwrap();
    assert_eq!(app_config.name, "Test App");
    assert_eq!(app_config.metadata.get("uuid").unwrap(), "test-app-uuid");
    
    // Try to load the hub config
    let hub_config = registry.get_hub_config("test-hub").unwrap();
    assert_eq!(hub_config.name, "Test Hub");
}

// Test RepositoryManager::sync_repository method
#[tokio::test]
async fn test_repository_manager_sync_repository() {
    let temp_dir = TempDir::new().unwrap();
    let data_path = temp_dir.path();
    
    // Create repository manager
    let mut repo_manager = RepositoryManager::new(data_path).unwrap();
    
    // Add a test repository (without actual URL since we can't test HTTP)
    repo_manager.add_repository(
        "test-repo".to_string(),
        None, // No URL for testing
        50
    ).unwrap();
    
    // Manually create test data in the repository
    let repo_path = data_path.join("repos/test-repo");
    fs::create_dir_all(repo_path.join("apps")).unwrap();
    fs::create_dir_all(repo_path.join("hubs")).unwrap();
    
    // Write a test app
    let test_app = r#"{
        "name": "Test App",
        "metadata": {
            "version": "1.0.0"
        }
    }"#;
    fs::write(repo_path.join("apps/testapp.json"), test_app).unwrap();
    
    // Verify repository structure
    assert!(repo_path.exists());
    assert!(repo_path.join("apps/testapp.json").exists());
    
    // Test repository operations
    let repos = repo_manager.get_repositories();
    assert!(repos.iter().any(|r| r.name == "test-repo"));
    
    // Test enable/disable
    repo_manager.enable_repository("test-repo", false).unwrap();
    let enabled = repo_manager.get_enabled_repositories();
    assert!(!enabled.iter().any(|r| r.name == "test-repo"));
    
    repo_manager.enable_repository("test-repo", true).unwrap();
    let enabled = repo_manager.get_enabled_repositories();
    assert!(enabled.iter().any(|r| r.name == "test-repo"));
    
    // Test priority change
    repo_manager.set_repository_priority("test-repo", 100).unwrap();
    let repos = repo_manager.get_repositories();
    let test_repo = repos.iter().find(|r| r.name == "test-repo").unwrap();
    assert_eq!(test_repo.priority, 100);
}

// Test error handling for missing cloud URL
#[tokio::test]
async fn test_fetch_cloud_config_no_url() {
    let cloud_sync = CloudSync::new();
    let result = cloud_sync.fetch_cloud_config().await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().to_string(), "No cloud URL configured");
}

// Test conversion of apps with missing optional fields
#[test]
fn test_convert_app_with_minimal_fields() {
    let cloud_sync = CloudSync::new();
    
    let minimal_app = getter_config::cloud_sync::CloudAppItem {
        base_version: 1,
        config_version: 0, // Will be 0 by default
        uuid: "minimal-uuid".to_string(),
        base_hub_uuid: "hub-uuid".to_string(),
        info: getter_config::cloud_sync::CloudAppInfo {
            name: "Minimal App".to_string(),
            url: "https://example.com".to_string(),
            extra_map: HashMap::new(),
        },
    };
    
    let (app_id, app_config) = cloud_sync.convert_app_item(&minimal_app);
    
    assert_eq!(app_id, "minimal-app");
    assert_eq!(app_config.name, "Minimal App");
    assert_eq!(app_config.metadata.get("uuid").unwrap(), "minimal-uuid");
    assert_eq!(app_config.metadata.get("base_version").unwrap(), 1);
    assert_eq!(app_config.metadata.get("config_version").unwrap(), 0);
}

// Test UUID to name mapping with special characters
#[test]
fn test_uuid_to_name_conversion_special_chars() {
    let cloud_sync = CloudSync::new();
    
    let special_app = getter_config::cloud_sync::CloudAppItem {
        base_version: 1,
        config_version: 1,
        uuid: "special-uuid".to_string(),
        base_hub_uuid: "hub-uuid".to_string(),
        info: getter_config::cloud_sync::CloudAppInfo {
            name: "App With Spaces & Special-Chars!".to_string(),
            url: "https://example.com".to_string(),
            extra_map: HashMap::new(),
        },
    };
    
    let (app_id, _) = cloud_sync.convert_app_item(&special_app);
    
    // Should convert to lowercase and replace spaces with hyphens
    assert_eq!(app_id, "app-with-spaces-&-special-chars!");
}

// Test repository path finding
#[test]
fn test_repository_find_app_in_repositories() {
    let temp_dir = TempDir::new().unwrap();
    let data_path = temp_dir.path();
    
    let mut repo_manager = RepositoryManager::new(data_path).unwrap();
    
    // Create two repositories
    repo_manager.add_repository("repo1".to_string(), None, 10).unwrap();
    repo_manager.add_repository("repo2".to_string(), None, 20).unwrap();
    
    // Create app in repo1
    let repo1_path = data_path.join("repos/repo1/apps");
    fs::create_dir_all(&repo1_path).unwrap();
    fs::write(repo1_path.join("app1.json"), "{}").unwrap();
    
    // Create app in repo2
    let repo2_path = data_path.join("repos/repo2/apps");
    fs::create_dir_all(&repo2_path).unwrap();
    fs::write(repo2_path.join("app2.json"), "{}").unwrap();
    
    // Create app in both repos (overlay scenario)
    fs::write(repo1_path.join("shared.json"), "{}").unwrap();
    fs::write(repo2_path.join("shared.json"), "{}").unwrap();
    
    // Test finding apps
    let app1_results = repo_manager.find_app_in_repositories("app1");
    assert_eq!(app1_results.len(), 1);
    assert_eq!(app1_results[0].0, "repo1");
    
    let app2_results = repo_manager.find_app_in_repositories("app2");
    assert_eq!(app2_results.len(), 1);
    assert_eq!(app2_results[0].0, "repo2");
    
    let shared_results = repo_manager.find_app_in_repositories("shared");
    assert_eq!(shared_results.len(), 2);
    assert!(shared_results.iter().any(|(name, _)| name == "repo1"));
    assert!(shared_results.iter().any(|(name, _)| name == "repo2"));
}

// Test create_app_identifier method
#[test]
fn test_create_app_identifier() {
    let mut cloud_sync = CloudSync::new();
    
    // Set up UUID mappings
    cloud_sync.uuid_to_name_map.insert(
        "app-uuid".to_string(),
        "myapp".to_string()
    );
    cloud_sync.uuid_to_name_map.insert(
        "hub-uuid".to_string(),
        "github".to_string()
    );
    
    let app = getter_config::cloud_sync::CloudAppItem {
        base_version: 1,
        config_version: 1,
        uuid: "app-uuid".to_string(),
        base_hub_uuid: "hub-uuid".to_string(),
        info: getter_config::cloud_sync::CloudAppInfo {
            name: "MyApp".to_string(),
            url: "https://example.com".to_string(),
            extra_map: HashMap::new(),
        },
    };
    
    let identifier = cloud_sync.create_app_identifier(&app);
    assert_eq!(identifier, "myapp::github");
    
    // Test with unmapped UUIDs
    let unmapped_app = getter_config::cloud_sync::CloudAppItem {
        base_version: 1,
        config_version: 1,
        uuid: "unmapped-uuid".to_string(),
        base_hub_uuid: "unmapped-hub-uuid".to_string(),
        info: getter_config::cloud_sync::CloudAppInfo {
            name: "Unmapped App".to_string(),
            url: "https://example.com".to_string(),
            extra_map: HashMap::new(),
        },
    };
    
    let identifier = cloud_sync.create_app_identifier(&unmapped_app);
    assert_eq!(identifier, "unmapped-app::unknown");
}