use getter_cli::{run, ExitCode};
use getter_core::repository::RepositoryMetadata;
use getter_core::{RepositoryId, RepositoryPriority};
use getter_storage::{MainDb, StoredPackageResolution, TrackedPackageUpsert};
use serde_json::{json, Value};

#[test]
fn startup_command_proves_shared_getter_snapshot_with_empty_inventory_default() {
    let temp = tempfile::tempdir().unwrap();
    let output = run([
        "getter",
        "--data-dir",
        temp.path().to_str().unwrap(),
        "startup",
    ]);
    assert_eq!(output.exit_code, ExitCode::Success, "{}", output.stdout);
    let envelope: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(envelope["command"], "startup");
    assert_eq!(envelope["data"]["format"], "getter-startup-snapshot");
    assert_eq!(envelope["data"]["version"], 1);
    assert_eq!(envelope["data"]["bootstrap"]["lifecycle"], "initialized");
    assert_eq!(envelope["data"]["apps"], serde_json::json!([]));
}

#[test]
fn startup_command_uses_inventory_file_to_join_installed_version() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo/official");
    let package = repo.join("android/com.example");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("Manifest"), "").unwrap();
    std::fs::write(
        package.join("metadata.jsonc"),
        r#"{
          "type":"android:app",
          "display_name":"Example",
          "android":{"package_name":"com.example"}
        }"#,
    )
    .unwrap();
    std::fs::write(
        package.join("1.lua"),
        r#"#!/bin/upa-lua v1
return package_version { updates = { { version="2.4.0", artifacts={{name="app.apk",url="https://example.invalid/app.apk"}} } } }"#,
    )
    .unwrap();
    let package_id = "android/com.example".parse().unwrap();
    let repository_id = RepositoryId::new("official").unwrap();
    let db = MainDb::open(temp.path().join("main.db")).unwrap();
    db.upsert_repository(
        &RepositoryMetadata {
            id: repository_id.clone(),
            name: "Official".to_owned(),
            priority: RepositoryPriority::DEFAULT,
            api_version: "v1".to_owned(),
        },
        Some(&repo),
        None,
    )
    .unwrap();
    db.upsert_tracked_package(&TrackedPackageUpsert {
        package_id,
        enabled: true,
        favorite: false,
        pin_version: None,
        repository_id: Some(repository_id),
        package_resolution: StoredPackageResolution::OfficialRepositoryPackage,
    })
    .unwrap();
    let inventory_path = temp.path().join("inventory.json");
    std::fs::write(
        &inventory_path,
        serde_json::to_vec(&json!({
            "format": "upgradeall-installed-inventory",
            "version": 1,
            "items": [{
                "kind": "android_package",
                "package_name": "com.example",
                "label": "Example",
                "version_name": "2.4.0",
                "version_code": 24
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let output = run([
        "getter",
        "--data-dir",
        temp.path().to_str().unwrap(),
        "startup",
        "--inventory",
        inventory_path.to_str().unwrap(),
    ]);

    assert_eq!(output.exit_code, ExitCode::Success, "{}", output.stdout);
    let envelope: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(
        envelope["data"]["apps"][0]["installed_version"], "2.4.0",
        "{}",
        output.stdout
    );
    assert_eq!(envelope["data"]["apps"][0]["update_status"], "up_to_date");
}
