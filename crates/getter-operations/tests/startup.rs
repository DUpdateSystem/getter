#![cfg(feature = "lua")]

use getter_core::autogen::{InstalledInventory, InstalledInventoryItem};
use getter_core::repository::RepositoryMetadata;
use getter_core::{RepositoryId, RepositoryPriority};
use getter_operations::startup::{startup, UpdateStatus};
use getter_storage::{
    CacheDb, MainDb, ProviderResponseUpsert, StoredPackageResolution, TrackedPackageUpsert,
};
use serde_json::json;
use std::fs;

#[test]
fn startup_bootstraps_idempotently_and_joins_static_updates() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path();

    let first = startup(data_dir, InstalledInventory::new(vec![])).unwrap();
    let second = startup(data_dir, InstalledInventory::new(vec![])).unwrap();
    assert_eq!(
        first.bootstrap.lifecycle,
        getter_operations::startup::BootstrapLifecycle::Initialized
    );
    assert_eq!(
        second.bootstrap.lifecycle,
        getter_operations::startup::BootstrapLifecycle::AlreadyInitialized
    );
    assert!(first.bootstrap.main_db.created_this_call);
    assert!(!second.bootstrap.main_db.created_this_call);
    assert_eq!(
        first.bootstrap.main_db.contract_version,
        getter_storage::MAIN_STORAGE_CONTRACT_VERSION
    );
    assert_eq!(
        first.bootstrap.cache_db.contract_version,
        getter_storage::CACHE_STORAGE_CONTRACT_VERSION
    );
    let metadata = fs::read_to_string(data_dir.join("repo/metadata.jsonc")).unwrap();
    assert!(metadata.contains("// \"generated_repository\": \"autogen\""));
    assert!(metadata.contains("\"official\": 0"));
    assert!(first
        .bootstrap
        .diagnostics
        .iter()
        .any(|diagnostic| { diagnostic.code == "storage.main_migrations_applied" }));
    assert!(first
        .bootstrap
        .diagnostics
        .iter()
        .any(|diagnostic| { diagnostic.code == "storage.cache_migrations_applied" }));
    assert!(second.bootstrap.diagnostics.is_empty());

    let repo = data_dir.join("repo/official");
    let package = repo.join("android/org.fdroid.fdroid");
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("Manifest"), "").unwrap();
    fs::write(
        package.join("metadata.jsonc"),
        r#"{
      "type":"android:app", "display_name":"F-Droid",
      "android":{"package_name":"org.fdroid.fdroid"},
      "lua":{"9999.lua":{"permission":["allow_free_network"]}}
    }"#,
    )
    .unwrap();
    fs::write(
        package.join("9999.lua"),
        r#"#!/bin/upa-lua v1
return package_version { updates = {
  { version="1.1.0", artifacts={{name="fdroid.apk",url="https://example.invalid/1.1.apk"}} },
  { version="1.2.0", artifacts={{name="fdroid.apk",url="https://example.invalid/1.2.apk"}} },
} }"#,
    )
    .unwrap();

    let db = MainDb::open(data_dir.join("main.db")).unwrap();
    db.upsert_repository(
        &RepositoryMetadata {
            id: RepositoryId::new("official").unwrap(),
            name: "Official".into(),
            priority: RepositoryPriority::DEFAULT,
            api_version: "v1".into(),
        },
        Some(&repo),
        None,
    )
    .unwrap();
    let package_id = "android/org.fdroid.fdroid".parse().unwrap();
    db.upsert_tracked_package(&TrackedPackageUpsert {
        package_id,
        enabled: true,
        favorite: true,
        pin_version: Some("1.1.0".into()),
        repository_id: Some(RepositoryId::new("official").unwrap()),
        package_resolution: StoredPackageResolution::OfficialRepositoryPackage,
    })
    .unwrap();

    let snapshot = startup(
        data_dir,
        InstalledInventory::new(vec![InstalledInventoryItem::AndroidPackage {
            package_name: "org.fdroid.fdroid".into(),
            label: Some("F-Droid installed".into()),
            version_name: Some("1.0.0".into()),
            version_code: Some(100),
        }]),
    )
    .unwrap();
    let app = &snapshot.apps[0];
    assert_eq!(app.name.as_deref(), Some("F-Droid"));
    assert_eq!(app.installed_version.as_deref(), Some("1.0.0"));
    assert_eq!(app.effective_installed_version.as_deref(), Some("1.1.0"));
    assert_eq!(app.latest_version.as_deref(), Some("1.2.0"));
    assert_eq!(app.update_status, UpdateStatus::Available);
    assert!(app.warning.free_network);
    assert_eq!(snapshot.update_count, 1);
}

#[test]
fn startup_resolves_official_package_without_repository_to_highest_priority_covering_repo() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path();
    startup(data_dir, InstalledInventory::new(vec![])).unwrap();

    let db = MainDb::open(data_dir.join("main.db")).unwrap();
    for (id, priority, contains_package, latest_version) in [
        ("highest-without-package", 100, false, "9.9.9"),
        ("highest-covering", 50, true, "2.0.0"),
        ("lower-covering", 10, true, "1.5.0"),
    ] {
        let repo = data_dir.join("repo").join(id);
        fs::create_dir_all(&repo).unwrap();
        if contains_package {
            let package = repo.join("android/org.example.app");
            fs::create_dir_all(&package).unwrap();
            fs::write(package.join("Manifest"), "").unwrap();
            fs::write(
                package.join("metadata.jsonc"),
                r#"{
                  "type":"android:app", "display_name":"Example",
                  "android":{"package_name":"org.example.app"}
                }"#,
            )
            .unwrap();
            fs::write(
                package.join("9999.lua"),
                format!(
                    r#"#!/bin/upa-lua v1
return package_version {{ updates = {{{{ version="{latest_version}", artifacts={{{{name="app.apk",url="https://example.invalid/app.apk"}}}} }}}} }}"#
                ),
            )
            .unwrap();
        }
        db.upsert_repository(
            &RepositoryMetadata {
                id: RepositoryId::new(id).unwrap(),
                name: id.into(),
                priority: RepositoryPriority::new(priority),
                api_version: "v1".into(),
            },
            Some(&repo),
            None,
        )
        .unwrap();
    }
    db.upsert_tracked_package(&TrackedPackageUpsert {
        package_id: "android/org.example.app".parse().unwrap(),
        enabled: true,
        favorite: false,
        pin_version: None,
        repository_id: None,
        package_resolution: StoredPackageResolution::OfficialRepositoryPackage,
    })
    .unwrap();

    let snapshot = startup(
        data_dir,
        InstalledInventory::new(vec![InstalledInventoryItem::AndroidPackage {
            package_name: "org.example.app".into(),
            label: Some("Installed Example".into()),
            version_name: Some("1.0.0".into()),
            version_code: Some(1),
        }]),
    )
    .unwrap();

    assert_eq!(snapshot.apps[0].name.as_deref(), Some("Example"));
    assert_eq!(snapshot.apps[0].latest_version.as_deref(), Some("2.0.0"));
    assert_eq!(snapshot.apps[0].update_status, UpdateStatus::Available);
    assert!(!snapshot
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "startup.repository_missing"));
}

#[test]
fn startup_reports_initialized_when_an_existing_valid_database_needs_a_migration() {
    let temp = tempfile::tempdir().unwrap();
    startup(temp.path(), InstalledInventory::new(vec![])).unwrap();

    let main_db = rusqlite::Connection::open(temp.path().join("main.db")).unwrap();
    main_db
        .execute(
            "DELETE FROM schema_migrations WHERE id = 'main-task-v1'",
            [],
        )
        .unwrap();

    let migrated = startup(temp.path(), InstalledInventory::new(vec![])).unwrap();

    assert_eq!(
        migrated.bootstrap.lifecycle,
        getter_operations::startup::BootstrapLifecycle::Initialized
    );
    assert!(!migrated.bootstrap.main_db.created_this_call);
    assert!(migrated
        .bootstrap
        .diagnostics
        .iter()
        .any(|diagnostic| { diagnostic.code == "storage.main_migrations_applied" }));
}

#[test]
fn startup_keeps_missing_facts_nullable_with_stable_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    startup(temp.path(), InstalledInventory::new(vec![])).unwrap();
    let db = MainDb::open(temp.path().join("main.db")).unwrap();
    let package_id = "android/org.missing.app".parse().unwrap();
    db.upsert_tracked_package(&TrackedPackageUpsert {
        package_id,
        enabled: true,
        favorite: false,
        pin_version: None,
        repository_id: None,
        package_resolution: StoredPackageResolution::MissingPackageDefinition,
    })
    .unwrap();

    let snapshot = startup(temp.path(), InstalledInventory::new(vec![])).unwrap();
    let app = &snapshot.apps[0];
    assert!(app.repository_id.is_none());
    assert!(app.installed_version.is_none());
    assert!(app.latest_version.is_none());
    let codes: Vec<_> = app.diagnostics.iter().map(|d| d.code.as_str()).collect();
    assert!(codes.contains(&"startup.package_missing"));
    assert!(codes.contains(&"startup.installed_inventory_missing"));
    assert_eq!(app.update_status, UpdateStatus::NotInstalled);
    assert_eq!(snapshot.update_count, 0);
}

#[test]
fn bootstrap_reports_layout_created_even_when_databases_already_exist() {
    let temp = tempfile::tempdir().unwrap();
    MainDb::open(temp.path().join("main.db")).unwrap();
    CacheDb::open(temp.path().join("cache.db")).unwrap();

    let snapshot = startup(temp.path(), InstalledInventory::new(vec![])).unwrap();
    assert_eq!(
        snapshot.bootstrap.lifecycle,
        getter_operations::startup::BootstrapLifecycle::Initialized
    );
    assert!(!snapshot.bootstrap.main_db.created_this_call);
    assert!(!snapshot.bootstrap.cache_db.created_this_call);
    assert!(snapshot.bootstrap.repo.join("metadata.jsonc").is_file());
    assert!(snapshot.bootstrap.rc.is_dir());
}

#[test]
fn startup_reads_warm_github_cache_without_transport_and_normalizes_updates() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path();
    startup(data_dir, InstalledInventory::new(vec![])).unwrap();
    let repo = data_dir.join("repo/official");
    let package = repo.join("android/com.example.app");
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("Manifest"), "").unwrap();
    fs::write(
        package.join("metadata.jsonc"),
        r#"{
      "type":"android:app", "display_name":"Example",
      "android":{"package_name":"com.example.app"},
      "lua":{"9999.lua":{"permission":["allow_free_network"]}}
    }"#,
    )
    .unwrap();
    fs::write(
        package.join("9999.lua"),
        r#"#!/bin/upa-lua v1
local result = getter.provider.github.release_candidates {
  owner = "example", repo = "app", asset = { include = "app.apk" },
}
return package_version { name="Example", source_priority={"github"}, updates=result.candidates }
"#,
    )
    .unwrap();
    let metadata = RepositoryMetadata {
        id: RepositoryId::new("official").unwrap(),
        name: "Official".into(),
        priority: RepositoryPriority::new(0),
        api_version: "v1".into(),
    };
    MainDb::open(data_dir.join("main.db"))
        .unwrap()
        .upsert_repository(&metadata, Some(&repo), None)
        .unwrap();
    let package_id = "android/com.example.app".parse().unwrap();
    MainDb::open(data_dir.join("main.db"))
        .unwrap()
        .upsert_tracked_package(&TrackedPackageUpsert {
            package_id,
            enabled: true,
            favorite: false,
            pin_version: None,
            repository_id: Some(metadata.id),
            package_resolution: StoredPackageResolution::OfficialRepositoryPackage,
        })
        .unwrap();
    let config = getter_operations::github_releases::GithubReleaseConfig {
        api_base_url: getter_operations::github_releases::DEFAULT_GITHUB_API_BASE_URL.into(),
        owner: "example".into(),
        repo: "app".into(),
    };
    CacheDb::open(data_dir.join("cache.db")).unwrap().upsert_provider_response(&ProviderResponseUpsert {
        cache_key: config.cache_key(), provider: "github".into(),
        response_json: json!([{"tag_name":"2.0.0","name":"2.0.0","draft":false,"prerelease":false,
          "published_at":"2026-01-01T00:00:00Z","assets":[{"name":"app.apk","browser_download_url":"https://example/app.apk","size":1}]}]),
        source_response_sha512: vec![], provenance_schema_version: None, freshness_json: json!({}),
    }).unwrap();

    let snapshot = startup(
        data_dir,
        InstalledInventory::new(vec![InstalledInventoryItem::AndroidPackage {
            package_name: "com.example.app".into(),
            label: None,
            version_name: Some("1.0.0".into()),
            version_code: None,
        }]),
    )
    .unwrap();
    assert_eq!(snapshot.apps[0].latest_version.as_deref(), Some("2.0.0"));
    assert_eq!(snapshot.apps[0].update_status, UpdateStatus::Available);
    assert_eq!(snapshot.update_count, 1);
}
