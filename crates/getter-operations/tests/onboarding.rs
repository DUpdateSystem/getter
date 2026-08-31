use getter_core::autogen::{InstalledInventory, InstalledInventoryItem};
use getter_operations::onboarding::{
    apply_setup_preview, preview_setup_with_transport, FdroidCatalogTransport, SetupAcceptance,
    SetupCandidateCategory, SetupReadiness,
};
use getter_storage::{CacheDb, MainDb};
use std::cell::Cell;

const INDEX_XML: &str = r#"<?xml version="1.0"?>
<fdroid>
  <repo url="https://f-droid.org/repo" name="F-Droid" />
  <application id="org.fdroid.app">
    <name>F-Droid App</name>
    <package><version>1.2.3</version><versioncode>12</versioncode><apkname>fdroid.apk</apkname><hash type="sha256">aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</hash><size>123</size></package>
  </application>
</fdroid>"#;

struct MockTransport {
    calls: Cell<usize>,
    result: Result<String, String>,
}

impl FdroidCatalogTransport for MockTransport {
    fn fetch_official_index_xml(&self) -> Result<String, String> {
        self.calls.set(self.calls.get() + 1);
        self.result.clone()
    }
}

fn inventory() -> InstalledInventory {
    InstalledInventory::new(vec![
        InstalledInventoryItem::AndroidPackage {
            package_name: "org.fdroid.app".into(),
            version_name: Some("1.0".into()),
            version_code: Some(10),
            label: Some("F-Droid App".into()),
        },
        InstalledInventoryItem::AndroidPackage {
            package_name: "com.example.fallback".into(),
            version_name: Some("2.0".into()),
            version_code: Some(20),
            label: Some("Fallback".into()),
        },
    ])
}

#[test]
fn preview_refreshes_once_classifies_fdroid_first_and_deduplicates_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let main = MainDb::open(temp.path().join("main.db")).unwrap();
    let cache = CacheDb::open(temp.path().join("cache.db")).unwrap();
    let transport = MockTransport {
        calls: Cell::new(0),
        result: Ok(INDEX_XML.into()),
    };

    let preview =
        preview_setup_with_transport(temp.path(), &main, &cache, inventory(), &transport).unwrap();

    assert_eq!(transport.calls.get(), 1);
    assert_eq!(preview.candidates.len(), 2);
    assert_eq!(
        preview.candidates[0].package_id,
        "android/app/com.example.fallback"
    );
    assert_eq!(
        preview.candidates[0].category,
        SetupCandidateCategory::InstalledFallback
    );
    assert_eq!(
        preview.candidates[1].package_id,
        "android/f-droid/app/org.fdroid.app"
    );
    assert_eq!(
        preview.candidates[1].category,
        SetupCandidateCategory::Fdroid
    );
    assert!(!preview
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "setup.fdroid_unavailable"));
}

#[test]
fn refresh_failure_without_cache_keeps_fallback_and_stable_diagnostic() {
    let temp = tempfile::tempdir().unwrap();
    let main = MainDb::open(temp.path().join("main.db")).unwrap();
    let cache = CacheDb::open(temp.path().join("cache.db")).unwrap();
    let transport = MockTransport {
        calls: Cell::new(0),
        result: Err("offline".into()),
    };

    let preview =
        preview_setup_with_transport(temp.path(), &main, &cache, inventory(), &transport).unwrap();

    assert_eq!(preview.candidates.len(), 2);
    assert!(preview
        .candidates
        .iter()
        .all(|candidate| candidate.category == SetupCandidateCategory::InstalledFallback));
    assert!(preview
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "setup.fdroid_unavailable"));
}

#[test]
fn refresh_failure_with_cache_uses_stale_fdroid_match_and_excludes_fallback_duplicate() {
    let temp = tempfile::tempdir().unwrap();
    let main = MainDb::open(temp.path().join("main.db")).unwrap();
    let cache = CacheDb::open(temp.path().join("cache.db")).unwrap();
    let warm = MockTransport {
        calls: Cell::new(0),
        result: Ok(INDEX_XML.into()),
    };
    preview_setup_with_transport(temp.path(), &main, &cache, inventory(), &warm).unwrap();

    let unavailable = MockTransport {
        calls: Cell::new(0),
        result: Err("offline".into()),
    };
    let preview =
        preview_setup_with_transport(temp.path(), &main, &cache, inventory(), &unavailable)
            .unwrap();

    assert_eq!(unavailable.calls.get(), 1);
    assert_eq!(
        preview
            .candidates
            .iter()
            .filter(|candidate| candidate.package_id == "android/f-droid/app/org.fdroid.app")
            .count(),
        1
    );
    assert!(!preview.candidates.iter().any(|candidate| {
        candidate.package_id == "android/app/org.fdroid.app"
            && candidate.category == SetupCandidateCategory::InstalledFallback
    }));
    assert!(preview
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "cache.refresh_failed"));
    assert!(preview
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "used_stale_cache"));
}

#[test]
fn covered_fdroid_app_is_not_reintroduced_as_installed_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let official = temp.path().join("repo/official");
    let covered = official.join("android/f-droid/app/org.fdroid.app");
    std::fs::create_dir_all(&covered).unwrap();
    std::fs::write(
        covered.join("metadata.jsonc"),
        r#"{"type":"android:app","android":{"package_name":"org.fdroid.app"}}"#,
    )
    .unwrap();
    std::fs::write(covered.join("1.lua"), "return { version = '1' }").unwrap();
    let main = MainDb::open(temp.path().join("main.db")).unwrap();
    assert!(
        getter_core::repository::RepositoryPackageDirectoryLayout::load(&official)
            .unwrap()
            .package(&"android/f-droid/app/org.fdroid.app".parse().unwrap())
            .is_some()
    );
    main.upsert_repository(
        &getter_core::repository::RepositoryMetadata {
            id: "official".parse().unwrap(),
            name: "Official".into(),
            priority: getter_core::RepositoryPriority::new(1),
            api_version: "1".into(),
        },
        Some(&official),
        None,
    )
    .unwrap();
    let cache = CacheDb::open(temp.path().join("cache.db")).unwrap();
    let transport = MockTransport {
        calls: Cell::new(0),
        result: Ok(INDEX_XML.into()),
    };

    let preview =
        preview_setup_with_transport(temp.path(), &main, &cache, inventory(), &transport).unwrap();
    assert!(
        !preview
            .candidates
            .iter()
            .any(|candidate| candidate.package_id.ends_with("org.fdroid.app")),
        "{:?}",
        preview.candidates
    );
}

#[test]
fn apply_rejects_tampered_or_unknown_selection_before_writes() {
    let temp = tempfile::tempdir().unwrap();
    let main = MainDb::open(temp.path().join("main.db")).unwrap();
    let cache = CacheDb::open(temp.path().join("cache.db")).unwrap();
    let transport = MockTransport {
        calls: Cell::new(0),
        result: Ok(INDEX_XML.into()),
    };
    let preview =
        preview_setup_with_transport(temp.path(), &main, &cache, inventory(), &transport).unwrap();

    let error = apply_setup_preview(
        temp.path(),
        &main,
        &preview,
        &SetupAcceptance::Accept(vec!["android/unknown".parse().unwrap()]),
    )
    .unwrap_err();
    assert!(error.to_string().contains("unknown setup candidate"));
    assert!(!temp.path().join("repo/autogen/android").exists());

    let mut tampered = preview.clone();
    tampered.candidates.clear();
    apply_setup_preview(temp.path(), &main, &tampered, &SetupAcceptance::AcceptAll).unwrap();
    assert!(temp.path().join("repo/autogen/android").exists());

    let error =
        apply_setup_preview(temp.path(), &main, &preview, &SetupAcceptance::AcceptAll).unwrap_err();
    assert!(error
        .to_string()
        .contains("unknown, consumed, or already applying"));
}

#[test]
fn apply_rejects_preview_after_tracked_state_changes_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let main = MainDb::open(temp.path().join("main.db")).unwrap();
    let cache = CacheDb::open(temp.path().join("cache.db")).unwrap();
    let transport = MockTransport {
        calls: Cell::new(0),
        result: Ok(INDEX_XML.into()),
    };
    let preview =
        preview_setup_with_transport(temp.path(), &main, &cache, inventory(), &transport).unwrap();
    main.upsert_tracked_package(&getter_storage::TrackedPackageUpsert {
        package_id: "android/app/other".parse().unwrap(),
        enabled: true,
        favorite: false,
        pin_version: None,
        repository_id: None,
        package_resolution: getter_storage::StoredPackageResolution::MissingPackageDefinition,
    })
    .unwrap();

    let error =
        apply_setup_preview(temp.path(), &main, &preview, &SetupAcceptance::AcceptAll).unwrap_err();
    assert!(error.to_string().contains("setup preview is stale"));
    assert!(!temp.path().join("repo/autogen/android").exists());
}

#[test]
fn apply_delegates_both_categories_and_returns_ready() {
    let temp = tempfile::tempdir().unwrap();
    let main = MainDb::open(temp.path().join("main.db")).unwrap();
    let cache = CacheDb::open(temp.path().join("cache.db")).unwrap();
    let transport = MockTransport {
        calls: Cell::new(0),
        result: Ok(INDEX_XML.into()),
    };
    let preview =
        preview_setup_with_transport(temp.path(), &main, &cache, inventory(), &transport).unwrap();

    let result =
        apply_setup_preview(temp.path(), &main, &preview, &SetupAcceptance::AcceptAll).unwrap();

    assert_eq!(result.readiness, SetupReadiness::Ready);
    assert_eq!(
        result.applied_package_ids,
        vec![
            "android/app/com.example.fallback",
            "android/f-droid/app/org.fdroid.app"
        ]
    );
    assert!(temp
        .path()
        .join("repo/autogen/android/app/com.example.fallback/.autogen.jsonc")
        .is_file());
    assert!(temp
        .path()
        .join("repo/autogen/android/f-droid/app/org.fdroid.app/.autogen.jsonc")
        .is_file());
}
