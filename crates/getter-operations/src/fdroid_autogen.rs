//! Fixture-backed F-Droid explicit autogen preview/apply operations.
//!
//! This consumes getter-owned cached F-Droid catalog facts and writes ordinary
//! generated package directories into the configured generated repository. It is
//! intentionally still fixture/offline friendly: live HTTP, downloader state,
//! installer semantics, and Flutter/Kotlin provider logic remain outside this
//! slice.

use crate::autogen::{self, AutogenAcceptance, AutogenOperationError, AutogenOperationResult};
use crate::fdroid_catalog::{
    read_or_refresh_fdroid_catalog, FdroidEndpointConfig, DEFAULT_FDROID_ENDPOINT_ID,
    DEFAULT_FDROID_ENDPOINT_URL,
};
use crate::provider_cache::ProviderCacheMode;
use getter_core::autogen::{
    content_hash, package_relative_path, record_file_key, render_autogen_record,
    validate_installed_inventory, AutogenRecord, AutogenRecordInput, GeneratedPackageFile,
    InstalledInventory, InstalledInventoryItem, AUTOGEN_RECORD_VERSION, FDROID_AUTOGEN_GENERATOR,
};
use getter_core::{InstalledTarget, PackageId, PackageKind};
use getter_providers::FdroidApp;
use getter_storage::{CacheDb, MainDb};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub fn preview_fdroid_packages_json(
    data_dir: &Path,
    main_db: &MainDb,
    cache_db: &CacheDb,
    request_json: &str,
) -> AutogenOperationResult<Value> {
    let request: FdroidAutogenPreviewRequest =
        serde_json::from_str(request_json).map_err(|source| {
            AutogenOperationError::Autogen(format!(
                "invalid F-Droid autogen preview request: {source}"
            ))
        })?;
    let endpoint = request.endpoint_config();
    let mode = request.cache_mode()?;
    let requested_package_names = requested_fdroid_package_names(&request)?;
    let catalog = read_or_refresh_fdroid_catalog(cache_db, endpoint, mode, || {
        request
            .index_xml
            .ok_or_else(|| "fixture-backed F-Droid autogen refresh requires index_xml".to_owned())
    })
    .map_err(|source| AutogenOperationError::Autogen(source.to_string()))?;
    let (target_alias, target_path, target_priority) =
        autogen::generated_repository_config(data_dir)?;
    let covered =
        autogen::higher_priority_package_coverage(main_db, &target_alias, target_priority)?;
    let mut candidates = Vec::new();
    let mut skipped = Vec::new();
    let mut diagnostics: Vec<Value> = catalog
        .diagnostics
        .iter()
        .map(|diagnostic| {
            json!({
                "code": diagnostic.code,
                "message": diagnostic.message,
                "cache_key": diagnostic.cache_key,
                "provider": diagnostic.provider,
                "stale_fetched_at_unix": diagnostic.stale_fetched_at_unix,
            })
        })
        .collect();

    for package_name in requested_package_names {
        let package_id = fdroid_package_id(&package_name)?;
        if let Some(repository_id) = covered.get(&package_id) {
            skipped.push(json!({
                "package_id": package_id.to_string(),
                "reason": "covered_by_higher_priority_repo",
                "covering_repo_id": repository_id.as_str(),
            }));
            continue;
        }
        let Some(app) = catalog.app(&package_name) else {
            diagnostics.push(json!({
                "code": "provider.fdroid.package_not_found",
                "message": format!("F-Droid package '{package_name}' was not found in endpoint '{}'", catalog.endpoint.endpoint_id),
                "package_id": package_id.to_string(),
                "provider": "fdroid",
                "endpoint_id": catalog.endpoint.endpoint_id,
            }));
            skipped.push(json!({
                "package_id": package_id.to_string(),
                "reason": "provider_package_not_found",
            }));
            continue;
        };
        let candidate = fdroid_candidate_json(
            &catalog.endpoint.endpoint_id,
            &catalog.endpoint.endpoint_url,
            &catalog.source_response_sha512,
            &package_id,
            app,
        )?;
        candidates.push(candidate);
    }

    Ok(json!({
        "operation": "fdroid.autogen.preview",
        "provider": "fdroid",
        "endpoint_id": catalog.endpoint.endpoint_id,
        "endpoint_url": catalog.endpoint.endpoint_url,
        "cache_key": catalog.cache_key,
        "source": match catalog.source {
            crate::provider_cache::ProviderCacheSource::Cache => "cache",
            crate::provider_cache::ProviderCacheSource::Refreshed => "refreshed",
            crate::provider_cache::ProviderCacheSource::Stale => "stale",
        },
        "target_repo_id": target_alias.as_str(),
        "target_repo_path": target_path,
        "summary": {
            "candidate_count": candidates.len(),
            "skipped_count": skipped.len(),
            "write_count": candidates.len(),
            "delete_count": 0,
        },
        "candidates": candidates,
        "skipped": skipped,
        "diagnostics": diagnostics,
    }))
}

pub fn apply_fdroid_preview_json(
    data_dir: &Path,
    main_db: &MainDb,
    preview: &Value,
    acceptance: &AutogenAcceptance,
) -> AutogenOperationResult<Value> {
    autogen::apply_preview(
        data_dir,
        main_db,
        preview,
        acceptance,
        "fdroid.autogen.preview",
        FDROID_AUTOGEN_GENERATOR,
    )
}

#[derive(Debug, Deserialize)]
struct FdroidAutogenPreviewRequest {
    #[serde(default)]
    endpoint_id: Option<String>,
    #[serde(default)]
    endpoint_url: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    index_xml: Option<String>,
    #[serde(default)]
    package_names: Vec<String>,
    #[serde(default)]
    installed_inventory: Option<InstalledInventory>,
}

impl FdroidAutogenPreviewRequest {
    fn endpoint_config(&self) -> FdroidEndpointConfig {
        FdroidEndpointConfig {
            endpoint_id: self
                .endpoint_id
                .clone()
                .unwrap_or_else(|| DEFAULT_FDROID_ENDPOINT_ID.to_owned()),
            endpoint_url: self
                .endpoint_url
                .clone()
                .unwrap_or_else(|| DEFAULT_FDROID_ENDPOINT_URL.to_owned()),
        }
    }

    fn cache_mode(&self) -> AutogenOperationResult<ProviderCacheMode> {
        match self.mode.as_deref() {
            Some("force_refresh") => Ok(ProviderCacheMode::ForceRefresh),
            Some("use_cached") | None => Ok(ProviderCacheMode::UseCached),
            Some(other) => Err(AutogenOperationError::Autogen(format!(
                "unknown F-Droid autogen preview mode '{other}'"
            ))),
        }
    }
}

fn requested_fdroid_package_names(
    request: &FdroidAutogenPreviewRequest,
) -> AutogenOperationResult<Vec<String>> {
    if let Some(inventory) = &request.installed_inventory {
        validate_installed_inventory(inventory)
            .map_err(|source| AutogenOperationError::Autogen(source.to_string()))?;
    }
    let mut packages = request
        .package_names
        .iter()
        .map(|package| package.trim())
        .filter(|package| !package.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if let Some(inventory) = &request.installed_inventory {
        packages.extend(inventory.items.iter().filter_map(|item| match item {
            InstalledInventoryItem::AndroidPackage { package_name, .. } => {
                let package_name = package_name.trim();
                (!package_name.is_empty()).then(|| package_name.to_owned())
            }
            InstalledInventoryItem::MagiskModule { .. } => None,
        }));
    }
    packages.sort();
    packages.dedup();
    Ok(packages)
}

fn fdroid_package_id(package_name: &str) -> AutogenOperationResult<PackageId> {
    PackageId::new(PackageKind::Android, format!("f-droid/app/{package_name}"))
        .map_err(|source| AutogenOperationError::Autogen(source.to_string()))
}

fn fdroid_candidate_json(
    endpoint_id: &str,
    endpoint_url: &str,
    source_response_sha512: &[String],
    package_id: &PackageId,
    app: &FdroidApp,
) -> AutogenOperationResult<Value> {
    let relative_path = package_relative_path(package_id);
    let display_name = app
        .name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&app.package_name);
    let files = fdroid_generated_files(endpoint_id, endpoint_url, source_response_sha512, app)?;
    let record = AutogenRecord {
        version: AUTOGEN_RECORD_VERSION,
        generator: FDROID_AUTOGEN_GENERATOR.to_owned(),
        package_id: package_id.clone(),
        output_relative_path: relative_path.clone(),
        input: AutogenRecordInput::FdroidPackage {
            endpoint_id: endpoint_id.to_owned(),
            endpoint_url: endpoint_url.to_owned(),
            package_name: app.package_name.clone(),
        },
        files: files
            .iter()
            .map(|file| {
                (
                    record_file_key(&file.relative_path),
                    file.content_hash.clone(),
                )
            })
            .collect(),
    };
    let record_content = render_autogen_record(&record)
        .map_err(|source| AutogenOperationError::Autogen(source.to_string()))?;
    let content_hash = content_hash(&record_content);
    let file_json: Vec<Value> = files
        .iter()
        .map(|file| {
            json!({
                "relative_path": file.relative_path,
                "content_hash": file.content_hash,
                "content": file.content,
            })
        })
        .collect();

    Ok(json!({
        "package_id": package_id.to_string(),
        "kind": package_id.kind().as_str(),
        "display_name": display_name,
        "installed_target": InstalledTarget::AndroidPackage { package_name: app.package_name.clone() },
        "action": "create",
        "output_relative_path": relative_path,
        "content_hash": content_hash,
        "content": record_content,
        "autogen_record_content": record_content,
        "files": file_json,
        "provider": "fdroid",
        "endpoint_id": endpoint_id,
        "upstream_id": app.package_name,
    }))
}

fn fdroid_generated_files(
    endpoint_id: &str,
    endpoint_url: &str,
    source_response_sha512: &[String],
    app: &FdroidApp,
) -> AutogenOperationResult<Vec<GeneratedPackageFile>> {
    if endpoint_id != DEFAULT_FDROID_ENDPOINT_ID || endpoint_url != DEFAULT_FDROID_ENDPOINT_URL {
        return Err(AutogenOperationError::Autogen(format!(
            "F-Droid provider-module autogen currently supports only endpoint '{}={}', got '{}={}'",
            DEFAULT_FDROID_ENDPOINT_ID, DEFAULT_FDROID_ENDPOINT_URL, endpoint_id, endpoint_url
        )));
    }
    if source_response_sha512.is_empty() {
        return Err(AutogenOperationError::Autogen(
            "F-Droid provider-module autogen requires provider source provenance for Manifest generation"
                .to_owned(),
        ));
    }
    let metadata = render_pretty_json(json!({
        "type": "android:app",
        "android": { "package_name": app.package_name },
    }))?;
    let manifest = fdroid_manifest(source_response_sha512, app);
    let version_lua = fdroid_version_lua(app);
    Ok(vec![
        generated_file("metadata.jsonc", metadata),
        generated_file("Manifest", manifest),
        generated_file("9999.lua", version_lua),
    ])
}

fn fdroid_manifest(source_response_sha512: &[String], app: &FdroidApp) -> String {
    let mut memberships = HashSet::new();
    let mut manifest = String::new();
    for digest in source_response_sha512 {
        let membership = (digest.clone(), "fdroid-index.xml".to_owned());
        if memberships.insert(membership.clone()) {
            manifest.push_str(&format!("{} {}\n", membership.0, membership.1));
        }
    }
    for release in &app.packages {
        if let Some(digest) = release.sha256.as_deref().filter(|digest| {
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            let membership = (digest.to_ascii_lowercase(), release.apk_name.clone());
            if memberships.insert(membership.clone()) {
                manifest.push_str(&format!("{} {}\n", membership.0, membership.1));
            }
        }
    }
    manifest
}

fn fdroid_version_lua(app: &FdroidApp) -> String {
    format!(
        "#!/bin/upa-lua v1\n-- @generated by UpgradeAll getter autogen (F-Droid provider module)\nlocal fdroid = require(\"luaclass.fdroid_android\")\n\nreturn fdroid.package {{\n  package_name = {},\n}}\n",
        lua_string(&app.package_name),
    )
}

fn render_pretty_json(value: Value) -> AutogenOperationResult<String> {
    serde_json::to_string_pretty(&value)
        .map(|mut content| {
            content.push('\n');
            content
        })
        .map_err(|source| AutogenOperationError::Autogen(source.to_string()))
}

fn generated_file(relative_path: &str, content: String) -> GeneratedPackageFile {
    GeneratedPackageFile {
        relative_path: PathBuf::from(relative_path),
        content_hash: content_hash(&content),
        content,
    }
}

fn lua_string(value: &str) -> String {
    let mut escaped = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other => escaped.push(other),
        }
    }
    escaped.push('"');
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_cache::source_response_sha512;
    #[cfg(feature = "lua")]
    use getter_core::repository::{PackageTypeMetadata, RepositoryPackageDirectoryLayout};
    use getter_core::repository::{RepositoryMetadata, REPO_API_VERSION_V1};
    #[cfg(feature = "lua")]
    use getter_core::runtime::GetterRuntime;
    use getter_core::RepositoryPriority;

    const FDROID_FIXTURE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<fdroid>
  <repo name="F-Droid" timestamp="1700000000" url="https://f-droid.org/repo" />
  <application id="org.fdroid.fdroid">
    <name>F-Droid</name>
    <summary>App repository client</summary>
    <package>
      <version>1.20.0</version>
      <versioncode>1020000</versioncode>
      <apkname>org.fdroid.fdroid_1020000.apk</apkname>
      <hash type="sha256">aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</hash>
      <size>1234567</size>
    </package>
    <package>
      <version>1.19.0</version>
      <versioncode>1019000</versioncode>
      <apkname>org.fdroid.fdroid_1019000.apk</apkname>
      <hash type="sha256">bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb</hash>
      <size>1111111</size>
    </package>
  </application>
  <application id="org.example.other">
    <name>Other</name>
    <package>
      <version>2.0</version>
      <apkname>org.example.other_2.apk</apkname>
    </package>
  </application>
</fdroid>
"#;

    #[test]
    fn preview_generates_fdroid_package_directory_from_cached_catalog() {
        let temp = tempfile::tempdir().unwrap();
        let main_db = MainDb::open(temp.path().join("main.db")).unwrap();
        let cache_db = CacheDb::open(temp.path().join("cache.db")).unwrap();

        let preview = preview_fdroid_packages_json(
            temp.path(),
            &main_db,
            &cache_db,
            &json!({
                "index_xml": FDROID_FIXTURE,
                "package_names": ["org.fdroid.fdroid", "missing.package"]
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(preview["operation"], "fdroid.autogen.preview");
        assert_eq!(preview["target_repo_id"], "autogen");
        assert_eq!(preview["source"], "refreshed");
        assert_eq!(preview["candidates"].as_array().unwrap().len(), 1);
        let candidate = &preview["candidates"][0];
        assert_eq!(
            candidate["package_id"],
            "android/f-droid/app/org.fdroid.fdroid"
        );
        assert_eq!(
            candidate["output_relative_path"],
            "android/f-droid/app/org.fdroid.fdroid"
        );
        let record: Value =
            serde_json::from_str(candidate["autogen_record_content"].as_str().unwrap()).unwrap();
        assert_eq!(record["generator"], "fdroid-catalog");
        assert_eq!(record["input"]["kind"], "fdroid_package");
        assert_eq!(record["input"]["endpoint_id"], "official");
        assert_eq!(record["input"]["endpoint_url"], "https://f-droid.org/repo");
        assert_eq!(record["input"]["package_name"], "org.fdroid.fdroid");
        let files = candidate["files"].as_array().unwrap();
        let version_lua = files
            .iter()
            .find(|file| file["relative_path"] == "9999.lua")
            .and_then(|file| file["content"].as_str())
            .unwrap();
        assert!(version_lua.contains("local fdroid = require(\"luaclass.fdroid_android\")"));
        assert!(version_lua.contains("return fdroid.package"));
        assert!(version_lua.contains("package_name = \"org.fdroid.fdroid\""));
        assert!(!version_lua.contains("endpoint_id"));
        assert!(!version_lua.contains("local catalog_package ="));
        assert!(!version_lua.contains("version_code = 1020000"));
        assert!(!version_lua.contains("https://f-droid.org/repo/org.fdroid.fdroid_1020000.apk"));
        assert!(!version_lua.contains("getter_dev"));
        assert!(!version_lua.contains("getter.provider"));
        let manifest = files
            .iter()
            .find(|file| file["relative_path"] == "Manifest")
            .and_then(|file| file["content"].as_str())
            .unwrap();
        assert_eq!(
            manifest,
            format!(
                "{} fdroid-index.xml\n{} org.fdroid.fdroid_1020000.apk\n{} org.fdroid.fdroid_1019000.apk\n",
                source_response_sha512(FDROID_FIXTURE),
                "a".repeat(64),
                "b".repeat(64),
            )
        );
        assert_eq!(
            preview["skipped"][0]["reason"],
            "provider_package_not_found"
        );
        assert_eq!(
            preview["diagnostics"][0]["code"],
            "provider.fdroid.package_not_found"
        );
        assert!(!temp.path().join("repo/autogen").exists());
    }

    #[test]
    fn manifest_deduplicates_reused_artifact_membership() {
        let digest = "A".repeat(64);
        let app = FdroidApp {
            package_name: "org.example.app".to_owned(),
            name: None,
            summary: None,
            packages: vec![
                getter_providers::FdroidRelease {
                    version: "2".to_owned(),
                    version_code: Some(2),
                    apk_name: "app.apk".to_owned(),
                    sha256: Some(digest.clone()),
                    size: None,
                },
                getter_providers::FdroidRelease {
                    version: "1".to_owned(),
                    version_code: Some(1),
                    apk_name: "app.apk".to_owned(),
                    sha256: Some(digest.clone()),
                    size: None,
                },
            ],
        };

        let manifest = fdroid_manifest(&["b".repeat(128)], &app);

        getter_core::manifest::PackageManifest::parse(&manifest).unwrap();
        assert_eq!(
            manifest
                .lines()
                .filter(|line| *line == format!("{} app.apk", digest.to_ascii_lowercase()))
                .count(),
            1
        );
    }

    #[test]
    fn provider_module_generation_rejects_non_default_endpoint() {
        let app = FdroidApp {
            package_name: "org.fdroid.fdroid".to_owned(),
            name: None,
            summary: None,
            packages: Vec::new(),
        };
        let digest = vec![source_response_sha512(FDROID_FIXTURE)];

        let custom_id_error =
            fdroid_generated_files("custom", DEFAULT_FDROID_ENDPOINT_URL, &digest, &app)
                .unwrap_err();
        assert!(
            matches!(custom_id_error, AutogenOperationError::Autogen(detail) if detail.contains("supports only endpoint 'official=https://f-droid.org/repo'"))
        );

        let custom_url_error = fdroid_generated_files(
            DEFAULT_FDROID_ENDPOINT_ID,
            "https://example.invalid/fdroid",
            &digest,
            &app,
        )
        .unwrap_err();
        assert!(
            matches!(custom_url_error, AutogenOperationError::Autogen(detail) if detail.contains("got 'official=https://example.invalid/fdroid'"))
        );
    }

    #[test]
    fn provider_module_generation_requires_source_provenance() {
        let app = FdroidApp {
            package_name: "org.fdroid.fdroid".to_owned(),
            name: None,
            summary: None,
            packages: Vec::new(),
        };

        let error = fdroid_generated_files(
            DEFAULT_FDROID_ENDPOINT_ID,
            DEFAULT_FDROID_ENDPOINT_URL,
            &[],
            &app,
        )
        .unwrap_err();

        assert!(
            matches!(error, AutogenOperationError::Autogen(detail) if detail.contains("requires provider source provenance"))
        );
    }

    #[test]
    fn preview_skips_fdroid_package_covered_by_higher_priority_repo() {
        let temp = tempfile::tempdir().unwrap();
        let main_db = MainDb::open(temp.path().join("main.db")).unwrap();
        let cache_db = CacheDb::open(temp.path().join("cache.db")).unwrap();
        let official_root = temp.path().join("repo/official");
        let package_dir = official_root.join("android/f-droid/app/org.fdroid.fdroid");
        std::fs::create_dir_all(&package_dir).unwrap();
        std::fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{ "type": "android:app", "android": { "package_name": "org.fdroid.fdroid" } }"#,
        )
        .unwrap();
        main_db
            .upsert_repository(
                &RepositoryMetadata {
                    id: "official".parse().unwrap(),
                    name: "Official".to_owned(),
                    priority: RepositoryPriority::DEFAULT,
                    api_version: REPO_API_VERSION_V1.to_owned(),
                },
                Some(&official_root),
                None,
            )
            .unwrap();

        let preview = preview_fdroid_packages_json(
            temp.path(),
            &main_db,
            &cache_db,
            &json!({
                "index_xml": FDROID_FIXTURE,
                "package_names": ["org.fdroid.fdroid"]
            })
            .to_string(),
        )
        .unwrap();

        assert!(preview["candidates"].as_array().unwrap().is_empty());
        assert_eq!(
            preview["skipped"][0]["package_id"],
            "android/f-droid/app/org.fdroid.fdroid"
        );
        assert_eq!(
            preview["skipped"][0]["reason"],
            "covered_by_higher_priority_repo"
        );
        assert_eq!(preview["skipped"][0]["covering_repo_id"], "official");
    }

    #[cfg(feature = "lua")]
    #[test]
    fn apply_writes_valid_fdroid_package_directory() {
        let temp = tempfile::tempdir().unwrap();
        let main_db = MainDb::open(temp.path().join("main.db")).unwrap();
        let cache_db = CacheDb::open(temp.path().join("cache.db")).unwrap();
        let preview = preview_fdroid_packages_json(
            temp.path(),
            &main_db,
            &cache_db,
            &json!({
                "index_xml": FDROID_FIXTURE,
                "package_names": ["org.fdroid.fdroid"]
            })
            .to_string(),
        )
        .unwrap();

        let result = apply_fdroid_preview_json(
            temp.path(),
            &main_db,
            &preview,
            &AutogenAcceptance::AcceptAll,
        )
        .unwrap();

        assert_eq!(result["applied_count"], 1);
        let repo_root = temp.path().join("repo/autogen");
        let package_dir = repo_root.join("android/f-droid/app/org.fdroid.fdroid");
        assert!(package_dir.join("metadata.jsonc").is_file());
        assert!(package_dir.join("Manifest").is_file());
        assert!(package_dir.join("9999.lua").is_file());
        assert!(package_dir.join(".autogen.jsonc").is_file());
        let layout = RepositoryPackageDirectoryLayout::load(&repo_root).unwrap();
        let package_dir = layout
            .package(&"android/f-droid/app/org.fdroid.fdroid".parse().unwrap())
            .unwrap();
        let metadata = layout.package_metadata(package_dir).unwrap();
        assert!(matches!(
            &metadata.package,
            PackageTypeMetadata::AndroidApp { android }
                if android.package_name == "org.fdroid.fdroid"
        ));

        let mut runtime = GetterRuntime::new();
        let issued = crate::runtime::issue_action_from_registered_package_json(
            &mut runtime,
            temp.path(),
            &main_db,
            &json!({
                "package_id": "android/f-droid/app/org.fdroid.fdroid",
                "installed_version": "1.0.0"
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(issued["provider_calls"][0]["provider"], "fdroid");
        assert_eq!(issued["provider_calls"][0]["source"], "cache");
        assert_eq!(issued["update"]["status"], "update_available");
        assert_eq!(
            issued["update"]["selected"]["candidate"]["version"],
            "1.20.0"
        );
        assert_eq!(
            issued["update"]["selected"]["candidate"]["version_code"],
            1020000
        );
        assert_eq!(
            issued["update"]["actions"][0]["url"],
            "https://f-droid.org/repo/org.fdroid.fdroid_1020000.apk"
        );
    }

    #[cfg(feature = "lua")]
    #[test]
    fn generated_fdroid_package_update_check_rejects_manifest_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        let main_db = MainDb::open(temp.path().join("main.db")).unwrap();
        let cache_db = CacheDb::open(temp.path().join("cache.db")).unwrap();
        let preview = preview_fdroid_packages_json(
            temp.path(),
            &main_db,
            &cache_db,
            &json!({
                "index_xml": FDROID_FIXTURE,
                "package_names": ["org.fdroid.fdroid"]
            })
            .to_string(),
        )
        .unwrap();
        apply_fdroid_preview_json(
            temp.path(),
            &main_db,
            &preview,
            &AutogenAcceptance::AcceptAll,
        )
        .unwrap();
        let manifest_path = temp
            .path()
            .join("repo/autogen/android/f-droid/app/org.fdroid.fdroid/Manifest");
        std::fs::write(&manifest_path, "00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000 fdroid-index.xml\n").unwrap();

        let mut runtime = GetterRuntime::new();
        let error = crate::runtime::issue_action_from_registered_package_json(
            &mut runtime,
            temp.path(),
            &main_db,
            &json!({
                "package_id": "android/f-droid/app/org.fdroid.fdroid",
                "installed_version": "1.0.0"
            })
            .to_string(),
        )
        .unwrap_err();

        assert_eq!(error.code(), "package.eval_error");
        assert!(error
            .to_string()
            .contains("package.provider.response_not_in_manifest"));
    }

    #[test]
    fn apply_rejects_existing_fdroid_directory_owned_by_another_generator() {
        let temp = tempfile::tempdir().unwrap();
        let main_db = MainDb::open(temp.path().join("main.db")).unwrap();
        let cache_db = CacheDb::open(temp.path().join("cache.db")).unwrap();
        let preview = preview_fdroid_packages_json(
            temp.path(),
            &main_db,
            &cache_db,
            &json!({
                "index_xml": FDROID_FIXTURE,
                "package_names": ["org.fdroid.fdroid"]
            })
            .to_string(),
        )
        .unwrap();
        apply_fdroid_preview_json(
            temp.path(),
            &main_db,
            &preview,
            &AutogenAcceptance::AcceptAll,
        )
        .unwrap();
        let record_path = temp
            .path()
            .join("repo/autogen/android/f-droid/app/org.fdroid.fdroid/.autogen.jsonc");
        let mut record: Value =
            serde_json::from_str(&std::fs::read_to_string(&record_path).unwrap())
                .expect("record JSON parses");
        record["generator"] = Value::String("installed-inventory".to_owned());
        std::fs::write(
            &record_path,
            serde_json::to_string_pretty(&record).unwrap() + "\n",
        )
        .unwrap();

        let error = apply_fdroid_preview_json(
            temp.path(),
            &main_db,
            &preview,
            &AutogenAcceptance::AcceptAll,
        )
        .unwrap_err();

        assert!(
            matches!(error, AutogenOperationError::Autogen(detail) if detail.contains("does not match 'fdroid-catalog'"))
        );
    }

    #[test]
    fn apply_rejects_existing_fdroid_directory_without_ownership() {
        let temp = tempfile::tempdir().unwrap();
        let main_db = MainDb::open(temp.path().join("main.db")).unwrap();
        let cache_db = CacheDb::open(temp.path().join("cache.db")).unwrap();
        let existing = temp
            .path()
            .join("repo/autogen/android/f-droid/app/org.fdroid.fdroid");
        std::fs::create_dir_all(&existing).unwrap();
        std::fs::write(
            existing.join("metadata.jsonc"),
            r#"{ "type": "android:app", "android": { "package_name": "org.fdroid.fdroid" } }"#,
        )
        .unwrap();
        let preview = preview_fdroid_packages_json(
            temp.path(),
            &main_db,
            &cache_db,
            &json!({
                "index_xml": FDROID_FIXTURE,
                "package_names": ["org.fdroid.fdroid"]
            })
            .to_string(),
        )
        .unwrap();

        let error = apply_fdroid_preview_json(
            temp.path(),
            &main_db,
            &preview,
            &AutogenAcceptance::AcceptAll,
        )
        .unwrap_err();

        assert!(
            matches!(error, AutogenOperationError::Autogen(detail) if detail.contains("missing .autogen.jsonc"))
        );
    }

    #[test]
    fn preview_matches_installed_android_inventory_against_fdroid_catalog() {
        let temp = tempfile::tempdir().unwrap();
        let main_db = MainDb::open(temp.path().join("main.db")).unwrap();
        let cache_db = CacheDb::open(temp.path().join("cache.db")).unwrap();

        let preview = preview_fdroid_packages_json(
            temp.path(),
            &main_db,
            &cache_db,
            &json!({
                "index_xml": FDROID_FIXTURE,
                "installed_inventory": {
                    "format": "upgradeall-installed-inventory",
                    "version": 1,
                    "items": [
                        {
                            "kind": "android",
                            "package_name": "org.fdroid.fdroid",
                            "label": "F-Droid"
                        },
                        {
                            "kind": "android",
                            "package_name": "missing.package",
                            "label": "Missing"
                        },
                        {
                            "kind": "magisk",
                            "module_id": "zygisk-next"
                        }
                    ]
                }
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(preview["operation"], "fdroid.autogen.preview");
        assert_eq!(preview["candidates"].as_array().unwrap().len(), 1);
        assert_eq!(
            preview["candidates"][0]["package_id"],
            "android/f-droid/app/org.fdroid.fdroid"
        );
        assert_eq!(
            preview["skipped"][0]["package_id"],
            "android/f-droid/app/missing.package"
        );
        assert_eq!(
            preview["skipped"][0]["reason"],
            "provider_package_not_found"
        );
    }
}
