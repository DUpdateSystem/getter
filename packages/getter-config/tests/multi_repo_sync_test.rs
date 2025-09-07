use getter_config::{cloud_sync::CloudSync, repository::RepositoryManager, AppRegistry};
use std::fs;
use tempfile::TempDir;
use tokio::time::{Duration, Instant};

// Test syncing multiple repositories
#[tokio::test]
async fn test_sync_multiple_repositories() {
    let temp_dir = TempDir::new().unwrap();
    let data_path = temp_dir.path();

    // Create repository manager
    let mut repo_manager = RepositoryManager::new(data_path).unwrap();

    // Add multiple repositories with different priorities
    repo_manager
        .add_repository(
            "main".to_string(),
            None, // Use local files for testing
            0,
        )
        .unwrap();

    repo_manager
        .add_repository("community".to_string(), None, 50)
        .unwrap();

    repo_manager
        .add_repository("testing".to_string(), None, 75)
        .unwrap();

    repo_manager
        .add_repository("local".to_string(), None, 100)
        .unwrap();

    // Create test data for each repository
    let repos = vec![
        ("main", 0, vec!["app1", "app2", "shared"]),
        ("community", 50, vec!["app3", "app4", "shared"]),
        ("testing", 75, vec!["app5", "shared"]),
        ("local", 100, vec!["app6", "shared"]),
    ];

    for (repo_name, _priority, apps) in &repos {
        let repo_path = data_path.join("repos").join(repo_name);
        let apps_dir = repo_path.join("apps");
        let hubs_dir = repo_path.join("hubs");
        fs::create_dir_all(&apps_dir).unwrap();
        fs::create_dir_all(&hubs_dir).unwrap();

        // Create hub config
        let hub_config = serde_json::json!({
            "name": format!("{} Hub", repo_name),
            "provider_type": "test",
            "config": {
                "repo": repo_name
            }
        });
        fs::write(
            hubs_dir.join("test-hub.json"),
            serde_json::to_string_pretty(&hub_config).unwrap(),
        )
        .unwrap();

        // Create app configs
        for app_name in apps {
            let app_config = serde_json::json!({
                "name": app_name.to_string(),
                "metadata": {
                    "repo": repo_name,
                    "priority": _priority,
                    "version": format!("{}-1.0.0", repo_name)
                }
            });
            fs::write(
                apps_dir.join(format!("{}.json", app_name)),
                serde_json::to_string_pretty(&app_config).unwrap(),
            )
            .unwrap();
        }
    }

    // Create AppRegistry and verify multi-repo loading
    let mut registry = AppRegistry::new(data_path).unwrap();

    // Get shared app, should come from highest priority local repo
    let shared_config = registry.get_app_config("shared").unwrap();
    assert_eq!(shared_config.metadata.get("repo").unwrap(), "local");
    assert_eq!(
        shared_config.metadata.get("version").unwrap(),
        "local-1.0.0"
    );

    // Get apps that only exist in specific repos
    let app1_config = registry.get_app_config("app1").unwrap();
    assert_eq!(app1_config.metadata.get("repo").unwrap(), "main");

    let app3_config = registry.get_app_config("app3").unwrap();
    assert_eq!(app3_config.metadata.get("repo").unwrap(), "community");

    // List all available apps
    let available_apps = registry.list_available_apps().unwrap();
    assert!(available_apps.contains(&"app1".to_string()));
    assert!(available_apps.contains(&"app3".to_string()));
    assert!(available_apps.contains(&"app5".to_string()));
    assert!(available_apps.contains(&"shared".to_string()));

    // shared should appear only once (deduped)
    let shared_count = available_apps.iter().filter(|&app| app == "shared").count();
    assert_eq!(shared_count, 1);
}

// Test repository priority override
#[tokio::test]
async fn test_repository_priority_override() {
    let temp_dir = TempDir::new().unwrap();
    let data_path = temp_dir.path();

    let mut repo_manager = RepositoryManager::new(data_path).unwrap();

    // Add three repositories with different priorities
    repo_manager
        .add_repository("low".to_string(), None, 10)
        .unwrap();
    repo_manager
        .add_repository("medium".to_string(), None, 50)
        .unwrap();
    repo_manager
        .add_repository("high".to_string(), None, 100)
        .unwrap();

    // Create same-named config with different content in each repo
    let test_app = "testapp";

    // Low priority repo - complete config
    let low_repo = data_path.join("repos/low/apps");
    fs::create_dir_all(&low_repo).unwrap();
    let low_config = serde_json::json!({
        "name": "Test App",
        "metadata": {
            "version": "1.0.0",
            "description": "Base version from low priority",
            "author": "Original Author",
            "license": "MIT"
        }
    });
    fs::write(
        low_repo.join(format!("{}.json", test_app)),
        serde_json::to_string_pretty(&low_config).unwrap(),
    )
    .unwrap();

    // Medium priority repo - partial override
    let medium_repo = data_path.join("repos/medium/apps");
    fs::create_dir_all(&medium_repo).unwrap();
    let medium_config = serde_json::json!({
        "metadata": {
            "version": "2.0.0",
            "description": "Enhanced by medium priority",
            "enhanced": true
        }
    });
    fs::write(
        medium_repo.join(format!("{}.json", test_app)),
        serde_json::to_string_pretty(&medium_config).unwrap(),
    )
    .unwrap();

    // High priority repo - final override
    let high_repo = data_path.join("repos/high/apps");
    fs::create_dir_all(&high_repo).unwrap();
    let high_config = serde_json::json!({
        "metadata": {
            "version": "3.0.0",
            "priority_override": true
        }
    });
    fs::write(
        high_repo.join(format!("{}.json", test_app)),
        serde_json::to_string_pretty(&high_config).unwrap(),
    )
    .unwrap();

    // Create necessary hub configs
    for repo in ["low", "medium", "high"] {
        let hub_dir = data_path.join(format!("repos/{}/hubs", repo));
        fs::create_dir_all(&hub_dir).unwrap();
    }

    // Load config and verify merge result
    let mut registry = AppRegistry::new(data_path).unwrap();
    let merged_config = registry.get_app_config(test_app).unwrap();

    // Verify merge results
    assert_eq!(merged_config.name, "Test App"); // from low
    assert_eq!(merged_config.metadata.get("version").unwrap(), "3.0.0"); // overridden by high
    assert_eq!(
        merged_config.metadata.get("author").unwrap(),
        "Original Author"
    ); // preserved from low
    assert_eq!(merged_config.metadata.get("license").unwrap(), "MIT"); // preserved from low
    assert_eq!(merged_config.metadata.get("enhanced").unwrap(), true); // from medium
    assert_eq!(
        merged_config.metadata.get("priority_override").unwrap(),
        true
    ); // from high

    // description should be overridden by medium
    assert_eq!(
        merged_config.metadata.get("description").unwrap(),
        "Enhanced by medium priority"
    );
}

// Test disabled repository is ignored
#[tokio::test]
async fn test_disabled_repository_ignored() {
    let temp_dir = TempDir::new().unwrap();
    let data_path = temp_dir.path();

    let mut repo_manager = RepositoryManager::new(data_path).unwrap();

    // Add two repositories
    repo_manager
        .add_repository("active".to_string(), None, 50)
        .unwrap();
    repo_manager
        .add_repository("disabled".to_string(), None, 100)
        .unwrap();

    // Create configs
    for repo_name in ["active", "disabled"] {
        let repo_path = data_path.join(format!("repos/{}/apps", repo_name));
        fs::create_dir_all(&repo_path).unwrap();

        let config = serde_json::json!({
            "name": "App",
            "metadata": {
                "from": repo_name,
                "version": format!("{}-1.0", repo_name)
            }
        });
        fs::write(
            repo_path.join("app.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        )
        .unwrap();
    }

    // Create registry
    let mut registry = AppRegistry::new(data_path).unwrap();

    // First confirm disabled repo has higher priority
    let config = registry.get_app_config("app").unwrap();
    assert_eq!(config.metadata.get("from").unwrap(), "disabled");

    // Disable the disabled repository
    if let Some(repo_mgr) = registry.get_repository_manager() {
        repo_mgr.enable_repository("disabled", false).unwrap();
    }

    // Clear cache and reload
    registry.clear_cache();
    let config = registry.get_app_config("app").unwrap();

    // Should now load from active repository
    assert_eq!(config.metadata.get("from").unwrap(), "active");
    assert_eq!(config.metadata.get("version").unwrap(), "active-1.0");
}

// Test concurrent repository sync
#[tokio::test]
async fn test_concurrent_repository_sync() {
    let temp_dir = TempDir::new().unwrap();
    let data_path = temp_dir.path();

    // Prepare multiple test config files
    let test_configs = vec![
        (
            "repo1",
            r#"{
            "app_config_list": [{
                "base_version": 1,
                "config_version": 1,
                "uuid": "repo1-app1",
                "base_hub_uuid": "hub1",
                "info": {
                    "name": "Repo1 App1",
                    "url": "https://example.com/repo1/app1",
                    "extra_map": {}
                }
            }],
            "hub_config_list": [{
                "base_version": 1,
                "config_version": 1,
                "uuid": "hub1",
                "info": {
                    "hub_name": "Hub1",
                    "hub_icon_url": ""
                },
                "target_check_api": "",
                "api_keywords": [],
                "app_url_templates": []
            }]
        }"#,
        ),
        (
            "repo2",
            r#"{
            "app_config_list": [{
                "base_version": 1,
                "config_version": 1,
                "uuid": "repo2-app1",
                "base_hub_uuid": "hub2",
                "info": {
                    "name": "Repo2 App1",
                    "url": "https://example.com/repo2/app1",
                    "extra_map": {}
                }
            }],
            "hub_config_list": [{
                "base_version": 1,
                "config_version": 1,
                "uuid": "hub2",
                "info": {
                    "hub_name": "Hub2",
                    "hub_icon_url": ""
                },
                "target_check_api": "",
                "api_keywords": [],
                "app_url_templates": []
            }]
        }"#,
        ),
        (
            "repo3",
            r#"{
            "app_config_list": [{
                "base_version": 1,
                "config_version": 1,
                "uuid": "repo3-app1",
                "base_hub_uuid": "hub3",
                "info": {
                    "name": "Repo3 App1",
                    "url": "https://example.com/repo3/app1",
                    "extra_map": {}
                }
            }],
            "hub_config_list": [{
                "base_version": 1,
                "config_version": 1,
                "uuid": "hub3",
                "info": {
                    "hub_name": "Hub3",
                    "hub_icon_url": ""
                },
                "target_check_api": "",
                "api_keywords": [],
                "app_url_templates": []
            }]
        }"#,
        ),
    ];

    // Write test configs to files
    for (repo_name, config_content) in &test_configs {
        let config_file = data_path.join(format!("{}_config.json", repo_name));
        fs::write(&config_file, config_content).unwrap();
    }

    // Simulate concurrent sync
    let start = Instant::now();
    let mut sync_tasks = Vec::new();

    for (repo_name, _) in test_configs.iter() {
        let repo_path = data_path.join(format!("repos/{}", repo_name));
        let config_file = data_path.join(format!("{}_config.json", repo_name));
        let repo_name_clone = repo_name.to_string();

        // Create async task
        let task = tokio::spawn(async move {
            // Simulate sync delay
            tokio::time::sleep(Duration::from_millis(10)).await;

            // Execute sync
            let mut cloud_sync = CloudSync::new();
            let cloud_config = cloud_sync.load_from_file(&config_file).unwrap();

            // Create directories
            fs::create_dir_all(repo_path.join("apps")).unwrap();
            fs::create_dir_all(repo_path.join("hubs")).unwrap();

            // Convert and save
            for hub in &cloud_config.hub_config_list {
                let hub_id = hub.info.hub_name.to_lowercase().replace(' ', "-");
                cloud_sync
                    .uuid_to_name_map
                    .insert(hub.uuid.clone(), hub_id.clone());

                let (_, hub_config) = cloud_sync.convert_hub_item(hub);
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

            repo_name_clone
        });

        sync_tasks.push(task);
    }

    // Wait for all sync tasks to complete
    let results = futures::future::join_all(sync_tasks).await;
    let elapsed = start.elapsed();

    println!(
        "Concurrent sync of {} repositories took: {:?}",
        results.len(),
        elapsed
    );

    // Verify all syncs succeeded
    for result in results {
        let repo_name = result.unwrap();
        let repo_path = data_path.join(format!("repos/{}", repo_name));

        // Verify files exist
        assert!(repo_path.join("apps").exists());
        assert!(repo_path.join("hubs").exists());

        // Verify specific files
        let apps_dir = repo_path.join("apps");
        let entries: Vec<_> = fs::read_dir(apps_dir).unwrap().collect();
        assert_eq!(entries.len(), 1); // Each repo has one app
    }

    // Verify concurrent execution is faster than serial
    assert!(elapsed < Duration::from_millis(100)); // Should be much less than 3 * 10ms
}

// Test conflict resolution across repositories
#[tokio::test]
async fn test_conflict_resolution_across_repos() {
    let temp_dir = TempDir::new().unwrap();
    let data_path = temp_dir.path();

    let mut repo_manager = RepositoryManager::new(data_path).unwrap();

    // Create three repos with same-named but different content apps
    let repos = vec![
        ("upstream", 0, "1.0.0", "Original upstream version"),
        ("community", 50, "1.5.0", "Community enhanced version"),
        ("experimental", 75, "2.0.0-beta", "Experimental features"),
    ];

    for (repo_name, priority, version, description) in &repos {
        repo_manager
            .add_repository(repo_name.to_string(), None, *priority)
            .unwrap();

        let repo_path = data_path.join(format!("repos/{}", repo_name));
        let apps_dir = repo_path.join("apps");
        let hubs_dir = repo_path.join("hubs");
        fs::create_dir_all(&apps_dir).unwrap();
        fs::create_dir_all(&hubs_dir).unwrap();

        // Create different versions of same-named app
        let app_config = serde_json::json!({
            "name": "ConflictApp",
            "metadata": {
                "version": version,
                "description": description,
                "source": repo_name,
                "priority": priority,
                // Repo-specific fields
                format!("{}_specific", repo_name): true,
                format!("{}_feature", repo_name): format!("Feature from {}", repo_name)
            }
        });

        fs::write(
            apps_dir.join("conflictapp.json"),
            serde_json::to_string_pretty(&app_config).unwrap(),
        )
        .unwrap();

        // Create hub config
        let hub_config = serde_json::json!({
            "name": "TestHub",
            "provider_type": "test",
            "config": {}
        });
        fs::write(
            hubs_dir.join("testhub.json"),
            serde_json::to_string_pretty(&hub_config).unwrap(),
        )
        .unwrap();
    }

    // Load and verify conflict resolution
    let mut registry = AppRegistry::new(data_path).unwrap();
    let merged = registry.get_app_config("conflictapp").unwrap();

    // Basic info should come from lowest priority (preserve original name)
    assert_eq!(merged.name, "ConflictApp");

    // Version should be overridden by highest priority
    assert_eq!(merged.metadata.get("version").unwrap(), "2.0.0-beta");

    // All repo-specific fields should be preserved
    assert_eq!(merged.metadata.get("upstream_specific").unwrap(), true);
    assert_eq!(merged.metadata.get("community_specific").unwrap(), true);
    assert_eq!(merged.metadata.get("experimental_specific").unwrap(), true);

    // Verify all feature fields exist
    assert!(merged.metadata.contains_key("upstream_feature"));
    assert!(merged.metadata.contains_key("community_feature"));
    assert!(merged.metadata.contains_key("experimental_feature"));
}

// Test dynamic priority changes
#[tokio::test]
async fn test_dynamic_priority_change() {
    let temp_dir = TempDir::new().unwrap();
    let data_path = temp_dir.path();

    let mut repo_manager = RepositoryManager::new(data_path).unwrap();

    // Add two repositories with same initial priority
    repo_manager
        .add_repository("repo_a".to_string(), None, 50)
        .unwrap();
    repo_manager
        .add_repository("repo_b".to_string(), None, 50)
        .unwrap();

    // Create configs
    for (repo, value) in [("repo_a", "A"), ("repo_b", "B")] {
        let repo_path = data_path.join(format!("repos/{}/apps", repo));
        fs::create_dir_all(&repo_path).unwrap();

        let config = serde_json::json!({
            "name": "TestApp",
            "metadata": {
                "value": value,
                "repo": repo
            }
        });
        fs::write(
            repo_path.join("testapp.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        )
        .unwrap();
    }

    let mut registry = AppRegistry::new(data_path).unwrap();

    // Initial state - when priorities are equal, order is undefined
    let _config = registry.get_app_config("testapp").unwrap();
    // Result depends on implementation details when priorities are equal

    // Change repo_b to higher priority
    if let Some(repo_mgr) = registry.get_repository_manager() {
        repo_mgr.set_repository_priority("repo_b", 100).unwrap();
    }

    // Clear cache and reload
    registry.clear_cache();
    let config = registry.get_app_config("testapp").unwrap();

    // Should now use repo_b config (higher priority)
    assert_eq!(config.metadata.get("value").unwrap(), "B");
    assert_eq!(config.metadata.get("repo").unwrap(), "repo_b");

    // Change again, make repo_a highest priority
    if let Some(repo_mgr) = registry.get_repository_manager() {
        repo_mgr.set_repository_priority("repo_a", 150).unwrap();
    }

    registry.clear_cache();
    let config = registry.get_app_config("testapp").unwrap();

    // Should now use repo_a config
    assert_eq!(config.metadata.get("value").unwrap(), "A");
    assert_eq!(config.metadata.get("repo").unwrap(), "repo_a");
}

// Test behavior after repository removal
#[tokio::test]
async fn test_repository_removal() {
    let temp_dir = TempDir::new().unwrap();
    let data_path = temp_dir.path();

    let mut repo_manager = RepositoryManager::new(data_path).unwrap();

    // Add three repositories
    repo_manager
        .add_repository("keep1".to_string(), None, 10)
        .unwrap();
    repo_manager
        .add_repository("remove".to_string(), None, 50)
        .unwrap();
    repo_manager
        .add_repository("keep2".to_string(), None, 30)
        .unwrap();

    // Create test data
    for repo in ["keep1", "remove", "keep2"] {
        let repo_path = data_path.join(format!("repos/{}/apps", repo));
        fs::create_dir_all(&repo_path).unwrap();

        // Each repo has a unique app
        let unique_config = serde_json::json!({
            "name": format!("{} App", repo),
            "metadata": {
                "repo": repo
            }
        });
        fs::write(
            repo_path.join(format!("{}_app.json", repo)),
            serde_json::to_string_pretty(&unique_config).unwrap(),
        )
        .unwrap();

        // All repos have shared_app
        let shared_config = serde_json::json!({
            "name": "Shared App",
            "metadata": {
                "repo": repo,
                "priority": if repo == "remove" { 50 } else { 10 }
            }
        });
        fs::write(
            repo_path.join("shared_app.json"),
            serde_json::to_string_pretty(&shared_config).unwrap(),
        )
        .unwrap();
    }

    let mut registry = AppRegistry::new(data_path).unwrap();

    // Verify initial state
    let shared = registry.get_app_config("shared_app").unwrap();
    assert_eq!(shared.metadata.get("repo").unwrap(), "remove"); // Highest priority

    // Verify remove_app exists
    assert!(registry.get_app_config("remove_app").is_ok());

    // Remove the 'remove' repository
    if let Some(repo_mgr) = registry.get_repository_manager() {
        assert!(repo_mgr.remove_repository("remove").unwrap());
    }

    // Clear cache
    registry.clear_cache();

    // shared_app should fall back to next highest priority repo
    let shared = registry.get_app_config("shared_app").unwrap();
    assert_eq!(shared.metadata.get("repo").unwrap(), "keep2");

    // remove_app should no longer exist
    assert!(registry.get_app_config("remove_app").is_err());

    // Apps from other repos should still exist
    assert!(registry.get_app_config("keep1_app").is_ok());
    assert!(registry.get_app_config("keep2_app").is_ok());
}

// Performance test: many repositories
#[tokio::test]
async fn test_many_repositories_performance() {
    let temp_dir = TempDir::new().unwrap();
    let data_path = temp_dir.path();

    let mut repo_manager = RepositoryManager::new(data_path).unwrap();

    let num_repos = 20usize;
    let apps_per_repo = 50usize;

    // Add multiple repositories
    let start = Instant::now();
    for i in 0..num_repos {
        repo_manager
            .add_repository(format!("repo_{:02}", i), None, (i * 5) as i32)
            .unwrap();

        let repo_path = data_path.join(format!("repos/repo_{:02}", i));
        let apps_dir = repo_path.join("apps");
        fs::create_dir_all(&apps_dir).unwrap();

        // Create multiple apps per repository
        for j in 0..apps_per_repo {
            let config = serde_json::json!({
                "name": format!("App_{:02}_{:03}", i, j),
                "metadata": {
                    "repo": format!("repo_{:02}", i),
                    "index": j
                }
            });
            fs::write(
                apps_dir.join(format!("app_{:02}_{:03}.json", i, j)),
                serde_json::to_string_pretty(&config).unwrap(),
            )
            .unwrap();
        }
    }

    let setup_time = start.elapsed();
    println!(
        "Created {} repositories with {} total apps in: {:?}",
        num_repos,
        num_repos * apps_per_repo,
        setup_time
    );

    // Test performance of listing all apps
    let mut registry = AppRegistry::new(data_path).unwrap();

    let start = Instant::now();
    let all_apps = registry.list_available_apps().unwrap();
    let list_time = start.elapsed();

    assert_eq!(all_apps.len(), num_repos * apps_per_repo);
    println!("Listed {} apps in: {:?}", all_apps.len(), list_time);
    assert!(list_time < Duration::from_millis(100)); // Should be fast

    // Test performance of loading configs
    let start = Instant::now();
    for i in 0..10 {
        let app_name = format!("app_{:02}_{:03}", i, 0);
        let _ = registry.get_app_config(&app_name).unwrap();
    }
    let load_time = start.elapsed();

    println!("Loaded 10 app configs in: {:?}", load_time);
    assert!(load_time < Duration::from_millis(50)); // Should be fast
}
