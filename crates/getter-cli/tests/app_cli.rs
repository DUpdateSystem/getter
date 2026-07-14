use getter_cli::{run, ExitCode};
use getter_core::repository::RepositoryMetadata;
use getter_core::{RepositoryId, RepositoryPriority};
use getter_storage::{MainDb, StoredPackageResolution, TrackedPackageUpsert};
use serde_json::{json, Value};
use std::fs;

fn fixture(latest: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    run([
        "getter",
        "--data-dir",
        temp.path().to_str().unwrap(),
        "init",
    ]);
    let package = temp.path().join("repo/official/android/com.example");
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("Manifest"), "").unwrap();
    fs::write(package.join("metadata.jsonc"), r#"{"type":"android:app","display_name":"Example","android":{"package_name":"com.example"}}"#).unwrap();
    fs::write(package.join("1.lua"), format!(r#"#!/bin/upa-lua v1
return package_version {{ updates = {{ {{ version="{latest}", artifacts={{{{name="app.apk",url="https://example.invalid/app.apk"}}}} }} }} }}"#)).unwrap();
    let db = MainDb::open(temp.path().join("main.db")).unwrap();
    db.upsert_repository(
        &RepositoryMetadata {
            id: RepositoryId::new("official").unwrap(),
            name: "Official".into(),
            priority: RepositoryPriority::DEFAULT,
            api_version: "v1".into(),
        },
        Some(&temp.path().join("repo/official")),
        None,
    )
    .unwrap();
    db.upsert_tracked_package(&TrackedPackageUpsert {
        package_id: "android/com.example".parse().unwrap(),
        enabled: true,
        favorite: false,
        pin_version: None,
        repository_id: None,
        package_resolution: StoredPackageResolution::OfficialRepositoryPackage,
    })
    .unwrap();
    fs::write(temp.path().join("inventory.json"), json!({"format":"upgradeall-installed-inventory","version":1,"items":[{"kind":"android_package","package_name":"com.example","version_name":"1"}]}).to_string()).unwrap();
    temp
}

#[test]
fn app_show_is_normal_cache_only_detail_command() {
    let temp = fixture("2");
    let output = run([
        "getter",
        "--data-dir",
        temp.path().to_str().unwrap(),
        "app",
        "show",
        "android/com.example",
        "--inventory",
        temp.path().join("inventory.json").to_str().unwrap(),
    ]);
    assert_eq!(output.exit_code, ExitCode::Success, "{}", output.stdout);
    let envelope: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(envelope["command"], "app show");
    assert_eq!(envelope["data"]["package_id"], "android/com.example");
    assert_eq!(envelope["data"]["update_status"], "available");
    assert!(envelope["data"].get("action").is_none());
}

#[test]
fn app_check_returns_current_process_action_for_update() {
    let temp = fixture("2");
    let output = run([
        "getter",
        "--data-dir",
        temp.path().to_str().unwrap(),
        "app",
        "check",
        "android/com.example",
        "--inventory",
        temp.path().join("inventory.json").to_str().unwrap(),
    ]);
    assert_eq!(output.exit_code, ExitCode::Success, "{}", output.stdout);
    let envelope: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(envelope["command"], "app check");
    assert!(envelope["data"]["action"]["action_id"].is_string());
}

#[test]
fn app_show_without_inventory_reports_not_installed_without_inventing_version() {
    let temp = fixture("2");
    let output = run([
        "getter",
        "--data-dir",
        temp.path().to_str().unwrap(),
        "app",
        "show",
        "android/com.example",
    ]);
    let body: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(output.exit_code, ExitCode::Success);
    assert_eq!(body["data"]["update_status"], "not_installed");
    assert_eq!(body["data"]["installed_version"], Value::Null);
}

#[test]
fn app_commands_preserve_stable_inventory_and_runtime_error_envelopes() {
    let temp = fixture("2");
    fs::write(
        temp.path().join("invalid-inventory.json"),
        json!({"format":"wrong","version":1,"items":[]}).to_string(),
    )
    .unwrap();
    let invalid_inventory = run([
        "getter",
        "--data-dir",
        temp.path().to_str().unwrap(),
        "app",
        "show",
        "android/com.example",
        "--inventory",
        temp.path().join("invalid-inventory.json").to_str().unwrap(),
    ]);
    let body: Value = serde_json::from_str(&invalid_inventory.stdout).unwrap();
    assert_eq!(invalid_inventory.exit_code, ExitCode::GenericFailure);
    assert_eq!(body["error"]["code"], "inventory.invalid");
    assert_eq!(body["error"]["message"], "Installed inventory is invalid");

    fs::write(
        temp.path().join("repo/official/android/com.example/1.lua"),
        "this is not valid lua",
    )
    .unwrap();
    let runtime_failure = run([
        "getter",
        "--data-dir",
        temp.path().to_str().unwrap(),
        "app",
        "check",
        "android/com.example",
        "--inventory",
        temp.path().join("inventory.json").to_str().unwrap(),
    ]);
    let body: Value = serde_json::from_str(&runtime_failure.stdout).unwrap();
    assert_eq!(runtime_failure.exit_code, ExitCode::GenericFailure);
    assert_eq!(body["error"]["code"], "package.eval_error");
    assert_ne!(body["error"]["code"], "storage.error");
}

#[test]
fn app_show_returns_untracked_and_disabled_error_envelopes() {
    let temp = fixture("2");
    let untracked = run([
        "getter",
        "--data-dir",
        temp.path().to_str().unwrap(),
        "app",
        "show",
        "android/not.tracked",
    ]);
    let body: Value = serde_json::from_str(&untracked.stdout).unwrap();
    assert_eq!(untracked.exit_code, ExitCode::GenericFailure);
    assert_eq!(body["error"]["code"], "app.untracked");

    let db = MainDb::open(temp.path().join("main.db")).unwrap();
    db.upsert_tracked_package(&TrackedPackageUpsert {
        package_id: "android/com.example".parse().unwrap(),
        enabled: false,
        favorite: false,
        pin_version: None,
        repository_id: None,
        package_resolution: StoredPackageResolution::OfficialRepositoryPackage,
    })
    .unwrap();
    let disabled = run([
        "getter",
        "--data-dir",
        temp.path().to_str().unwrap(),
        "app",
        "show",
        "android/com.example",
    ]);
    let body: Value = serde_json::from_str(&disabled.stdout).unwrap();
    assert_eq!(disabled.exit_code, ExitCode::GenericFailure);
    assert_eq!(body["error"]["code"], "app.disabled");
}

#[test]
fn app_commands_reject_product_transport_controls() {
    let temp = fixture("2");
    let output = run([
        "getter",
        "--data-dir",
        temp.path().to_str().unwrap(),
        "app",
        "check",
        "android/com.example",
        "--endpoint",
        "https://example.invalid",
    ]);
    assert_eq!(output.exit_code, ExitCode::Usage);
}
