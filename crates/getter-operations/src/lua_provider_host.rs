//! Operation-installed Lua provider host bindings.
//!
//! This module is an implementation bridge toward ADR-0012 provider-backed Lua
//! modules. It exercises higher-level getter operations installing host
//! functions for package evaluation without making `getter-core` depend on
//! provider cache/storage crates and without defining the final stable Lua
//! provider API.

use crate::github_releases::{
    read_or_refresh_github_releases, GithubReleaseConfig, GithubReleaseOperationError,
    DEFAULT_GITHUB_API_BASE_URL, GITHUB_ASSET_NOT_FOUND, GITHUB_PROVIDER_ID,
};
use crate::provider_cache::{ProviderCacheDiagnostic, ProviderCacheMode, ProviderCacheSource};
use getter_core::lua::{evaluate_package_directory_script_with_host_bindings, LuaPackageError};
use getter_core::repository::{RepositoryLoadError, RepositoryPackageDirectoryLayout};
use getter_core::{PackageId, RepositoryId, UpdateArtifact, UpdateCandidate};
use getter_providers::{
    github_release_update_candidates, GithubAssetFilter, GithubProviderError, GithubRelease,
    GithubReleaseCandidateOptions,
};
use getter_storage::{CacheDb, MainDb, StorageError, StoredRepository};
use mlua::{Lua, Table, Value as LuaValue};
use serde::Deserialize;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Debug, thiserror::Error)]
pub enum LuaProviderHostOperationError {
    #[error("invalid Lua provider-host request: {0}")]
    InvalidRequest(String),
    #[error("storage operation failed: {0}")]
    Storage(#[from] StorageError),
    #[error("repository operation failed: {0}")]
    Repository(#[from] RepositoryLoadError),
    #[error("package evaluation failed: {0}")]
    PackageEval(#[from] LuaPackageError),
    #[error("GitHub provider operation failed: {0}")]
    Github(#[from] GithubReleaseOperationError),
    #[error("GitHub provider normalization failed: {0}")]
    GithubProvider(#[from] GithubProviderError),
    #[error("Lua provider-host response serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Evaluates a package with a fixture-backed GitHub release host binding.
///
/// This is a development/provider-host tracer, not the stable package-eval or
/// product update-check API. Repository packages can exercise repository-local
/// `luaclass/` modules that call `getter_dev.github_release_candidates { ... }`,
/// while release parsing/cache behavior stays in `getter-operations`.
pub fn github_package_eval_json(
    data_dir: &Path,
    request_json: &str,
) -> Result<Value, LuaProviderHostOperationError> {
    let request: GithubPackageEvalRequest = serde_json::from_str(request_json)
        .map_err(|source| LuaProviderHostOperationError::InvalidRequest(source.to_string()))?;
    let mode = provider_cache_mode(request.mode.as_deref())?;
    let api_base_url = request
        .api_base_url
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_GITHUB_API_BASE_URL.to_owned());
    let main_db = MainDb::open(data_dir.join("main.db"))?;
    let repository = find_repository(&main_db, &request.repository_id)?;
    let repository_path = repo_path(&repository)?;
    let layout = RepositoryPackageDirectoryLayout::load(&repository_path)?;
    let package_directory = layout.package(&request.package_id).ok_or_else(|| {
        LuaProviderHostOperationError::InvalidRequest(format!(
            "package '{}' was not found in repository '{}'",
            request.package_id, request.repository_id
        ))
    })?;
    let metadata = layout.package_metadata(package_directory)?;
    let script = layout.unambiguous_version_script(package_directory)?;
    let provider_calls = Rc::new(RefCell::new(Vec::new()));
    let cache_db_path = data_dir.join("cache.db");
    let default_include_prereleases = request.include_prereleases;
    let releases_json = request.releases_json;
    let package = evaluate_package_directory_script_with_host_bindings(
        &repository.id,
        package_directory,
        &metadata,
        script,
        {
            let provider_calls = Rc::clone(&provider_calls);
            move |lua| {
                install_github_dev_host(
                    lua,
                    GithubDevHostConfig {
                        cache_db_path,
                        api_base_url,
                        mode,
                        releases_json,
                        default_include_prereleases,
                        provider_calls,
                    },
                )
            }
        },
    )?;
    let package = serde_json::to_value(package)?;

    Ok(json!({
        "operation": "github.package_eval.fixture",
        "package": package,
        "provider_calls": provider_calls.borrow().clone(),
    }))
}

#[derive(Debug, Deserialize)]
struct GithubPackageEvalRequest {
    repository_id: RepositoryId,
    package_id: PackageId,
    #[serde(default)]
    api_base_url: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    releases_json: Option<String>,
    #[serde(default)]
    include_prereleases: bool,
}

struct GithubDevHostConfig {
    cache_db_path: PathBuf,
    api_base_url: String,
    mode: ProviderCacheMode,
    releases_json: Option<String>,
    default_include_prereleases: bool,
    provider_calls: Rc<RefCell<Vec<Value>>>,
}

fn install_github_dev_host(lua: &Lua, config: GithubDevHostConfig) -> mlua::Result<()> {
    let host = lua.create_table()?;
    host.set(
        "github_release_candidates",
        lua.create_function(move |lua, spec: Table| {
            let request = GithubReleaseHostRequest::from_lua_table(&spec)?;
            let db = CacheDb::open(&config.cache_db_path).map_err(mlua::Error::external)?;
            let provider_config = GithubReleaseConfig {
                api_base_url: config.api_base_url.clone(),
                owner: request.owner,
                repo: request.repo,
            };
            let refresh_fixture = config.releases_json.clone();
            let result = read_or_refresh_github_releases(&db, provider_config, config.mode, || {
                refresh_fixture.ok_or_else(|| {
                    "fixture-backed GitHub release host requires releases_json".to_owned()
                })
            })
            .map_err(mlua::Error::external)?;
            let options = GithubReleaseCandidateOptions {
                include_prereleases: request
                    .include_prereleases
                    .unwrap_or(config.default_include_prereleases),
                asset_filter: request.asset_filter,
            };
            let candidates = github_release_update_candidates(&result.releases, &options)
                .map_err(mlua::Error::external)?;
            let mut diagnostics = result
                .diagnostics
                .iter()
                .map(provider_diagnostic_json)
                .collect::<Vec<_>>();
            if candidates.is_empty()
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
            }
            config.provider_calls.borrow_mut().push(json!({
                "provider": GITHUB_PROVIDER_ID,
                "request": "releases",
                "owner": result.config.owner,
                "repo": result.config.repo,
                "cache_key": result.cache_key,
                "source": provider_source_json(result.source),
                "diagnostics": diagnostics,
            }));
            update_candidates_to_lua(lua, &candidates)
        })?,
    )?;
    lua.globals().set("getter_dev", host)
}

struct GithubReleaseHostRequest {
    owner: String,
    repo: String,
    asset_filter: GithubAssetFilter,
    include_prereleases: Option<bool>,
}

impl GithubReleaseHostRequest {
    fn from_lua_table(table: &Table) -> mlua::Result<Self> {
        Ok(Self {
            owner: required_lua_string(table, "owner")?,
            repo: required_lua_string(table, "repo")?,
            asset_filter: asset_filter_from_lua(table.get("asset")?)?,
            include_prereleases: optional_lua_bool(table, "include_prereleases")?,
        })
    }
}

fn provider_cache_mode(
    mode: Option<&str>,
) -> Result<ProviderCacheMode, LuaProviderHostOperationError> {
    match mode {
        Some("force_refresh") => Ok(ProviderCacheMode::ForceRefresh),
        Some("use_cached") | None => Ok(ProviderCacheMode::UseCached),
        Some(other) => Err(LuaProviderHostOperationError::InvalidRequest(format!(
            "unknown mode '{other}'"
        ))),
    }
}

fn find_repository(
    db: &MainDb,
    repository_id: &RepositoryId,
) -> Result<StoredRepository, LuaProviderHostOperationError> {
    db.repositories()?
        .into_iter()
        .find(|repository| &repository.id == repository_id)
        .ok_or_else(|| {
            LuaProviderHostOperationError::InvalidRequest(format!(
                "repository '{repository_id}' is not registered"
            ))
        })
}

fn repo_path(repository: &StoredRepository) -> Result<PathBuf, LuaProviderHostOperationError> {
    repository.path.as_ref().map(PathBuf::from).ok_or_else(|| {
        LuaProviderHostOperationError::InvalidRequest(format!(
            "repository '{}' has no path",
            repository.id
        ))
    })
}

fn required_lua_string(table: &Table, field: &'static str) -> mlua::Result<String> {
    let value: Option<String> = table.get(field)?;
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            mlua::Error::external(format!(
                "getter_dev.github_release_candidates missing {field}"
            ))
        })
}

fn optional_lua_bool(table: &Table, field: &'static str) -> mlua::Result<Option<bool>> {
    match table.get::<LuaValue>(field)? {
        LuaValue::Nil => Ok(None),
        LuaValue::Boolean(value) => Ok(Some(value)),
        _ => Err(mlua::Error::external(format!(
            "getter_dev.github_release_candidates {field} must be a boolean"
        ))),
    }
}

fn asset_filter_from_lua(value: LuaValue) -> mlua::Result<GithubAssetFilter> {
    match value {
        LuaValue::Nil => Ok(GithubAssetFilter::default()),
        LuaValue::Table(table) => Ok(GithubAssetFilter {
            include: optional_lua_string(&table, "include")?,
            exclude: optional_lua_string(&table, "exclude")?,
        }),
        _ => Err(mlua::Error::external(
            "getter_dev.github_release_candidates asset must be a table",
        )),
    }
}

fn optional_lua_string(table: &Table, field: &'static str) -> mlua::Result<Option<String>> {
    match table.get::<LuaValue>(field)? {
        LuaValue::Nil => Ok(None),
        LuaValue::String(value) => Ok(Some(value.to_str()?.to_owned())),
        _ => Err(mlua::Error::external(format!(
            "getter_dev.github_release_candidates asset.{field} must be a string"
        ))),
    }
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

fn update_candidates_to_lua(lua: &Lua, candidates: &[UpdateCandidate]) -> mlua::Result<LuaValue> {
    if candidates.is_empty() {
        return Ok(LuaValue::Nil);
    }

    let table = lua.create_table()?;
    for (index, candidate) in candidates.iter().enumerate() {
        table.raw_set(index + 1, update_candidate_to_lua(lua, candidate)?)?;
    }
    Ok(LuaValue::Table(table))
}

fn update_candidate_to_lua(lua: &Lua, candidate: &UpdateCandidate) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("version", candidate.version.as_str())?;
    if let Some(version_code) = candidate.version_code {
        table.set("version_code", version_code)?;
    }
    if let Some(channel) = candidate.channel.as_ref() {
        table.set("channel", channel.as_str())?;
    }
    if let Some(source) = candidate.source.as_ref() {
        table.set("source", source.as_str())?;
    }
    let artifacts = lua.create_table()?;
    for (index, artifact) in candidate.artifacts.iter().enumerate() {
        artifacts.raw_set(index + 1, update_artifact_to_lua(lua, artifact)?)?;
    }
    table.set("artifacts", artifacts)?;
    Ok(table)
}

fn update_artifact_to_lua(lua: &Lua, artifact: &UpdateArtifact) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("name", artifact.name.as_str())?;
    table.set("url", artifact.url.as_str())?;
    if let Some(file_name) = artifact.file_name.as_ref() {
        table.set("file_name", file_name.as_str())?;
    }
    if let Some(sha256) = artifact.sha256.as_ref() {
        table.set("sha256", sha256.as_str())?;
    }
    if let Some(size) = artifact.size {
        table.set("size", size)?;
    }
    Ok(table)
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
    use getter_core::repository::{RepositoryMetadata, REPO_API_VERSION_V1};
    use getter_core::{RepositoryId, RepositoryPriority};
    use getter_storage::CacheDb;
    use serde_json::json;
    use std::fs;

    const GITHUB_RELEASES_FIXTURE: &str = r#"[
  {
    "tag_name": "v1.20.0",
    "name": "F-Droid 1.20.0",
    "draft": false,
    "prerelease": false,
    "published_at": "2026-06-01T00:00:00Z",
    "assets": [
      {
        "name": "F-Droid.apk",
        "browser_download_url": "https://github.com/f-droid/fdroidclient/releases/download/v1.20.0/F-Droid.apk",
        "content_type": "application/vnd.android.package-archive",
        "size": 12345,
        "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      }
    ]
  }
]"#;

    #[test]
    fn github_package_eval_uses_repository_luaclass_and_provider_cache() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        write_github_package_fixture(data_dir, "[.]apk$");

        let first = github_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/app/org.fdroid.fdroid",
                "releases_json": GITHUB_RELEASES_FIXTURE
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(first["operation"], "github.package_eval.fixture");
        assert_eq!(first["provider_calls"][0]["source"], "refreshed");
        assert_eq!(first["package"]["name"], "F-Droid");
        assert_eq!(first["package"]["source_priority"], json!(["github"]));
        assert_eq!(first["package"]["updates"][0]["version"], "v1.20.0");
        assert_eq!(first["package"]["updates"][0]["source"], "github");
        assert_eq!(
            first["package"]["updates"][0]["artifacts"][0]["sha256"],
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        let cache_key = first["provider_calls"][0]["cache_key"].as_str().unwrap();
        assert!(CacheDb::open(data_dir.join("cache.db"))
            .unwrap()
            .provider_response(cache_key)
            .unwrap()
            .is_some());

        let second = github_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/app/org.fdroid.fdroid"
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(second["provider_calls"][0]["source"], "cache");
        assert_eq!(second["package"]["updates"][0]["version"], "v1.20.0");
    }

    #[test]
    fn github_package_eval_reports_asset_filter_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        write_github_package_fixture(data_dir, "[.]zip$");

        let result = github_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/app/org.fdroid.fdroid",
                "releases_json": GITHUB_RELEASES_FIXTURE
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(result["package"]["updates"], json!([]));
        assert_eq!(
            result["provider_calls"][0]["diagnostics"][0]["code"],
            GITHUB_ASSET_NOT_FOUND
        );
        assert_eq!(
            result["provider_calls"][0]["diagnostics"][0]["provider"],
            GITHUB_PROVIDER_ID
        );
    }

    fn write_github_package_fixture(data_dir: &std::path::Path, asset_include: &str) {
        let repo_root = data_dir.join("repo/official");
        let package_dir = repo_root.join("android/app/org.fdroid.fdroid");
        fs::create_dir_all(repo_root.join("luaclass")).unwrap();
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            repo_root.join("luaclass/github_android_apk.lua"),
            r#"
local github_android = {}

function github_android.package(spec)
  return package_version {
    name = spec.name,
    source_priority = { "github" },
    updates = getter_dev.github_release_candidates {
      owner = spec.owner,
      repo = spec.repo,
      asset = spec.asset,
    },
  }
end

return github_android
"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{
  "type": "android:app",
  "android": { "package_name": "org.fdroid.fdroid" }
}"#,
        )
        .unwrap();
        let version_script = r#"#!/bin/upa-lua v1
local github_android = require("luaclass.github_android_apk")
return github_android.package {
  name = "F-Droid",
  owner = "f-droid",
  repo = "fdroidclient",
  asset = { include = "__ASSET_INCLUDE__" },
}
"#
        .replace("__ASSET_INCLUDE__", asset_include);
        fs::write(package_dir.join("9999.lua"), version_script).unwrap();
        let main_db = MainDb::open(data_dir.join("main.db")).unwrap();
        main_db
            .upsert_repository(
                &RepositoryMetadata {
                    id: RepositoryId::new("official").unwrap(),
                    name: "Official".to_owned(),
                    priority: RepositoryPriority::DEFAULT,
                    api_version: REPO_API_VERSION_V1.to_owned(),
                },
                Some(&repo_root),
                None,
            )
            .unwrap();
    }
}
