use getter_config::{cloud_sync::CloudSync, repository::RepositoryManager, AppRegistry};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

#[test]
fn test_real_cloud_config_parsing() {
    // Use the actual cloud config file from getter-provider tests
    let test_data_path = Path::new("tests/files/cloud_config.json");
    assert!(
        test_data_path.exists(),
        "Test data file not found at {:?}",
        test_data_path
    );

    let cloud_sync = CloudSync::new();
    let cloud_config = cloud_sync.load_from_file(test_data_path).unwrap();

    // Verify the actual data structure
    println!(
        "Loaded {} apps and {} hubs",
        cloud_config.app_config_list.len(),
        cloud_config.hub_config_list.len()
    );

    assert_eq!(cloud_config.app_config_list.len(), 346);
    assert_eq!(cloud_config.hub_config_list.len(), 13);

    // Check for known apps
    let upgradeall = cloud_config
        .app_config_list
        .iter()
        .find(|app| app.info.name == "UpgradeAll")
        .expect("UpgradeAll should exist");
    assert_eq!(upgradeall.uuid, "f27f71e1-d7a1-4fd1-bbcc-9744380611a1");
    assert_eq!(
        upgradeall.base_hub_uuid,
        "fd9b2602-62c5-4d55-bd1e-0d6537714ca0"
    );

    let apkgrabber = cloud_config
        .app_config_list
        .iter()
        .find(|app| app.info.name == "APKGrabber")
        .expect("APKGrabber should exist");
    assert_eq!(apkgrabber.uuid, "ec2f237e-a502-4a1c-864b-3b64eaa75303");

    // Check for known hubs
    let github_hub = cloud_config
        .hub_config_list
        .iter()
        .find(|hub| hub.info.hub_name == "GitHub")
        .expect("GitHub hub should exist");
    assert_eq!(github_hub.uuid, "fd9b2602-62c5-4d55-bd1e-0d6537714ca0");

    let google_play_hub = cloud_config
        .hub_config_list
        .iter()
        .find(|hub| hub.info.hub_name == "Google Play")
        .expect("Google Play hub should exist");
    assert_eq!(google_play_hub.uuid, "65c2f60c-7d08-48b8-b4ba-ac6ee924f6fa");

    // Verify extra_map fields exist
    let android_apps: Vec<_> = cloud_config
        .app_config_list
        .iter()
        .filter(|app| app.info.extra_map.contains_key("android_app_package"))
        .collect();
    println!(
        "Found {} Android apps with package names",
        android_apps.len()
    );
    assert!(android_apps.len() > 100); // Should have many Android apps
}

#[tokio::test]
async fn test_sync_real_cloud_data_to_repo() {
    let temp_dir = TempDir::new().unwrap();
    let data_path = temp_dir.path();

    // Load the actual cloud config
    let test_data_path = Path::new("tests/files/cloud_config.json");
    let mut cloud_sync = CloudSync::new();
    let cloud_config = cloud_sync.load_from_file(test_data_path).unwrap();

    // Sync to repository
    let repo_path = data_path.join("repos/main");
    fs::create_dir_all(&repo_path).unwrap();

    // Build UUID mappings for all hubs first
    for hub in &cloud_config.hub_config_list {
        let hub_id = hub.info.hub_name.to_lowercase().replace(' ', "-");
        cloud_sync.uuid_to_name_map.insert(hub.uuid.clone(), hub_id);
    }

    // Convert and save all configs
    let apps_dir = repo_path.join("apps");
    let hubs_dir = repo_path.join("hubs");
    fs::create_dir_all(&apps_dir).unwrap();
    fs::create_dir_all(&hubs_dir).unwrap();

    // Process hubs
    let mut hub_count = 0;
    for hub in &cloud_config.hub_config_list {
        let (hub_id, hub_config) = cloud_sync.convert_hub_item(hub);
        let hub_path = hubs_dir.join(format!("{}.json", hub_id));
        let json = serde_json::to_string_pretty(&hub_config).unwrap();
        fs::write(hub_path, json).unwrap();
        hub_count += 1;
    }

    // Process apps
    let mut app_count = 0;
    for app in &cloud_config.app_config_list {
        let (app_id, app_config) = cloud_sync.convert_app_item(app);
        let app_path = apps_dir.join(format!("{}.json", app_id));
        let json = serde_json::to_string_pretty(&app_config).unwrap();
        fs::write(app_path, json).unwrap();
        app_count += 1;
    }

    println!("Synced {} apps and {} hubs", app_count, hub_count);
    assert_eq!(app_count, 346);
    assert_eq!(hub_count, 13);

    // Verify some key files exist with correct names
    assert!(hubs_dir.join("github.json").exists());
    assert!(hubs_dir.join("google-play.json").exists());
    assert!(hubs_dir.join("f-droid.json").exists());
    assert!(apps_dir.join("upgradeall.json").exists());
    assert!(apps_dir.join("apkgrabber.json").exists());
    assert!(apps_dir.join("1password.json").exists());
}

#[tokio::test]
async fn test_multi_repo_with_real_data() {
    let temp_dir = TempDir::new().unwrap();
    let data_path = temp_dir.path();

    // Setup repository manager
    let mut repo_manager = RepositoryManager::new(data_path).unwrap();

    // Add main repository (simulating cloud sync)
    repo_manager
        .add_repository(
            "main".to_string(),
            Some(
                "https://raw.githubusercontent.com/DUpdateSystem/getter/master/cloud_config.json"
                    .to_string(),
            ),
            0,
        )
        .unwrap();

    // Sync the actual cloud config to main repo
    let test_data_path = Path::new("tests/files/cloud_config.json");
    let mut cloud_sync = CloudSync::new();
    let cloud_config = cloud_sync.load_from_file(test_data_path).unwrap();

    let main_repo_path = data_path.join("repos/main");
    fs::create_dir_all(&main_repo_path).unwrap();

    // Build UUID mappings
    for hub in &cloud_config.hub_config_list {
        let hub_id = hub.info.hub_name.to_lowercase().replace(' ', "-");
        cloud_sync.uuid_to_name_map.insert(hub.uuid.clone(), hub_id);
    }

    // Sync hubs
    fs::create_dir_all(main_repo_path.join("hubs")).unwrap();
    for hub in &cloud_config.hub_config_list {
        let (hub_id, hub_config) = cloud_sync.convert_hub_item(hub);
        let hub_path = main_repo_path.join("hubs").join(format!("{}.json", hub_id));
        let json = serde_json::to_string_pretty(&hub_config).unwrap();
        fs::write(hub_path, json).unwrap();
    }

    // Sync a subset of apps for testing
    fs::create_dir_all(main_repo_path.join("apps")).unwrap();
    for app in cloud_config.app_config_list.iter().take(10) {
        let (app_id, app_config) = cloud_sync.convert_app_item(app);
        let app_path = main_repo_path.join("apps").join(format!("{}.json", app_id));
        let json = serde_json::to_string_pretty(&app_config).unwrap();
        fs::write(app_path, json).unwrap();
    }

    // Add a community overlay repository with higher priority
    repo_manager
        .add_repository("community".to_string(), None, 50)
        .unwrap();

    let community_repo_path = data_path.join("repos/community");
    fs::create_dir_all(community_repo_path.join("apps")).unwrap();

    // Add an overlay for UpgradeAll with modified metadata
    let overlay_config = serde_json::json!({
        "metadata": {
            "community_version": "2.0.0",
            "community_notes": "Enhanced by community"
        }
    });
    fs::write(
        community_repo_path.join("apps/upgradeall.json"),
        serde_json::to_string_pretty(&overlay_config).unwrap(),
    )
    .unwrap();

    // Create AppRegistry and test the overlay
    let mut registry = AppRegistry::new(data_path).unwrap();
    let app_config = registry.get_app_config("upgradeall").unwrap();

    // Should have both original and overlay metadata
    assert_eq!(app_config.name, "UpgradeAll");
    assert!(app_config.metadata.contains_key("uuid"));
    assert_eq!(
        app_config.metadata.get("community_version").unwrap(),
        "2.0.0"
    );
    assert_eq!(
        app_config.metadata.get("community_notes").unwrap(),
        "Enhanced by community"
    );

    // Test listing available apps
    let available_apps = registry.list_available_apps().unwrap();
    assert!(available_apps.contains(&"upgradeall".to_string()));

    // Test listing available hubs
    let available_hubs = registry.list_available_hubs().unwrap();
    assert!(available_hubs.contains(&"github".to_string()));
    assert!(available_hubs.contains(&"google-play".to_string()));
}

#[test]
fn test_uuid_mapping_with_real_data() {
    let test_data_path = Path::new("tests/files/cloud_config.json");
    let cloud_sync = CloudSync::new();
    let cloud_config = cloud_sync.load_from_file(test_data_path).unwrap();

    // Test conversion of real UUIDs to human-readable names
    let known_mappings = [
        ("f27f71e1-d7a1-4fd1-bbcc-9744380611a1", "upgradeall"),
        ("ec2f237e-a502-4a1c-864b-3b64eaa75303", "apkgrabber"),
        ("fd9b2602-62c5-4d55-bd1e-0d6537714ca0", "github"),
        ("65c2f60c-7d08-48b8-b4ba-ac6ee924f6fa", "google-play"),
        ("6a6d590b-1809-41bf-8ce3-7e3f6c8da945", "f-droid"),
    ];

    // Verify hub conversions
    for (uuid, expected_name) in &known_mappings[2..] {
        let hub = cloud_config
            .hub_config_list
            .iter()
            .find(|h| h.uuid == *uuid)
            .unwrap_or_else(|| panic!("Hub with UUID {} should exist", uuid));

        let (hub_id, _) = cloud_sync.convert_hub_item(hub);
        assert_eq!(
            hub_id, *expected_name,
            "Hub {} should map to {}",
            uuid, expected_name
        );
    }

    // Verify app conversions
    for (uuid, expected_name) in &known_mappings[..2] {
        let app = cloud_config
            .app_config_list
            .iter()
            .find(|a| a.uuid == *uuid)
            .unwrap_or_else(|| panic!("App with UUID {} should exist", uuid));

        let (app_id, _) = cloud_sync.convert_app_item(app);
        assert_eq!(
            app_id, *expected_name,
            "App {} should map to {}",
            uuid, expected_name
        );
    }
}

#[test]
fn test_app_categories_from_real_data() {
    let test_data_path = Path::new("tests/files/cloud_config.json");
    let cloud_sync = CloudSync::new();
    let cloud_config = cloud_sync.load_from_file(test_data_path).unwrap();

    // Group apps by hub
    let mut apps_by_hub: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    for app in &cloud_config.app_config_list {
        let hub_name = cloud_config
            .hub_config_list
            .iter()
            .find(|h| h.uuid == app.base_hub_uuid)
            .map(|h| h.info.hub_name.clone())
            .unwrap_or_else(|| "Unknown".to_string());

        apps_by_hub
            .entry(hub_name)
            .or_default()
            .push(app.info.name.clone());
    }

    // Print statistics
    println!("\nApp distribution by hub:");
    for (hub, apps) in &apps_by_hub {
        println!("  {}: {} apps", hub, apps.len());
    }

    // Verify we have apps from multiple sources
    assert!(apps_by_hub.contains_key("GitHub"));
    assert!(apps_by_hub.contains_key("Google Play"));
    assert!(apps_by_hub.contains_key("F-droid"));

    // Verify GitHub has many apps
    assert!(apps_by_hub.get("GitHub").unwrap().len() > 50);
}
