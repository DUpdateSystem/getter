#![cfg(feature = "lua")]

use getter_core::autogen::{InstalledInventory, InstalledInventoryItem};
use getter_core::repository::RepositoryMetadata;
use getter_core::runtime::GetterRuntime;
use getter_core::{RepositoryId, RepositoryPriority};
use getter_operations::app::{
    check_app, check_app_with_github_transport, inspect_app, AppOperationError,
};
use getter_operations::github_releases::{
    GithubReleaseTransport, GithubReleaseTransportError, GithubReleaseTransportRequest,
    GithubReleaseTransportResponse,
};
use getter_storage::{
    CacheDb, MainDb, ProviderResponseUpsert, StoredPackageResolution, TrackedPackageUpsert,
};
use serde_json::json;
use std::cell::RefCell;
use std::fs;
use std::rc::Rc;

fn write_package(root: &std::path::Path, name: &str, latest: &str) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("Manifest"), "").unwrap();
    fs::write(root.join("metadata.jsonc"), format!(r#"{{"type":"android:app","display_name":"{name}","android":{{"package_name":"com.example"}}}}"#)).unwrap();
    fs::write(root.join("1.lua"), format!(r#"#!/bin/upa-lua v1
return package_version {{ updates = {{ {{ version="{latest}", artifacts={{{{name="app.apk",url="https://example.invalid/app.apk"}}}} }} }} }}"#)).unwrap();
}

fn register(data: &std::path::Path, repo: &str, priority: i32, explicit: bool, enabled: bool) {
    let db = MainDb::open(data.join("main.db")).unwrap();
    db.upsert_repository(
        &RepositoryMetadata {
            id: RepositoryId::new(repo).unwrap(),
            name: repo.into(),
            priority: RepositoryPriority::new(priority),
            api_version: "v1".into(),
        },
        Some(&data.join("repo").join(repo)),
        None,
    )
    .unwrap();
    db.upsert_tracked_package(&TrackedPackageUpsert {
        package_id: "android/com.example".parse().unwrap(),
        package_resolution: StoredPackageResolution::OfficialRepositoryPackage,
        repository_id: explicit.then(|| RepositoryId::new(repo).unwrap()),
        enabled,
        favorite: false,
        pin_version: None,
    })
    .unwrap();
}

const GITHUB_RELEASES: &str = r#"[{"tag_name":"2","draft":false,"prerelease":false,"assets":[{"name":"example.apk","browser_download_url":"https://example.invalid/example-v2.apk"}]}]"#;

struct MockGithubTransport {
    requests: RefCell<usize>,
}

struct FailingGithubTransport;

impl GithubReleaseTransport for FailingGithubTransport {
    fn fetch_releases(
        &self,
        _request: &GithubReleaseTransportRequest<'_>,
    ) -> Result<GithubReleaseTransportResponse, GithubReleaseTransportError> {
        Err(GithubReleaseTransportError::Transport("offline".into()))
    }
}

impl GithubReleaseTransport for MockGithubTransport {
    fn fetch_releases(
        &self,
        _request: &GithubReleaseTransportRequest<'_>,
    ) -> Result<GithubReleaseTransportResponse, GithubReleaseTransportError> {
        *self.requests.borrow_mut() += 1;
        Ok(GithubReleaseTransportResponse {
            body: GITHUB_RELEASES.into(),
            etag: None,
            last_modified: None,
        })
    }
}

fn write_github_package(root: &std::path::Path) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("Manifest"), "14be2aa0ad6385271f005345095584d79556562f96cdcb7c25f500f5f35246fccacb3bbfbc38f8df48c512cee853928aad8b4fb7138f4bf9c8b5835b7e46838a github\n").unwrap();
    fs::write(
        root.join("metadata.jsonc"),
        r#"{"type":"android:app","android":{"package_name":"com.example"}}"#,
    )
    .unwrap();
    fs::write(
        root.join("9999.lua"),
        r#"#!/bin/upa-lua v1
local github_android = require("luaclass.github_android_apk")
return github_android.package {
  name = "Example",
  android_package = "com.example",
  owner = "owner",
  repo = "repo",
  asset = { include = "[.]apk$" },
}"#,
    )
    .unwrap();
}

fn inventory(version: &str) -> InstalledInventory {
    InstalledInventory::new(vec![InstalledInventoryItem::AndroidPackage {
        package_name: "com.example".into(),
        label: None,
        version_name: Some(version.into()),
        version_code: None,
    }])
}

#[test]
fn inspect_uses_repository_priority_and_is_cache_only() {
    let temp = tempfile::tempdir().unwrap();
    getter_operations::startup::bootstrap_data_dir(temp.path()).unwrap();
    write_package(
        &temp.path().join("repo/low/android/com.example"),
        "Low",
        "2",
    );
    write_package(
        &temp.path().join("repo/high/android/com.example"),
        "High",
        "3",
    );
    register(temp.path(), "low", 0, false, true);
    let db = MainDb::open(temp.path().join("main.db")).unwrap();
    db.upsert_repository(
        &RepositoryMetadata {
            id: RepositoryId::new("high").unwrap(),
            name: "high".into(),
            priority: RepositoryPriority::new(10),
            api_version: "v1".into(),
        },
        Some(&temp.path().join("repo/high")),
        None,
    )
    .unwrap();

    let app = inspect_app(
        temp.path(),
        inventory("1"),
        &"android/com.example".parse().unwrap(),
    )
    .unwrap();
    assert_eq!(app.repository_id.as_deref(), Some("high"));
    assert_eq!(app.name.as_deref(), Some("High"));
    assert_eq!(app.latest_version.as_deref(), Some("3"));
}

#[test]
fn inspect_honors_explicit_binding_and_reports_missing_inventory() {
    let temp = tempfile::tempdir().unwrap();
    getter_operations::startup::bootstrap_data_dir(temp.path()).unwrap();
    write_package(
        &temp.path().join("repo/low/android/com.example"),
        "Low",
        "2",
    );
    register(temp.path(), "low", 0, true, true);
    let app = inspect_app(
        temp.path(),
        InstalledInventory::new(vec![]),
        &"android/com.example".parse().unwrap(),
    )
    .unwrap();
    assert_eq!(app.repository_id.as_deref(), Some("low"));
    assert_eq!(
        app.update_status,
        getter_operations::startup::UpdateStatus::NotInstalled
    );
    assert!(app
        .diagnostics
        .iter()
        .any(|d| d.code == "startup.installed_inventory_missing"));
}

#[test]
fn inspect_rejects_untracked_and_disabled_packages() {
    let temp = tempfile::tempdir().unwrap();
    getter_operations::startup::bootstrap_data_dir(temp.path()).unwrap();
    let id = "android/com.example".parse().unwrap();
    assert!(matches!(
        inspect_app(temp.path(), InstalledInventory::new(vec![]), &id),
        Err(AppOperationError::Untracked(_))
    ));
    write_package(
        &temp.path().join("repo/official/android/com.example"),
        "Example",
        "2",
    );
    register(temp.path(), "official", 0, true, false);
    assert!(matches!(
        inspect_app(temp.path(), InstalledInventory::new(vec![]), &id),
        Err(AppOperationError::Disabled(_))
    ));
}

#[test]
fn inspect_reports_missing_package_definition_without_inventing_details() {
    let temp = tempfile::tempdir().unwrap();
    getter_operations::startup::bootstrap_data_dir(temp.path()).unwrap();
    register(temp.path(), "official", 0, true, true);
    let app = inspect_app(
        temp.path(),
        InstalledInventory::new(vec![]),
        &"android/com.example".parse().unwrap(),
    )
    .unwrap();
    assert_eq!(app.name, None);
    assert_eq!(app.latest_version, None);
    assert_eq!(
        app.update_status,
        getter_operations::startup::UpdateStatus::NotInstalled
    );
    assert!(!app.diagnostics.is_empty());
}

#[test]
fn check_preserves_stale_cache_refresh_diagnostics_on_the_app() {
    let temp = tempfile::tempdir().unwrap();
    getter_operations::startup::bootstrap_data_dir(temp.path()).unwrap();
    write_github_package(&temp.path().join("repo/official/android/com.example"));
    register(temp.path(), "official", 0, false, true);
    let config = getter_operations::github_releases::GithubReleaseConfig {
        api_base_url: getter_operations::github_releases::DEFAULT_GITHUB_API_BASE_URL.into(),
        owner: "owner".into(),
        repo: "repo".into(),
    };
    CacheDb::open(temp.path().join("cache.db"))
        .unwrap()
        .upsert_provider_response(&ProviderResponseUpsert {
            cache_key: config.cache_key(),
            provider: "github".into(),
            response_json: serde_json::from_str(GITHUB_RELEASES).unwrap(),
            source_response_sha512: vec!["14be2aa0ad6385271f005345095584d79556562f96cdcb7c25f500f5f35246fccacb3bbfbc38f8df48c512cee853928aad8b4fb7138f4bf9c8b5835b7e46838a".into()],
            provenance_schema_version: Some("provider-response-provenance-v1".into()),
            freshness_json: json!({}),
        })
        .unwrap();

    let checked = check_app_with_github_transport(
        &mut GetterRuntime::new(),
        temp.path(),
        inventory("1"),
        &"android/com.example".parse().unwrap(),
        Some(Rc::new(FailingGithubTransport)),
    )
    .unwrap();

    let codes = checked
        .app
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    assert!(codes.contains(&"cache.refresh_failed"));
    assert!(codes.contains(&"used_stale_cache"));
    assert!(checked
        .app
        .diagnostics
        .iter()
        .all(|diagnostic| !diagnostic.message.contains("api.github.com")));
}

#[test]
fn check_refreshes_cold_github_cache_before_matching_inventory_and_issuing_action() {
    let temp = tempfile::tempdir().unwrap();
    getter_operations::startup::bootstrap_data_dir(temp.path()).unwrap();
    write_github_package(&temp.path().join("repo/official/android/com.example"));
    register(temp.path(), "official", 0, false, true);
    let transport = Rc::new(MockGithubTransport {
        requests: RefCell::new(0),
    });
    let mut runtime = GetterRuntime::new();

    let checked = check_app_with_github_transport(
        &mut runtime,
        temp.path(),
        inventory("1"),
        &"android/com.example".parse().unwrap(),
        Some(transport.clone()),
    )
    .unwrap();

    assert_eq!(*transport.requests.borrow(), 1);
    assert_eq!(checked.app.installed_version.as_deref(), Some("1"));
    assert_eq!(checked.app.latest_version.as_deref(), Some("2"));
    assert_eq!(
        checked.app.update_status,
        getter_operations::startup::UpdateStatus::Available
    );
    let action = checked.action.expect("update action");
    assert_eq!(action.package_id.to_string(), "android/com.example");
}

#[test]
fn check_issues_current_process_action_only_when_update_exists() {
    let temp = tempfile::tempdir().unwrap();
    getter_operations::startup::bootstrap_data_dir(temp.path()).unwrap();
    write_package(
        &temp.path().join("repo/official/android/com.example"),
        "Example",
        "2",
    );
    register(temp.path(), "official", 0, true, true);
    let mut runtime = GetterRuntime::new();
    let updated = check_app(
        &mut runtime,
        temp.path(),
        inventory("1"),
        &"android/com.example".parse().unwrap(),
    )
    .unwrap();
    assert_eq!(
        updated.app.update_status,
        getter_operations::startup::UpdateStatus::Available
    );
    assert!(updated.action.is_some());

    let current = check_app(
        &mut runtime,
        temp.path(),
        inventory("2"),
        &"android/com.example".parse().unwrap(),
    )
    .unwrap();
    assert_eq!(
        current.app.update_status,
        getter_operations::startup::UpdateStatus::UpToDate
    );
    assert!(current.action.is_none());
}
