use getter_config::{cloud_sync::CloudSync, repository::RepositoryManager, AppRegistry};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

#[test]
fn test_multi_repository_loading() {
    let temp_dir = TempDir::new().unwrap();
    let data_path = temp_dir.path();

    // Setup repository manager
    let mut repo_manager = RepositoryManager::new(data_path).unwrap();
    repo_manager.init_default_repositories().unwrap();

    // Add custom repository
    repo_manager
        .add_repository("community".to_string(), None, 50)
        .unwrap();

    // Verify repositories are set up correctly
    let repos = repo_manager.get_repositories();
    assert_eq!(repos.len(), 3);
    assert_eq!(repos[0].name, "local");
    assert_eq!(repos[0].priority, 100);
    assert_eq!(repos[1].name, "community");
    assert_eq!(repos[1].priority, 50);
    assert_eq!(repos[2].name, "getter-main");
    assert_eq!(repos[2].priority, 0);
}

#[tokio::test]
async fn test_cloud_sync_with_real_data() {
    let temp_dir = TempDir::new().unwrap();
    let data_path = temp_dir.path();

    // Load test data from main_repo.json
    let test_data_path = Path::new("tests/data/cloud_configs/main_repo.json");
    let cloud_sync = CloudSync::new();
    let cloud_config = cloud_sync.load_from_file(test_data_path).unwrap();

    // Verify cloud configuration structure
    assert_eq!(cloud_config.app_config_list.len(), 4);
    assert_eq!(cloud_config.hub_config_list.len(), 3);

    // Check specific apps
    let upgradeall = cloud_config
        .app_config_list
        .iter()
        .find(|app| app.info.name == "UpgradeAll")
        .unwrap();
    assert_eq!(upgradeall.uuid, "f27f71e1-d7a1-4fd1-bbcc-9744380611a1");
    assert_eq!(
        upgradeall.base_hub_uuid,
        "fd9b2602-62c5-4d55-bd1e-0d6537714ca0"
    );
    assert_eq!(
        upgradeall.info.extra_map.get("android_app_package"),
        Some(&"net.xzos.upgradeall".to_string())
    );

    // Check specific hubs
    let github_hub = cloud_config
        .hub_config_list
        .iter()
        .find(|hub| hub.info.hub_name == "GitHub")
        .unwrap();
    assert_eq!(github_hub.uuid, "fd9b2602-62c5-4d55-bd1e-0d6537714ca0");
    assert_eq!(github_hub.api_keywords, vec!["owner", "repo"]);

    // Sync to repository
    let cloud_sync_mut = CloudSync::new();
    let repo_path = data_path.join("repos/main");
    fs::create_dir_all(&repo_path).unwrap();

    // Manually sync the loaded config
    for hub in &cloud_config.hub_config_list {
        let (hub_id, hub_config) = cloud_sync_mut.convert_hub_item(hub);
        let hub_path = repo_path.join("hubs").join(format!("{}.json", hub_id));
        fs::create_dir_all(hub_path.parent().unwrap()).unwrap();
        let json = serde_json::to_string_pretty(&hub_config).unwrap();
        fs::write(hub_path, json).unwrap();
    }

    for app in &cloud_config.app_config_list {
        let (app_id, app_config) = cloud_sync_mut.convert_app_item(app);
        let app_path = repo_path.join("apps").join(format!("{}.json", app_id));
        fs::create_dir_all(app_path.parent().unwrap()).unwrap();
        let json = serde_json::to_string_pretty(&app_config).unwrap();
        fs::write(app_path, json).unwrap();
    }

    // Verify files were created
    assert!(repo_path.join("apps/upgradeall.json").exists());
    assert!(repo_path.join("apps/apkgrabber.json").exists());
    assert!(repo_path.join("hubs/github.json").exists());
    assert!(repo_path.join("hubs/google-play.json").exists());
}

#[tokio::test]
async fn test_repository_overlay() {
    let temp_dir = TempDir::new().unwrap();
    let data_path = temp_dir.path();

    // Create main repository
    let main_repo_path = data_path.join("repos/main");
    fs::create_dir_all(main_repo_path.join("apps")).unwrap();
    fs::create_dir_all(main_repo_path.join("hubs")).unwrap();

    // Add app to main repo
    let main_app_config = serde_json::json!({
        "name": "TestApp",
        "metadata": {
            "version": "1.0.0",
            "description": "Main repo version"
        }
    });
    fs::write(
        main_repo_path.join("apps/testapp.json"),
        serde_json::to_string_pretty(&main_app_config).unwrap(),
    )
    .unwrap();

    // Create community repository with higher priority
    let community_repo_path = data_path.join("repos/community");
    fs::create_dir_all(community_repo_path.join("apps")).unwrap();

    // Add overlaying config to community repo
    let community_app_config = serde_json::json!({
        "metadata": {
            "version": "2.0.0",
            "community": true
        }
    });
    fs::write(
        community_repo_path.join("apps/testapp.json"),
        serde_json::to_string_pretty(&community_app_config).unwrap(),
    )
    .unwrap();

    // Create local config overlay
    let config_path = data_path.join("config");
    fs::create_dir_all(config_path.join("apps")).unwrap();

    let local_config = serde_json::json!({
        "metadata": {
            "custom": "user-override"
        }
    });
    fs::write(
        config_path.join("apps/testapp.json"),
        serde_json::to_string_pretty(&local_config).unwrap(),
    )
    .unwrap();

    // Setup repository manager
    let mut repo_manager = RepositoryManager::new(data_path).unwrap();
    repo_manager
        .add_repository("main".to_string(), None, 0)
        .unwrap();
    repo_manager
        .add_repository("community".to_string(), None, 50)
        .unwrap();

    // Create registry with repository manager
    let mut registry = AppRegistry::new(data_path).unwrap();

    // Load merged config
    let app_config = registry.get_app_config("testapp").unwrap();

    // Verify overlay merging
    assert_eq!(app_config.name, "TestApp");
    assert_eq!(app_config.metadata.get("version").unwrap(), "2.0.0"); // Community overlay
    assert_eq!(
        app_config.metadata.get("description").unwrap(),
        "Main repo version"
    ); // Main repo
    assert_eq!(app_config.metadata.get("community").unwrap(), true); // Community overlay
    assert_eq!(app_config.metadata.get("custom").unwrap(), "user-override"); // Local config
}

#[tokio::test]
async fn test_multiple_cloud_sources() {
    let temp_dir = TempDir::new().unwrap();
    let _data_path = temp_dir.path();

    // Load both test repositories
    let main_repo_data = Path::new("tests/data/cloud_configs/main_repo.json");
    let community_repo_data = Path::new("tests/data/cloud_configs/community_repo.json");

    let cloud_sync = CloudSync::new();
    let main_config = cloud_sync.load_from_file(main_repo_data).unwrap();
    let community_config = cloud_sync.load_from_file(community_repo_data).unwrap();

    // Main repo should have Android apps
    assert_eq!(main_config.app_config_list.len(), 4);
    let has_android_apps = main_config
        .app_config_list
        .iter()
        .any(|app| app.info.extra_map.contains_key("android_app_package"));
    assert!(has_android_apps);

    // Community repo should have development tools
    assert_eq!(community_config.app_config_list.len(), 3);
    let has_dev_tools = community_config
        .app_config_list
        .iter()
        .any(|app| app.info.name == "Rust" || app.info.name == "Neovim");
    assert!(has_dev_tools);

    // Verify different hub types
    assert_eq!(main_config.hub_config_list.len(), 3);
    assert_eq!(community_config.hub_config_list.len(), 1);

    let docker_hub = community_config
        .hub_config_list
        .iter()
        .find(|hub| hub.info.hub_name == "Docker Hub")
        .unwrap();
    assert_eq!(docker_hub.api_keywords, vec!["image", "tag"]);
}

#[test]
fn test_uuid_to_name_mapping() {
    let cloud_sync = CloudSync::new();

    // Test app conversion
    let cloud_app = getter_config::cloud_sync::CloudAppItem {
        base_version: 6,
        config_version: 1,
        uuid: "f27f71e1-d7a1-4fd1-bbcc-9744380611a1".to_string(),
        base_hub_uuid: "fd9b2602-62c5-4d55-bd1e-0d6537714ca0".to_string(),
        info: getter_config::cloud_sync::CloudAppInfo {
            name: "UpgradeAll".to_string(),
            url: "https://github.com/DUpdateSystem/UpgradeAll".to_string(),
            extra_map: std::collections::HashMap::from([(
                "android_app_package".to_string(),
                "net.xzos.upgradeall".to_string(),
            )]),
        },
    };

    let (app_id, app_config) = cloud_sync.convert_app_item(&cloud_app);

    // Should convert to lowercase with hyphens
    assert_eq!(app_id, "upgradeall");
    assert_eq!(app_config.name, "UpgradeAll");

    // Should preserve all metadata
    assert_eq!(
        app_config.metadata.get("uuid").unwrap(),
        "f27f71e1-d7a1-4fd1-bbcc-9744380611a1"
    );
    assert_eq!(
        app_config.metadata.get("base_hub_uuid").unwrap(),
        "fd9b2602-62c5-4d55-bd1e-0d6537714ca0"
    );
    assert_eq!(
        app_config.metadata.get("android_app_package").unwrap(),
        "net.xzos.upgradeall"
    );
}

#[test]
fn test_app_identifier_with_real_data() {
    // Test parsing real app identifiers
    let identifier = getter_config::AppIdentifier::parse("upgradeall::github").unwrap();
    assert_eq!(identifier.app_id, "upgradeall");
    assert_eq!(identifier.hub_id, "github");
    assert_eq!(identifier.to_string(), "upgradeall::github");

    let identifier2 = getter_config::AppIdentifier::parse("firefox::f-droid").unwrap();
    assert_eq!(identifier2.app_id, "firefox");
    assert_eq!(identifier2.hub_id, "f-droid");

    // Test invalid formats
    assert!(getter_config::AppIdentifier::parse("invalid").is_err());
    assert!(getter_config::AppIdentifier::parse("too::many::parts").is_err());
}
