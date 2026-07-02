//! Fixture-backed GitHub Android APK autogen preview/apply operations.
//!
//! This module generates ordinary package directories that use the standard
//! `luaclass.github_android_apk` provider module. It consumes cached or
//! fixture-backed GitHub release facts owned by getter and deliberately avoids
//! live GitHub transport or Flutter/Kotlin provider parsing.

use crate::autogen::{self, AutogenAcceptance, AutogenOperationError, AutogenOperationResult};
use crate::github_releases::{
    read_or_refresh_github_releases, GithubReleaseConfig, DEFAULT_GITHUB_API_BASE_URL,
    GITHUB_ASSET_NOT_FOUND, GITHUB_PROVIDER_ID,
};
use crate::provider_cache::{ProviderCacheDiagnostic, ProviderCacheMode, ProviderCacheSource};
use getter_core::autogen::{
    content_hash, package_relative_path, record_file_key, render_autogen_record, AutogenRecord,
    AutogenRecordInput, GeneratedPackageFile, AUTOGEN_RECORD_VERSION, GITHUB_AUTOGEN_GENERATOR,
};
use getter_core::{InstalledTarget, PackageId, PackageKind};
use getter_providers::{
    github_release_update_candidates, GithubAssetFilter, GithubRelease,
    GithubReleaseCandidateOptions,
};
use getter_storage::{CacheDb, MainDb};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub fn preview_github_android_package_json(
    data_dir: &Path,
    main_db: &MainDb,
    cache_db: &CacheDb,
    request_json: &str,
) -> AutogenOperationResult<Value> {
    let request: GithubAutogenPreviewRequest =
        serde_json::from_str(request_json).map_err(|source| {
            AutogenOperationError::Autogen(format!(
                "invalid GitHub autogen preview request: {source}"
            ))
        })?;
    let owner = required_segment(request.owner, "owner")?;
    let repo = required_segment(request.repo, "repo")?;
    let android_package = required_non_empty(request.android_package, "android_package")?;
    let display_name = request
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&repo)
        .to_owned();
    let api_base_url = request
        .api_base_url
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_GITHUB_API_BASE_URL.to_owned());
    if api_base_url != DEFAULT_GITHUB_API_BASE_URL {
        return Err(AutogenOperationError::Autogen(format!(
            "GitHub provider-module autogen currently supports only API base URL '{DEFAULT_GITHUB_API_BASE_URL}', got '{api_base_url}'"
        )));
    }
    let mode = match request.mode.as_deref() {
        Some("force_refresh") => ProviderCacheMode::ForceRefresh,
        Some("use_cached") | None => ProviderCacheMode::UseCached,
        Some(other) => {
            return Err(AutogenOperationError::Autogen(format!(
                "unknown GitHub autogen preview mode '{other}'"
            )))
        }
    };
    let asset_filter = request.asset.unwrap_or_default().into_filter();
    let options = GithubReleaseCandidateOptions {
        include_prereleases: request.include_prereleases,
        asset_filter: asset_filter.clone(),
    };
    let package_id = github_android_package_id(&owner, &repo, &android_package)?;
    let result = read_or_refresh_github_releases(
        cache_db,
        GithubReleaseConfig {
            api_base_url,
            owner: owner.clone(),
            repo: repo.clone(),
        },
        mode,
        || {
            request.releases_json.ok_or_else(|| {
                "fixture-backed GitHub autogen refresh requires releases_json".to_owned()
            })
        },
    )
    .map_err(|source| AutogenOperationError::Autogen(source.to_string()))?;
    let (target_alias, target_path, target_priority) =
        autogen::generated_repository_config(data_dir)?;
    let covered =
        autogen::higher_priority_package_coverage(main_db, &target_alias, target_priority)?;
    let mut diagnostics: Vec<Value> = result
        .diagnostics
        .iter()
        .map(provider_diagnostic_json)
        .collect();
    let mut candidates = Vec::new();
    let mut skipped = Vec::new();

    if let Some(repository_id) = covered.get(&package_id) {
        skipped.push(json!({
            "package_id": package_id.to_string(),
            "reason": "covered_by_higher_priority_repo",
            "covering_repo_id": repository_id.as_str(),
        }));
    } else {
        let update_candidates = github_release_update_candidates(&result.releases, &options)
            .map_err(|source| AutogenOperationError::Autogen(source.to_string()))?;
        if update_candidates.is_empty()
            && github_eligible_release_count(&result.releases, &options) > 0
        {
            diagnostics.push(json!({
                "code": GITHUB_ASSET_NOT_FOUND,
                "message": "no GitHub release assets matched the requested filters",
                "provider": GITHUB_PROVIDER_ID,
                "owner": result.config.owner,
                "repo": result.config.repo,
                "cache_key": result.cache_key,
            }));
            skipped.push(json!({
                "package_id": package_id.to_string(),
                "reason": "provider_asset_not_found",
            }));
        } else if update_candidates.is_empty() {
            diagnostics.push(json!({
                "code": "provider.github.release_not_found",
                "message": format!("GitHub repository '{owner}/{repo}' has no eligible releases"),
                "provider": GITHUB_PROVIDER_ID,
                "owner": owner,
                "repo": repo,
                "cache_key": result.cache_key,
            }));
            skipped.push(json!({
                "package_id": package_id.to_string(),
                "reason": "provider_release_not_found",
            }));
        } else {
            candidates.push(github_candidate_json(
                &result.source_response_sha512,
                &package_id,
                GithubGeneratedPackageInput {
                    owner: &owner,
                    repo: &repo,
                    android_package: &android_package,
                    display_name: &display_name,
                    asset_filter: &asset_filter,
                    include_prereleases: request.include_prereleases,
                },
            )?);
        }
    }

    Ok(json!({
        "operation": "github.autogen.preview",
        "provider": GITHUB_PROVIDER_ID,
        "api_base_url": result.config.api_base_url,
        "owner": result.config.owner,
        "repo": result.config.repo,
        "cache_key": result.cache_key,
        "source": provider_source_json(result.source),
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

pub fn apply_github_preview_json(
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
        "github.autogen.preview",
        GITHUB_AUTOGEN_GENERATOR,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GithubAutogenPreviewRequest {
    #[serde(default)]
    api_base_url: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    android_package: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    asset: Option<GithubAssetFilterRequest>,
    #[serde(default)]
    include_prereleases: bool,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    releases_json: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GithubAssetFilterRequest {
    #[serde(default)]
    include: Option<String>,
    #[serde(default)]
    exclude: Option<String>,
}

impl GithubAssetFilterRequest {
    fn into_filter(self) -> GithubAssetFilter {
        GithubAssetFilter {
            include: self
                .include
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .or_else(|| Some("[.]apk$".to_owned())),
            exclude: self
                .exclude
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
        }
    }
}

struct GithubGeneratedPackageInput<'a> {
    owner: &'a str,
    repo: &'a str,
    android_package: &'a str,
    display_name: &'a str,
    asset_filter: &'a GithubAssetFilter,
    include_prereleases: bool,
}

fn github_candidate_json(
    source_response_sha512: &[String],
    package_id: &PackageId,
    input: GithubGeneratedPackageInput<'_>,
) -> AutogenOperationResult<Value> {
    let relative_path = package_relative_path(package_id);
    let files = github_generated_files(source_response_sha512, &input)?;
    let record = AutogenRecord {
        version: AUTOGEN_RECORD_VERSION,
        generator: GITHUB_AUTOGEN_GENERATOR.to_owned(),
        package_id: package_id.clone(),
        output_relative_path: relative_path.clone(),
        input: AutogenRecordInput::GithubAndroidApk {
            owner: input.owner.to_owned(),
            repo: input.repo.to_owned(),
            android_package: input.android_package.to_owned(),
            asset_include: input.asset_filter.include.clone(),
            asset_exclude: input.asset_filter.exclude.clone(),
            include_prereleases: input.include_prereleases,
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
        "display_name": input.display_name,
        "installed_target": InstalledTarget::AndroidPackage { package_name: input.android_package.to_owned() },
        "action": "create",
        "output_relative_path": relative_path,
        "content_hash": content_hash,
        "content": record_content,
        "autogen_record_content": record_content,
        "files": file_json,
        "provider": GITHUB_PROVIDER_ID,
        "upstream_id": format!("{}/{}", input.owner, input.repo),
    }))
}

fn github_generated_files(
    source_response_sha512: &[String],
    input: &GithubGeneratedPackageInput<'_>,
) -> AutogenOperationResult<Vec<GeneratedPackageFile>> {
    if source_response_sha512.is_empty() {
        return Err(AutogenOperationError::Autogen(
            "GitHub provider-module autogen requires provider source provenance for Manifest generation"
                .to_owned(),
        ));
    }
    let metadata = render_pretty_json(json!({
        "type": "android:app",
        "display_name": input.display_name,
        "android": { "package_name": input.android_package },
    }))?;
    let manifest = github_manifest(source_response_sha512);
    let version_lua = github_version_lua(input);
    Ok(vec![
        generated_file("metadata.jsonc", metadata),
        generated_file("Manifest", manifest),
        generated_file("9999.lua", version_lua),
    ])
}

fn github_manifest(source_response_sha512: &[String]) -> String {
    source_response_sha512
        .iter()
        .map(|digest| format!("{digest} github-releases.json\n"))
        .collect()
}

fn github_version_lua(input: &GithubGeneratedPackageInput<'_>) -> String {
    let mut fields = vec![
        format!("  name = {},", lua_string(input.display_name)),
        format!("  android_package = {},", lua_string(input.android_package)),
        format!("  owner = {},", lua_string(input.owner)),
        format!("  repo = {},", lua_string(input.repo)),
    ];
    if input.asset_filter.include.is_some() || input.asset_filter.exclude.is_some() {
        fields.push("  asset = {".to_owned());
        if let Some(include) = &input.asset_filter.include {
            fields.push(format!("    include = {},", lua_string(include)));
        }
        if let Some(exclude) = &input.asset_filter.exclude {
            fields.push(format!("    exclude = {},", lua_string(exclude)));
        }
        fields.push("  },".to_owned());
    }
    if input.include_prereleases {
        fields.push("  include_prereleases = true,".to_owned());
    }
    format!(
        "#!/bin/upa-lua v1\n-- @generated by UpgradeAll getter autogen (GitHub release provider module)\nlocal github_android = require(\"luaclass.github_android_apk\")\n\nreturn github_android.package {{\n{}\n}}\n",
        fields.join("\n"),
    )
}

fn github_android_package_id(
    owner: &str,
    repo: &str,
    android_package: &str,
) -> AutogenOperationResult<PackageId> {
    PackageId::new(
        PackageKind::Android,
        format!("github/{owner}/{repo}/{android_package}"),
    )
    .map_err(|source| AutogenOperationError::Autogen(source.to_string()))
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

fn required_segment(value: Option<String>, field: &'static str) -> AutogenOperationResult<String> {
    let value = required_non_empty(value, field)?;
    if value.contains('/') {
        return Err(AutogenOperationError::Autogen(format!(
            "GitHub autogen {field} must not contain '/'"
        )));
    }
    Ok(value)
}

fn required_non_empty(
    value: Option<String>,
    field: &'static str,
) -> AutogenOperationResult<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AutogenOperationError::Autogen(format!("missing {field}")))
}

fn github_eligible_release_count(
    releases: &[GithubRelease],
    options: &GithubReleaseCandidateOptions,
) -> usize {
    releases
        .iter()
        .filter(|release| !release.draft)
        .filter(|release| options.include_prereleases || !release.prerelease)
        .count()
}

fn provider_source_json(source: ProviderCacheSource) -> &'static str {
    match source {
        ProviderCacheSource::Cache => "cache",
        ProviderCacheSource::Refreshed => "refreshed",
        ProviderCacheSource::Stale => "stale",
    }
}

fn provider_diagnostic_json(diagnostic: &ProviderCacheDiagnostic) -> Value {
    json!({
        "code": diagnostic.code,
        "message": diagnostic.message,
        "cache_key": diagnostic.cache_key,
        "provider": diagnostic.provider,
        "stale_fetched_at_unix": diagnostic.stale_fetched_at_unix,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "lua")]
    use getter_core::repository::{PackageTypeMetadata, RepositoryPackageDirectoryLayout};
    use getter_core::repository::{RepositoryMetadata, REPO_API_VERSION_V1};
    #[cfg(feature = "lua")]
    use getter_core::runtime::GetterRuntime;
    use getter_core::RepositoryPriority;
    use serde_json::{json, Value};
    use sha2::{Digest, Sha512};

    const UPGRADEALL_GITHUB_RELEASES: &str =
        include_str!("../../../tests/files/web/github_api_release.json");
    const UPGRADEALL_PACKAGE_ID: &str =
        "android/github/DUpdateSystem/UpgradeAll/net.xzos.upgradeall";

    #[test]
    fn preview_generates_github_android_package_directory_from_release_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let main_db = MainDb::open(temp.path().join("main.db")).unwrap();
        let cache_db = CacheDb::open(temp.path().join("cache.db")).unwrap();

        let preview = preview_github_android_package_json(
            temp.path(),
            &main_db,
            &cache_db,
            &upgradeall_request().to_string(),
        )
        .unwrap();

        assert_eq!(preview["operation"], "github.autogen.preview");
        assert_eq!(preview["provider"], "github");
        assert_eq!(preview["owner"], "DUpdateSystem");
        assert_eq!(preview["repo"], "UpgradeAll");
        assert_eq!(preview["target_repo_id"], "autogen");
        assert_eq!(preview["source"], "refreshed");
        assert_eq!(preview["candidates"].as_array().unwrap().len(), 1);
        let candidate = &preview["candidates"][0];
        assert_eq!(candidate["package_id"], UPGRADEALL_PACKAGE_ID);
        assert_eq!(candidate["output_relative_path"], UPGRADEALL_PACKAGE_ID);
        assert_eq!(candidate["display_name"], "UpgradeAll");
        assert_eq!(
            candidate["installed_target"]["package_name"],
            "net.xzos.upgradeall"
        );
        assert_eq!(candidate["provider"], "github");
        assert_eq!(candidate["upstream_id"], "DUpdateSystem/UpgradeAll");

        let record: Value =
            serde_json::from_str(candidate["autogen_record_content"].as_str().unwrap()).unwrap();
        assert_eq!(record["generator"], "github-releases");
        assert_eq!(record["input"]["kind"], "github_android_apk");
        assert_eq!(record["input"]["owner"], "DUpdateSystem");
        assert_eq!(record["input"]["repo"], "UpgradeAll");
        assert_eq!(record["input"]["android_package"], "net.xzos.upgradeall");
        assert_eq!(record["input"]["asset_include"], "UpgradeAll_.*[.]apk$");
        assert_eq!(record["input"]["include_prereleases"], false);

        let files = candidate["files"].as_array().unwrap();
        let version_lua = generated_file_content(files, "9999.lua");
        assert!(
            version_lua.contains("local github_android = require(\"luaclass.github_android_apk\")")
        );
        assert!(version_lua.contains("return github_android.package"));
        assert!(version_lua.contains("name = \"UpgradeAll\""));
        assert!(version_lua.contains("android_package = \"net.xzos.upgradeall\""));
        assert!(version_lua.contains("owner = \"DUpdateSystem\""));
        assert!(version_lua.contains("repo = \"UpgradeAll\""));
        assert!(version_lua.contains("include = \"UpgradeAll_.*[.]apk$\""));
        assert!(!version_lua.contains("releases_json"));
        assert!(!version_lua.contains("getter.provider"));
        assert!(!version_lua.contains("getter_dev"));
        let manifest = generated_file_content(files, "Manifest");
        assert_eq!(
            manifest,
            format!(
                "{} github-releases.json\n",
                sha512_hex(UPGRADEALL_GITHUB_RELEASES.as_bytes())
            )
        );
        assert!(!temp.path().join("repo/autogen").exists());
    }

    #[cfg(feature = "lua")]
    #[test]
    fn apply_writes_valid_github_package_directory_and_runtime_uses_cache() {
        let temp = tempfile::tempdir().unwrap();
        let main_db = MainDb::open(temp.path().join("main.db")).unwrap();
        let cache_db = CacheDb::open(temp.path().join("cache.db")).unwrap();
        let preview = preview_github_android_package_json(
            temp.path(),
            &main_db,
            &cache_db,
            &upgradeall_request().to_string(),
        )
        .unwrap();

        let result = apply_github_preview_json(
            temp.path(),
            &main_db,
            &preview,
            &AutogenAcceptance::AcceptAll,
        )
        .unwrap();

        assert_eq!(result["applied_count"], 1);
        let repo_root = temp.path().join("repo/autogen");
        let package_dir = repo_root.join(UPGRADEALL_PACKAGE_ID);
        assert!(package_dir.join("metadata.jsonc").is_file());
        assert!(package_dir.join("Manifest").is_file());
        assert!(package_dir.join("9999.lua").is_file());
        assert!(package_dir.join(".autogen.jsonc").is_file());
        let layout = RepositoryPackageDirectoryLayout::load(&repo_root).unwrap();
        let package_dir = layout
            .package(&UPGRADEALL_PACKAGE_ID.parse().unwrap())
            .unwrap();
        let metadata = layout.package_metadata(package_dir).unwrap();
        assert!(matches!(
            &metadata.package,
            PackageTypeMetadata::AndroidApp { android }
                if android.package_name == "net.xzos.upgradeall"
        ));

        let mut runtime = GetterRuntime::new();
        let issued = crate::runtime::issue_action_from_registered_package_json(
            &mut runtime,
            temp.path(),
            &main_db,
            &json!({
                "package_id": UPGRADEALL_PACKAGE_ID,
                "installed_version": "0.12.0"
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(issued["provider_calls"][0]["provider"], "github");
        assert_eq!(issued["provider_calls"][0]["owner"], "DUpdateSystem");
        assert_eq!(issued["provider_calls"][0]["repo"], "UpgradeAll");
        assert_eq!(issued["provider_calls"][0]["source"], "cache");
        assert_eq!(issued["update"]["status"], "update_available");
        assert_eq!(
            issued["update"]["selected"]["candidate"]["version"],
            "0.13-beta.4"
        );
        assert_eq!(
            issued["update"]["selected"]["candidate"]["artifacts"][0]["name"],
            "UpgradeAll_0.13-beta.4.apk"
        );
        assert_eq!(
            issued["update"]["selected"]["candidate"]["artifacts"][0]["content_type"],
            "application/vnd.android.package-archive"
        );
        assert!(issued["update"]["selected"]["candidate"]["changelog"]
            .as_str()
            .unwrap()
            .contains("Ukrainian"));
    }

    #[test]
    fn provider_module_generation_requires_source_provenance() {
        let asset_filter = GithubAssetFilter {
            include: Some("[.]apk$".to_owned()),
            exclude: None,
        };
        let error = github_generated_files(
            &[],
            &GithubGeneratedPackageInput {
                owner: "DUpdateSystem",
                repo: "UpgradeAll",
                android_package: "net.xzos.upgradeall",
                display_name: "UpgradeAll",
                asset_filter: &asset_filter,
                include_prereleases: false,
            },
        )
        .unwrap_err();

        assert!(
            matches!(error, AutogenOperationError::Autogen(detail) if detail.contains("requires provider source provenance"))
        );
    }

    #[test]
    fn preview_rejects_custom_github_api_base_url_generation() {
        let temp = tempfile::tempdir().unwrap();
        let main_db = MainDb::open(temp.path().join("main.db")).unwrap();
        let cache_db = CacheDb::open(temp.path().join("cache.db")).unwrap();
        let mut request = upgradeall_request();
        request["api_base_url"] = json!("https://example.invalid/api");

        let error = preview_github_android_package_json(
            temp.path(),
            &main_db,
            &cache_db,
            &request.to_string(),
        )
        .unwrap_err();

        assert!(
            matches!(error, AutogenOperationError::Autogen(detail) if detail.contains("supports only API base URL"))
        );
    }

    #[test]
    fn preview_skips_github_package_covered_by_higher_priority_repo() {
        let temp = tempfile::tempdir().unwrap();
        let main_db = MainDb::open(temp.path().join("main.db")).unwrap();
        let cache_db = CacheDb::open(temp.path().join("cache.db")).unwrap();
        let official_root = temp.path().join("repo/official");
        let package_dir = official_root.join(UPGRADEALL_PACKAGE_ID);
        std::fs::create_dir_all(&package_dir).unwrap();
        std::fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{ "type": "android:app", "android": { "package_name": "net.xzos.upgradeall" } }"#,
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

        let preview = preview_github_android_package_json(
            temp.path(),
            &main_db,
            &cache_db,
            &upgradeall_request().to_string(),
        )
        .unwrap();

        assert!(preview["candidates"].as_array().unwrap().is_empty());
        assert_eq!(preview["skipped"][0]["package_id"], UPGRADEALL_PACKAGE_ID);
        assert_eq!(
            preview["skipped"][0]["reason"],
            "covered_by_higher_priority_repo"
        );
        assert_eq!(preview["skipped"][0]["covering_repo_id"], "official");
    }

    fn upgradeall_request() -> Value {
        json!({
            "owner": "DUpdateSystem",
            "repo": "UpgradeAll",
            "android_package": "net.xzos.upgradeall",
            "display_name": "UpgradeAll",
            "asset": {
                "include": "UpgradeAll_.*[.]apk$"
            },
            "releases_json": UPGRADEALL_GITHUB_RELEASES,
        })
    }

    fn generated_file_content<'a>(files: &'a [Value], relative_path: &str) -> &'a str {
        files
            .iter()
            .find(|file| file["relative_path"] == relative_path)
            .and_then(|file| file["content"].as_str())
            .unwrap_or_else(|| panic!("generated file {relative_path} not found: {files:?}"))
    }

    fn sha512_hex(body: &[u8]) -> String {
        let mut hasher = Sha512::new();
        hasher.update(body);
        format!("{:x}", hasher.finalize())
    }
}
