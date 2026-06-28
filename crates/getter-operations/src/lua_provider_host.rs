//! Operation-installed Lua provider host bindings.
//!
//! This module is an implementation bridge toward ADR-0012 provider-backed Lua
//! modules. It exercises higher-level getter operations installing host
//! functions for package evaluation without making `getter-core` depend on
//! provider cache/storage crates. The stable `getter.provider.*` harness here
//! is still fixture-backed, dev-feature-gated, and hidden from product APIs.

use crate::fdroid_catalog::{
    read_or_refresh_fdroid_catalog, FdroidCatalogOperationError, FdroidEndpointConfig,
    DEFAULT_FDROID_ENDPOINT_ID, DEFAULT_FDROID_ENDPOINT_URL, FDROID_PROVIDER_ID,
};
use crate::github_releases::{
    read_or_refresh_github_releases, GithubReleaseConfig, GithubReleaseOperationError,
    DEFAULT_GITHUB_API_BASE_URL, GITHUB_ASSET_NOT_FOUND, GITHUB_PROVIDER_ID,
};
use crate::lua_runtime_hooks::load_runtime_hooks;
use crate::provider_cache::{
    ProviderCacheDiagnostic, ProviderCacheMode, ProviderCacheSource,
    PROVIDER_RESPONSE_PROVENANCE_SCHEMA_V1,
};
use getter_core::lua::{evaluate_package_directory_script_with_host_bindings, LuaPackageError};
use getter_core::repository::{
    PackageDirectory, PackageDirectoryMetadata, PackageLuaPermission, PackageVersionScript,
    RepositoryLoadError, RepositoryPackageDirectoryLayout, PACKAGE_MANIFEST_FILE,
};
use getter_core::{PackageId, RepositoryId, ResolvedPackage};
use getter_providers::{
    github_release_update_candidates, FdroidEndpoint, GithubAssetFilter, GithubProviderError,
    GithubRelease, GithubReleaseCandidateOptions,
};
use getter_storage::{CacheDb, MainDb, StorageError, StoredRepository};
use mlua::{Lua, Table, Value as LuaValue};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha512};
use std::cell::RefCell;
use std::collections::BTreeSet;
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
    #[error("F-Droid provider operation failed: {0}")]
    Fdroid(#[from] FdroidCatalogOperationError),
    #[error("GitHub provider operation failed: {0}")]
    Github(#[from] GithubReleaseOperationError),
    #[error("GitHub provider normalization failed: {0}")]
    GithubProvider(#[from] GithubProviderError),
    #[error("Lua provider-host response serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("failed to read package Manifest at {path}: {source}")]
    ReadManifest {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid package Manifest at {path}: {reason}")]
    InvalidManifest { path: PathBuf, reason: String },
}

const FDROID_PACKAGE_NOT_FOUND: &str = "provider.fdroid.package_not_found";
const PROVIDER_RESPONSE_NOT_IN_MANIFEST: &str = "package.provider.response_not_in_manifest";
const PROVIDER_CACHE_PROVENANCE_MISSING: &str = "package.provider.cache_provenance_missing";

/// Evaluates a package with a fixture-backed F-Droid catalog host binding.
///
/// This is a development/provider-host tracer, not the stable package-eval or
/// product update-check API. Repository packages can exercise repository-local
/// or built-in `luaclass/` modules while catalog parsing/cache behavior stays
/// in `getter-operations`.
pub fn fdroid_package_eval_json(
    data_dir: &Path,
    request_json: &str,
) -> Result<Value, LuaProviderHostOperationError> {
    let request: FdroidPackageEvalRequest = serde_json::from_str(request_json)
        .map_err(|source| LuaProviderHostOperationError::InvalidRequest(source.to_string()))?;
    let mode = provider_cache_mode(request.mode.as_deref())?;
    let endpoint = FdroidEndpointConfig {
        endpoint_id: request
            .endpoint_id
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_FDROID_ENDPOINT_ID.to_owned()),
        endpoint_url: request
            .endpoint_url
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_FDROID_ENDPOINT_URL.to_owned()),
    };
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
    let index_xml = request.index_xml;
    let package = evaluate_package_directory_script_with_host_bindings(
        &repository.id,
        package_directory,
        &metadata,
        script,
        {
            let provider_calls = Rc::clone(&provider_calls);
            move |lua| {
                install_fdroid_dev_host(
                    lua,
                    FdroidDevHostConfig {
                        cache_db_path,
                        endpoint,
                        mode,
                        index_xml,
                        provider_calls,
                    },
                )
            }
        },
    )?;
    let package = serde_json::to_value(package)?;

    Ok(json!({
        "operation": "fdroid.package_eval.fixture",
        "package": package,
        "provider_calls": provider_calls.borrow().clone(),
    }))
}

#[derive(Debug, Deserialize)]
struct FdroidPackageEvalRequest {
    repository_id: RepositoryId,
    package_id: PackageId,
    #[serde(default)]
    endpoint_id: Option<String>,
    #[serde(default)]
    endpoint_url: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    index_xml: Option<String>,
}

struct FdroidDevHostConfig {
    cache_db_path: PathBuf,
    endpoint: FdroidEndpointConfig,
    mode: ProviderCacheMode,
    index_xml: Option<String>,
    provider_calls: Rc<RefCell<Vec<Value>>>,
}

#[derive(Clone)]
struct FdroidProviderHostConfig {
    cache_db_path: PathBuf,
    endpoint: FdroidEndpointConfig,
    mode: ProviderCacheMode,
    index_xml: Option<String>,
    manifest: Option<PackageManifest>,
}

/// Evaluates a package with a fixture-backed GitHub release host binding.
///
/// This is a development/provider-host tracer, not the stable package-eval or
/// product update-check API. Repository packages can exercise repository-local
/// or built-in `luaclass/` modules while release parsing/cache behavior stays
/// in `getter-operations`.
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

/// Evaluates a package with the fixture-backed stable `getter.provider.*` host.
///
/// This is an internal Slice 1 harness for ADR-0012 provider host API v1. It
/// installs the stable namespace and runtime hooks, but still uses fixture data
/// and remains hidden from CLI/native/Flutter product APIs.
pub fn stable_provider_package_eval_json(
    data_dir: &Path,
    request_json: &str,
) -> Result<Value, LuaProviderHostOperationError> {
    let request: StableProviderPackageEvalRequest = serde_json::from_str(request_json)
        .map_err(|source| LuaProviderHostOperationError::InvalidRequest(source.to_string()))?;
    let mode = provider_cache_mode(request.mode.as_deref())?;
    let endpoint = FdroidEndpointConfig {
        endpoint_id: request
            .fdroid_endpoint_id
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_FDROID_ENDPOINT_ID.to_owned()),
        endpoint_url: request
            .fdroid_endpoint_url
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_FDROID_ENDPOINT_URL.to_owned()),
    };
    let github_api_base_url = request
        .github_api_base_url
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
    let runtime_hooks = Rc::new(RefCell::new(Vec::new()));
    let fdroid_index_xml = request.fdroid_index_xml;
    let github_releases_json = request.github_releases_json;
    let default_include_prereleases = request.github_include_prereleases;
    let manifest = if metadata
        .permissions_for(&script.file_name)
        .contains(&PackageLuaPermission::AllowFreeNetwork)
    {
        None
    } else {
        Some(PackageManifest::load(
            package_directory.path.join(PACKAGE_MANIFEST_FILE),
        )?)
    };
    let package = evaluate_with_stable_provider_host(
        data_dir,
        &repository.id,
        package_directory,
        &metadata,
        script,
        StableProviderHostEvalConfig {
            fdroid: FdroidProviderHostConfig {
                cache_db_path: data_dir.join("cache.db"),
                endpoint,
                mode,
                index_xml: fdroid_index_xml,
                manifest: manifest.clone(),
            },
            github: GithubProviderHostConfig {
                cache_db_path: data_dir.join("cache.db"),
                api_base_url: github_api_base_url,
                mode,
                releases_json: github_releases_json,
                default_include_prereleases,
                manifest,
            },
            provider_calls: Rc::clone(&provider_calls),
            runtime_hooks: Rc::clone(&runtime_hooks),
        },
    )?;
    let package = serde_json::to_value(package)?;

    Ok(json!({
        "operation": "provider.package_eval.fixture",
        "package": package,
        "provider_calls": provider_calls.borrow().clone(),
        "runtime_hooks": runtime_hooks.borrow().clone(),
    }))
}

#[derive(Debug, Deserialize)]
struct StableProviderPackageEvalRequest {
    repository_id: RepositoryId,
    package_id: PackageId,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    fdroid_endpoint_id: Option<String>,
    #[serde(default)]
    fdroid_endpoint_url: Option<String>,
    #[serde(default)]
    fdroid_index_xml: Option<String>,
    #[serde(default)]
    github_api_base_url: Option<String>,
    #[serde(default)]
    github_releases_json: Option<String>,
    #[serde(default)]
    github_include_prereleases: bool,
}

struct StableProviderHostEvalConfig {
    fdroid: FdroidProviderHostConfig,
    github: GithubProviderHostConfig,
    provider_calls: Rc<RefCell<Vec<Value>>>,
    runtime_hooks: Rc<RefCell<Vec<PathBuf>>>,
}

fn evaluate_with_stable_provider_host(
    data_dir: &Path,
    repository_id: &RepositoryId,
    package: &PackageDirectory,
    metadata: &PackageDirectoryMetadata,
    script: &PackageVersionScript,
    config: StableProviderHostEvalConfig,
) -> Result<ResolvedPackage, LuaProviderHostOperationError> {
    evaluate_package_directory_script_with_host_bindings(
        repository_id,
        package,
        metadata,
        script,
        move |lua| {
            install_stable_provider_host(lua, config.fdroid, config.github)?;
            *config.runtime_hooks.borrow_mut() = load_runtime_hooks(lua, data_dir)?;
            install_provider_call_trace(lua, Rc::clone(&config.provider_calls))?;
            Ok(())
        },
    )
    .map_err(Into::into)
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

#[derive(Clone)]
struct GithubProviderHostConfig {
    cache_db_path: PathBuf,
    api_base_url: String,
    mode: ProviderCacheMode,
    releases_json: Option<String>,
    default_include_prereleases: bool,
    manifest: Option<PackageManifest>,
}

fn install_fdroid_dev_host(lua: &Lua, config: FdroidDevHostConfig) -> mlua::Result<()> {
    let provider_config = FdroidProviderHostConfig {
        cache_db_path: config.cache_db_path.clone(),
        endpoint: config.endpoint.clone(),
        mode: config.mode,
        index_xml: config.index_xml.clone(),
        manifest: None,
    };
    let provider_calls = Rc::clone(&config.provider_calls);
    let update_candidates = lua.create_function(move |lua, spec: Table| {
        let result = fdroid_update_candidates_envelope(lua, spec, provider_config.clone())?;
        provider_calls
            .borrow_mut()
            .push(provider_call_from_envelope(&result)?);
        Ok(result)
    })?;

    let globals = lua.globals();
    let getter = nested_table(lua, globals.clone(), "getter")?;
    let provider = nested_table(lua, getter, "provider")?;
    let fdroid_table = nested_table(lua, provider, "fdroid")?;
    fdroid_table.set("update_candidates", update_candidates.clone())?;

    let getter_builtin = nested_table(lua, globals.clone(), "getter_builtin")?;
    let builtin_provider = nested_table(lua, getter_builtin, "provider")?;
    let builtin_fdroid = nested_table(lua, builtin_provider, "fdroid")?;
    builtin_fdroid.set("update_candidates", update_candidates.clone())?;

    let host = lua.create_table()?;
    host.set(
        "fdroid_update_candidates",
        lua.create_function(move |_, spec: Table| {
            let result: Table = update_candidates.call(spec)?;
            envelope_candidates(result)
        })?,
    )?;
    globals.set("getter_dev", host)
}

struct FdroidUpdateHostRequest {
    package_name: String,
}

impl FdroidUpdateHostRequest {
    fn from_lua_table(table: &Table) -> mlua::Result<Self> {
        let package_name: Option<String> = table.get("package_name")?;
        Ok(Self {
            package_name: package_name
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    mlua::Error::external("F-Droid provider host missing package_name")
                })?,
        })
    }
}

fn install_github_dev_host(lua: &Lua, config: GithubDevHostConfig) -> mlua::Result<()> {
    let provider_config = GithubProviderHostConfig {
        cache_db_path: config.cache_db_path.clone(),
        api_base_url: config.api_base_url.clone(),
        mode: config.mode,
        releases_json: config.releases_json.clone(),
        default_include_prereleases: config.default_include_prereleases,
        manifest: None,
    };
    let provider_calls = Rc::clone(&config.provider_calls);
    let release_candidates = lua.create_function(move |lua, spec: Table| {
        let result = github_release_candidates_envelope(lua, spec, provider_config.clone())?;
        provider_calls
            .borrow_mut()
            .push(provider_call_from_envelope(&result)?);
        Ok(result)
    })?;

    let globals = lua.globals();
    let getter = nested_table(lua, globals.clone(), "getter")?;
    let provider = nested_table(lua, getter, "provider")?;
    let github_table = nested_table(lua, provider, "github")?;
    github_table.set("release_candidates", release_candidates.clone())?;

    let getter_builtin = nested_table(lua, globals.clone(), "getter_builtin")?;
    let builtin_provider = nested_table(lua, getter_builtin, "provider")?;
    let builtin_github = nested_table(lua, builtin_provider, "github")?;
    builtin_github.set("release_candidates", release_candidates.clone())?;

    let host = lua.create_table()?;
    host.set(
        "github_release_candidates",
        lua.create_function(move |_, spec: Table| {
            let result: Table = release_candidates.call(spec)?;
            envelope_candidates(result)
        })?,
    )?;
    globals.set("getter_dev", host)
}

fn install_stable_provider_host(
    lua: &Lua,
    fdroid: FdroidProviderHostConfig,
    github: GithubProviderHostConfig,
) -> mlua::Result<()> {
    let fdroid_update_candidates = lua.create_function(move |lua, spec: Table| {
        fdroid_update_candidates_envelope(lua, spec, fdroid.clone())
    })?;
    let github_release_candidates = lua.create_function(move |lua, spec: Table| {
        github_release_candidates_envelope(lua, spec, github.clone())
    })?;

    let globals = lua.globals();
    let getter = nested_table(lua, globals.clone(), "getter")?;
    let provider = nested_table(lua, getter, "provider")?;
    let fdroid_table = nested_table(lua, provider.clone(), "fdroid")?;
    fdroid_table.set("update_candidates", fdroid_update_candidates.clone())?;
    let github_table = nested_table(lua, provider, "github")?;
    github_table.set("release_candidates", github_release_candidates.clone())?;

    let getter_builtin = nested_table(lua, globals, "getter_builtin")?;
    let builtin_provider = nested_table(lua, getter_builtin, "provider")?;
    let builtin_fdroid_table = nested_table(lua, builtin_provider.clone(), "fdroid")?;
    builtin_fdroid_table.set("update_candidates", fdroid_update_candidates)?;
    let builtin_github_table = nested_table(lua, builtin_provider, "github")?;
    builtin_github_table.set("release_candidates", github_release_candidates)
}

fn nested_table(lua: &Lua, parent: Table, key: &str) -> mlua::Result<Table> {
    match parent.get::<LuaValue>(key)? {
        LuaValue::Table(table) => Ok(table),
        LuaValue::Nil => {
            let table = lua.create_table()?;
            parent.set(key, table.clone())?;
            Ok(table)
        }
        _ => Err(mlua::Error::external(format!("{key} must be a table"))),
    }
}

fn install_provider_call_trace(
    lua: &Lua,
    provider_calls: Rc<RefCell<Vec<Value>>>,
) -> mlua::Result<()> {
    let globals = lua.globals();
    let getter: Table = globals.get("getter")?;
    let provider: Table = getter.get("provider")?;
    let fdroid_table: Table = provider.get("fdroid")?;
    let original_fdroid: mlua::Function = fdroid_table.get("update_candidates")?;
    fdroid_table.set(
        "update_candidates",
        lua.create_function({
            let provider_calls = Rc::clone(&provider_calls);
            move |_, spec: Table| {
                let result: Table = original_fdroid.call(spec)?;
                provider_calls
                    .borrow_mut()
                    .push(provider_call_from_envelope(&result)?);
                Ok(result)
            }
        })?,
    )?;

    let github_table: Table = provider.get("github")?;
    let original_github: mlua::Function = github_table.get("release_candidates")?;
    github_table.set(
        "release_candidates",
        lua.create_function(move |_, spec: Table| {
            let result: Table = original_github.call(spec)?;
            provider_calls
                .borrow_mut()
                .push(provider_call_from_envelope(&result)?);
            Ok(result)
        })?,
    )
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

fn fdroid_update_candidates_envelope(
    lua: &Lua,
    spec: Table,
    config: FdroidProviderHostConfig,
) -> mlua::Result<Table> {
    let request = FdroidUpdateHostRequest::from_lua_table(&spec)?;
    let db = CacheDb::open(&config.cache_db_path).map_err(mlua::Error::external)?;
    let endpoint = config.endpoint;
    let refresh_fixture = config.index_xml;
    let manifest = config.manifest.clone();
    let result = read_or_refresh_fdroid_catalog(&db, endpoint, config.mode, || {
        let xml = refresh_fixture
            .ok_or_else(|| "fixture-backed F-Droid catalog host requires index_xml".to_owned())?;
        require_manifest_body(&manifest, FDROID_PROVIDER_ID, &xml)
            .map_err(|source| source.to_string())?;
        Ok(xml)
    })
    .map_err(mlua::Error::external)?;
    reject_unproven_manifest_cache(
        &manifest,
        FDROID_PROVIDER_ID,
        result.source,
        &result.source_response_sha512,
        result.provenance_schema_version.as_deref(),
    )?;
    let mut diagnostics = result
        .diagnostics
        .iter()
        .map(provider_diagnostic_json)
        .collect::<Vec<_>>();
    let candidates = result
        .app(&request.package_name)
        .map(|app| {
            let endpoint = FdroidEndpoint {
                name: result.catalog.endpoint.name.clone(),
                url: Some(result.endpoint.endpoint_url.clone()),
                timestamp: result.catalog.endpoint.timestamp.clone(),
            };
            app.update_candidates(&endpoint)
        })
        .unwrap_or_else(|| {
            diagnostics.push(json!({
                "code": FDROID_PACKAGE_NOT_FOUND,
                "message": format!("F-Droid package '{}' was not found in endpoint '{}'", request.package_name, result.endpoint.endpoint_id),
                "provider": FDROID_PROVIDER_ID,
                "endpoint_id": result.endpoint.endpoint_id,
                "package_name": request.package_name,
                "cache_key": result.cache_key,
            }));
            Vec::new()
        });
    let envelope = provider_envelope_to_lua(
        lua,
        json!({
            "provider": FDROID_PROVIDER_ID,
            "request": "catalog",
            "endpoint_id": result.endpoint.endpoint_id,
            "endpoint_url": result.endpoint.endpoint_url,
            "package_name": request.package_name,
            "cache_key": result.cache_key,
            "source": provider_source_json(result.source),
            "candidates": candidates,
            "diagnostics": diagnostics,
        }),
    )?;
    Ok(envelope)
}

fn github_release_candidates_envelope(
    lua: &Lua,
    spec: Table,
    config: GithubProviderHostConfig,
) -> mlua::Result<Table> {
    let request = GithubReleaseHostRequest::from_lua_table(&spec)?;
    let db = CacheDb::open(&config.cache_db_path).map_err(mlua::Error::external)?;
    let provider_config = GithubReleaseConfig {
        api_base_url: config.api_base_url,
        owner: request.owner,
        repo: request.repo,
    };
    let refresh_fixture = config.releases_json;
    let manifest = config.manifest.clone();
    let result = read_or_refresh_github_releases(&db, provider_config, config.mode, || {
        let releases = refresh_fixture.ok_or_else(|| {
            "fixture-backed GitHub release host requires releases_json".to_owned()
        })?;
        require_manifest_body(&manifest, GITHUB_PROVIDER_ID, &releases)
            .map_err(|source| source.to_string())?;
        Ok(releases)
    })
    .map_err(mlua::Error::external)?;
    reject_unproven_manifest_cache(
        &manifest,
        GITHUB_PROVIDER_ID,
        result.source,
        &result.source_response_sha512,
        result.provenance_schema_version.as_deref(),
    )?;
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
    if candidates.is_empty() && github_eligible_release_count(&result.releases, &options) > 0 {
        diagnostics.push(json!({
            "code": GITHUB_ASSET_NOT_FOUND,
            "message": "no GitHub release assets matched the requested filters",
            "provider": GITHUB_PROVIDER_ID,
            "owner": result.config.owner,
            "repo": result.config.repo,
            "cache_key": result.cache_key,
        }));
    }
    let envelope = provider_envelope_to_lua(
        lua,
        json!({
            "provider": GITHUB_PROVIDER_ID,
            "request": "releases",
            "owner": result.config.owner,
            "repo": result.config.repo,
            "cache_key": result.cache_key,
            "source": provider_source_json(result.source),
            "candidates": candidates,
            "diagnostics": diagnostics,
        }),
    )?;
    Ok(envelope)
}

fn provider_envelope_to_lua(lua: &Lua, mut value: Value) -> mlua::Result<Table> {
    if value
        .get("candidates")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        if let Some(object) = value.as_object_mut() {
            object.remove("candidates");
        }
    }
    json_object_to_lua(lua, value)
}

fn provider_call_from_envelope(envelope: &Table) -> mlua::Result<Value> {
    let mut object = Map::new();
    for key in [
        "provider",
        "request",
        "endpoint_id",
        "endpoint_url",
        "package_name",
        "owner",
        "repo",
        "cache_key",
        "source",
        "diagnostics",
    ] {
        let value: LuaValue = envelope.get(key)?;
        if !matches!(value, LuaValue::Nil) {
            object.insert(key.to_owned(), lua_value_to_json(value)?);
        }
    }
    Ok(Value::Object(object))
}

fn envelope_candidates(envelope: Table) -> mlua::Result<LuaValue> {
    envelope.get("candidates")
}

fn json_object_to_lua(lua: &Lua, value: Value) -> mlua::Result<Table> {
    match json_value_to_lua(lua, value)? {
        LuaValue::Table(table) => Ok(table),
        _ => Err(mlua::Error::external(
            "provider host envelope must be an object",
        )),
    }
}

fn json_value_to_lua(lua: &Lua, value: Value) -> mlua::Result<LuaValue> {
    match value {
        Value::Null => Ok(LuaValue::Nil),
        Value::Bool(value) => Ok(LuaValue::Boolean(value)),
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                return Ok(LuaValue::Integer(value));
            }
            if let Some(value) = number.as_u64().and_then(|value| i64::try_from(value).ok()) {
                return Ok(LuaValue::Integer(value));
            }
            Ok(LuaValue::Number(number.as_f64().ok_or_else(|| {
                mlua::Error::external(format!(
                    "provider host number {number} cannot be represented"
                ))
            })?))
        }
        Value::String(value) => lua.create_string(value).map(LuaValue::String),
        Value::Array(values) => {
            let table = lua.create_table()?;
            for (index, value) in values.into_iter().enumerate() {
                table.raw_set(index + 1, json_value_to_lua(lua, value)?)?;
            }
            Ok(LuaValue::Table(table))
        }
        Value::Object(values) => {
            let table = lua.create_table()?;
            for (key, value) in values {
                if !value.is_null() {
                    table.set(key, json_value_to_lua(lua, value)?)?;
                }
            }
            Ok(LuaValue::Table(table))
        }
    }
}

fn lua_value_to_json(value: LuaValue) -> mlua::Result<Value> {
    match value {
        LuaValue::Nil => Ok(Value::Null),
        LuaValue::Boolean(value) => Ok(Value::Bool(value)),
        LuaValue::Integer(value) => Ok(json!(value)),
        LuaValue::Number(value) => Ok(json!(value)),
        LuaValue::String(value) => Ok(Value::String(value.to_str()?.to_owned())),
        LuaValue::Table(table) => lua_table_to_json_value(table),
        _ => Err(mlua::Error::external(
            "provider host trace contains unsupported Lua value",
        )),
    }
}

fn lua_table_to_json_value(table: Table) -> mlua::Result<Value> {
    let mut len = 0;
    for pair in table.clone().pairs::<LuaValue, LuaValue>() {
        let (key, _) = pair?;
        if let LuaValue::Integer(index) = key {
            if index > 0 {
                len = len.max(index as usize);
                continue;
            }
        }
        let mut object = Map::new();
        for pair in table.pairs::<LuaValue, LuaValue>() {
            let (key, value) = pair?;
            let key = match key {
                LuaValue::String(value) => value.to_str()?.to_owned(),
                LuaValue::Integer(value) => value.to_string(),
                _ => {
                    return Err(mlua::Error::external(
                        "provider host trace table keys must be strings or positive integers",
                    ))
                }
            };
            object.insert(key, lua_value_to_json(value)?);
        }
        return Ok(Value::Object(object));
    }
    let mut array = Vec::with_capacity(len);
    for index in 1..=len {
        array.push(lua_value_to_json(table.raw_get(index)?)?);
    }
    Ok(Value::Array(array))
}

#[derive(Debug, Clone)]
struct PackageManifest {
    sha512: BTreeSet<String>,
}

impl PackageManifest {
    fn load(path: impl AsRef<Path>) -> Result<Self, LuaProviderHostOperationError> {
        let path = path.as_ref();
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    sha512: BTreeSet::new(),
                })
            }
            Err(source) => {
                return Err(LuaProviderHostOperationError::ReadManifest {
                    path: path.to_path_buf(),
                    source,
                })
            }
        };
        let mut sha512 = BTreeSet::new();
        for (line_index, raw_line) in source.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }
            let hash = line
                .split_whitespace()
                .next()
                .expect("non-empty line has token");
            if !is_sha512_hex(hash) {
                return Err(LuaProviderHostOperationError::InvalidManifest {
                    path: path.to_path_buf(),
                    reason: format!(
                        "line {} must start with a 128-character SHA-512 hex digest",
                        line_index + 1
                    ),
                });
            }
            sha512.insert(hash.to_ascii_lowercase());
        }
        Ok(Self { sha512 })
    }

    fn allows_body(&self, body: &str) -> bool {
        self.allows_sha512(&sha512_hex(body.as_bytes()))
    }

    fn allows_sha512(&self, digest: &str) -> bool {
        is_sha512_hex(digest) && self.sha512.contains(&digest.to_ascii_lowercase())
    }
}

fn is_sha512_hex(value: &str) -> bool {
    value.len() == 128 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha512_hex(body: &[u8]) -> String {
    let mut hasher = Sha512::new();
    hasher.update(body);
    format!("{:x}", hasher.finalize())
}

fn require_manifest_body(
    manifest: &Option<PackageManifest>,
    provider: &str,
    body: &str,
) -> mlua::Result<()> {
    let Some(manifest) = manifest else {
        return Ok(());
    };
    if manifest.allows_body(body) {
        return Ok(());
    }
    Err(mlua::Error::external(format!(
        "{PROVIDER_RESPONSE_NOT_IN_MANIFEST}: {provider} provider response body is not listed in package Manifest"
    )))
}

fn reject_unproven_manifest_cache(
    manifest: &Option<PackageManifest>,
    provider: &str,
    source: ProviderCacheSource,
    source_response_sha512: &[String],
    provenance_schema_version: Option<&str>,
) -> mlua::Result<()> {
    let Some(manifest) = manifest else {
        return Ok(());
    };
    if source == ProviderCacheSource::Refreshed {
        return Ok(());
    }
    if provenance_schema_version != Some(PROVIDER_RESPONSE_PROVENANCE_SCHEMA_V1)
        || source_response_sha512.is_empty()
    {
        return Err(mlua::Error::external(format!(
            "{PROVIDER_CACHE_PROVENANCE_MISSING}: {provider} provider cache lacks Manifest-compatible source response provenance"
        )));
    }
    if source_response_sha512
        .iter()
        .all(|digest| manifest.allows_sha512(digest))
    {
        return Ok(());
    }
    Err(mlua::Error::external(format!(
        "{PROVIDER_RESPONSE_NOT_IN_MANIFEST}: {provider} provider cache source response digest is not listed in package Manifest"
    )))
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
            mlua::Error::external(format!("GitHub release provider host missing {field}"))
        })
}

fn optional_lua_bool(table: &Table, field: &'static str) -> mlua::Result<Option<bool>> {
    match table.get::<LuaValue>(field)? {
        LuaValue::Nil => Ok(None),
        LuaValue::Boolean(value) => Ok(Some(value)),
        _ => Err(mlua::Error::external(format!(
            "GitHub release provider host {field} must be a boolean"
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
            "GitHub release provider host asset must be a table",
        )),
    }
}

fn optional_lua_string(table: &Table, field: &'static str) -> mlua::Result<Option<String>> {
    match table.get::<LuaValue>(field)? {
        LuaValue::Nil => Ok(None),
        LuaValue::String(value) => Ok(Some(value.to_str()?.to_owned())),
        _ => Err(mlua::Error::external(format!(
            "GitHub release provider host asset.{field} must be a string"
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
    use getter_storage::{CacheDb, ProviderResponseUpsert};
    use serde_json::json;
    use std::fs;

    const FDROID_INDEX_FIXTURE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
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
  </application>
</fdroid>
"#;

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
    fn fdroid_package_eval_uses_builtin_luaclass_and_provider_cache() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        write_fdroid_package_fixture(data_dir, "org.fdroid.fdroid");

        let first = fdroid_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/f-droid/app/org.fdroid.fdroid",
                "index_xml": FDROID_INDEX_FIXTURE
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(first["operation"], "fdroid.package_eval.fixture");
        assert_eq!(first["provider_calls"][0]["source"], "refreshed");
        assert_eq!(first["provider_calls"][0]["endpoint_id"], "official");
        assert_eq!(
            first["provider_calls"][0]["package_name"],
            "org.fdroid.fdroid"
        );
        assert_eq!(first["package"]["name"], "F-Droid");
        assert_eq!(first["package"]["source_priority"], json!(["fdroid"]));
        assert_eq!(first["package"]["updates"][0]["version"], "1.20.0");
        assert_eq!(first["package"]["updates"][0]["version_code"], 1020000);
        assert_eq!(first["package"]["updates"][0]["source"], "fdroid");
        assert_eq!(
            first["package"]["updates"][0]["artifacts"][0]["url"],
            "https://f-droid.org/repo/org.fdroid.fdroid_1020000.apk"
        );
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

        let second = fdroid_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/f-droid/app/org.fdroid.fdroid"
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(second["provider_calls"][0]["source"], "cache");
        assert_eq!(second["package"]["updates"][0]["version"], "1.20.0");
    }

    #[test]
    fn fdroid_package_eval_reports_package_not_found_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        write_fdroid_package_fixture(data_dir, "missing.package");

        let result = fdroid_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/f-droid/app/org.fdroid.fdroid",
                "index_xml": FDROID_INDEX_FIXTURE
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(result["package"]["updates"], json!([]));
        assert_eq!(
            result["provider_calls"][0]["diagnostics"][0]["code"],
            FDROID_PACKAGE_NOT_FOUND
        );
        assert_eq!(
            result["provider_calls"][0]["diagnostics"][0]["provider"],
            FDROID_PROVIDER_ID
        );
        assert_eq!(
            result["provider_calls"][0]["diagnostics"][0]["package_name"],
            "missing.package"
        );
    }

    #[test]
    fn github_package_eval_uses_builtin_luaclass_and_provider_cache() {
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

    #[test]
    fn stable_provider_host_exists_only_in_provider_backed_operation() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let repo_root = data_dir.join("repo/official");
        let package_dir = repo_root.join("android/app/org.fdroid.fdroid");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{
  "type": "android:app",
  "android": { "package_name": "org.fdroid.fdroid" }
}"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("9999.lua"),
            r#"#!/bin/upa-lua v1
local provider_type = type(getter and getter.provider)
local builtin_provider_type = type(getter_builtin and getter_builtin.provider)
return package_version {
  name = provider_type .. ":" .. builtin_provider_type,
  updates = {
    {
      version = "1.0.0",
      artifacts = {
        { name = "placeholder", url = "https://example.invalid/placeholder.apk" },
      },
    },
  },
}
"#,
        )
        .unwrap();
        register_official_repository(data_dir, &repo_root);
        let layout = RepositoryPackageDirectoryLayout::load(&repo_root).unwrap();
        let package = layout
            .package(&"android/app/org.fdroid.fdroid".parse().unwrap())
            .unwrap();
        let metadata = layout.package_metadata(package).unwrap();
        let script = layout.unambiguous_version_script(package).unwrap();
        let plain = evaluate_package_directory_script_with_host_bindings(
            &RepositoryId::new("official").unwrap(),
            package,
            &metadata,
            script,
            |_| Ok(()),
        )
        .unwrap();

        assert_eq!(plain.name, "nil:nil");

        let stable = stable_provider_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/app/org.fdroid.fdroid"
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(stable["package"]["name"], "table:table");
    }

    #[test]
    fn stable_provider_host_evaluates_fdroid_release_envelope() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        write_stable_provider_package_fixture(
            data_dir,
            "android/f-droid/app/org.fdroid.fdroid",
            stable_fdroid_script("org.fdroid.fdroid"),
            Some(FDROID_INDEX_FIXTURE),
            false,
        );

        let result = stable_provider_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/f-droid/app/org.fdroid.fdroid",
                "fdroid_index_xml": FDROID_INDEX_FIXTURE
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(result["operation"], "provider.package_eval.fixture");
        assert_eq!(result["provider_calls"][0]["provider"], FDROID_PROVIDER_ID);
        assert_eq!(result["provider_calls"][0]["request"], "catalog");
        assert_eq!(result["provider_calls"][0]["source"], "refreshed");
        assert_eq!(result["package"]["updates"][0]["version"], "1.20.0");
        assert_eq!(result["package"]["updates"][0]["version_code"], 1020000);
        assert_eq!(
            result["package"]["updates"][0]["artifacts"][0]["sha256"],
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert!(result["package"]["updates"].is_array());
        assert_eq!(result["runtime_hooks"], json!([]));
        assert!(CacheDb::open(data_dir.join("cache.db"))
            .unwrap()
            .provider_response(result["provider_calls"][0]["cache_key"].as_str().unwrap())
            .unwrap()
            .is_some());
    }

    #[test]
    fn stable_provider_host_evaluates_github_release_envelope() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        write_stable_provider_package_fixture(
            data_dir,
            "android/app/org.fdroid.fdroid",
            stable_github_script("[.]apk$"),
            Some(GITHUB_RELEASES_FIXTURE),
            false,
        );

        let result = stable_provider_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/app/org.fdroid.fdroid",
                "github_releases_json": GITHUB_RELEASES_FIXTURE
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(result["provider_calls"][0]["provider"], GITHUB_PROVIDER_ID);
        assert_eq!(result["provider_calls"][0]["request"], "releases");
        assert_eq!(result["package"]["updates"][0]["version"], "v1.20.0");
        assert_eq!(result["package"]["updates"][0]["source"], "github");
        assert_eq!(
            result["package"]["updates"][0]["artifacts"][0]["file_name"],
            "F-Droid.apk"
        );
    }

    #[test]
    fn stable_provider_host_omits_no_candidate_updates() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        write_stable_provider_package_fixture(
            data_dir,
            "android/f-droid/app/org.fdroid.fdroid",
            stable_fdroid_script("missing.package"),
            Some(FDROID_INDEX_FIXTURE),
            false,
        );

        let result = stable_provider_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/f-droid/app/org.fdroid.fdroid",
                "fdroid_index_xml": FDROID_INDEX_FIXTURE
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(result["package"]["updates"], json!([]));
        assert_eq!(
            result["provider_calls"][0]["diagnostics"][0]["code"],
            FDROID_PACKAGE_NOT_FOUND
        );
    }

    #[test]
    fn stable_provider_builtin_fdroid_luaclass_calls_stable_host() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        write_stable_provider_package_fixture(
            data_dir,
            "android/f-droid/app/org.fdroid.fdroid",
            r#"#!/bin/upa-lua v1
local fdroid = require("luaclass.fdroid_android")
return fdroid.package {
  package_name = "org.fdroid.fdroid",
}
"#,
            Some(FDROID_INDEX_FIXTURE),
            false,
        );

        let result = stable_provider_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/f-droid/app/org.fdroid.fdroid",
                "fdroid_index_xml": FDROID_INDEX_FIXTURE
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(result["provider_calls"][0]["provider"], FDROID_PROVIDER_ID);
        assert_eq!(result["provider_calls"][0]["request"], "catalog");
        assert_eq!(result["package"]["source_priority"], json!(["fdroid"]));
        assert_eq!(result["package"]["updates"][0]["version"], "1.20.0");
        assert_eq!(result["package"]["updates"][0]["source"], "fdroid");
    }

    #[test]
    fn stable_provider_builtin_github_luaclass_calls_stable_host() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        write_stable_provider_package_fixture(
            data_dir,
            "android/app/org.fdroid.fdroid",
            r#"#!/bin/upa-lua v1
local github_android = require("luaclass.github_android_apk")
return github_android.package {
  name = "F-Droid",
  android_package = "org.fdroid.fdroid",
  owner = "f-droid",
  repo = "fdroidclient",
  asset = { include = "[.]apk$" },
}
"#,
            Some(GITHUB_RELEASES_FIXTURE),
            false,
        );

        let result = stable_provider_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/app/org.fdroid.fdroid",
                "github_releases_json": GITHUB_RELEASES_FIXTURE
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(result["provider_calls"][0]["provider"], GITHUB_PROVIDER_ID);
        assert_eq!(result["provider_calls"][0]["request"], "releases");
        assert_eq!(result["package"]["name"], "F-Droid");
        assert_eq!(
            result["package"]["installed"][0]["package_name"],
            "org.fdroid.fdroid"
        );
        assert_eq!(result["package"]["source_priority"], json!(["github"]));
        assert_eq!(result["package"]["updates"][0]["version"], "v1.20.0");
        assert_eq!(result["package"]["updates"][0]["source"], "github");
    }

    #[test]
    fn stable_provider_host_does_not_install_latest_commit_shape() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        write_stable_provider_package_fixture(
            data_dir,
            "android/app/org.fdroid.fdroid",
            r#"#!/bin/upa-lua v1
local latest_commit = getter.provider.github.latest_commit
return package_version {
  name = type(latest_commit),
  updates = {
    {
      version = "1.0.0",
      artifacts = {
        { name = "placeholder", url = "https://example.invalid/placeholder.apk" },
      },
    },
  },
}
"#,
            None,
            false,
        );

        let result = stable_provider_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/app/org.fdroid.fdroid"
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(result["package"]["name"], "nil");
    }

    #[test]
    fn stable_provider_hook_wraps_provider_function_and_calls_builtin_original() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        write_stable_provider_package_fixture(
            data_dir,
            "android/f-droid/app/org.fdroid.fdroid",
            stable_fdroid_script("org.fdroid.fdroid"),
            Some(FDROID_INDEX_FIXTURE),
            false,
        );
        write_hook(
            data_dir,
            "10-provider-tag.lua",
            r#"
local original = getter_builtin.provider.fdroid.update_candidates
function getter.provider.fdroid.update_candidates(spec)
  local result = original(spec)
  result.source = "hooked-" .. result.source
  return result
end
"#,
        );

        let result = stable_provider_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/f-droid/app/org.fdroid.fdroid",
                "fdroid_index_xml": FDROID_INDEX_FIXTURE
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(result["provider_calls"][0]["source"], "hooked-refreshed");
        assert_eq!(result["package"]["updates"][0]["version"], "1.20.0");
        assert_eq!(result["runtime_hooks"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn stable_provider_host_requires_manifest_listed_fixture_body() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        write_stable_provider_package_fixture(
            data_dir,
            "android/f-droid/app/org.fdroid.fdroid",
            stable_fdroid_script("org.fdroid.fdroid"),
            None,
            false,
        );

        let err = stable_provider_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/f-droid/app/org.fdroid.fdroid",
                "fdroid_index_xml": FDROID_INDEX_FIXTURE
            })
            .to_string(),
        )
        .unwrap_err();

        assert!(err.to_string().contains(PROVIDER_RESPONSE_NOT_IN_MANIFEST));
    }

    #[test]
    fn stable_provider_host_uses_manifest_compatible_cache_provenance() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        write_stable_provider_package_fixture(
            data_dir,
            "android/f-droid/app/org.fdroid.fdroid",
            stable_fdroid_script("org.fdroid.fdroid"),
            Some(FDROID_INDEX_FIXTURE),
            false,
        );
        stable_provider_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/f-droid/app/org.fdroid.fdroid",
                "fdroid_index_xml": FDROID_INDEX_FIXTURE
            })
            .to_string(),
        )
        .unwrap();

        let result = stable_provider_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/f-droid/app/org.fdroid.fdroid"
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(result["provider_calls"][0]["source"], "cache");
        assert_eq!(result["package"]["updates"][0]["version"], "1.20.0");
    }

    #[test]
    fn stable_provider_host_rejects_non_free_cache_hit_without_provenance() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        write_stable_provider_package_fixture(
            data_dir,
            "android/f-droid/app/org.fdroid.fdroid",
            stable_fdroid_script("org.fdroid.fdroid"),
            Some(FDROID_INDEX_FIXTURE),
            false,
        );
        write_fdroid_cache_response(data_dir, Vec::new(), None);

        let err = stable_provider_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/f-droid/app/org.fdroid.fdroid"
            })
            .to_string(),
        )
        .unwrap_err();

        assert!(err.to_string().contains(PROVIDER_CACHE_PROVENANCE_MISSING));
    }

    #[test]
    fn stable_provider_host_rejects_cache_provenance_digest_not_in_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        write_stable_provider_package_fixture(
            data_dir,
            "android/f-droid/app/org.fdroid.fdroid",
            stable_fdroid_script("org.fdroid.fdroid"),
            Some(FDROID_INDEX_FIXTURE),
            false,
        );
        write_fdroid_cache_response(
            data_dir,
            vec![sha512_hex(b"different provider body")],
            Some(PROVIDER_RESPONSE_PROVENANCE_SCHEMA_V1.to_owned()),
        );

        let err = stable_provider_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/f-droid/app/org.fdroid.fdroid"
            })
            .to_string(),
        )
        .unwrap_err();

        assert!(err.to_string().contains(PROVIDER_RESPONSE_NOT_IN_MANIFEST));
    }

    #[test]
    fn stable_provider_host_manifest_check_does_not_apply_to_free_network_script() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        write_stable_provider_package_fixture(
            data_dir,
            "android/f-droid/app/org.fdroid.fdroid",
            stable_fdroid_script("org.fdroid.fdroid"),
            None,
            true,
        );

        let result = stable_provider_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/f-droid/app/org.fdroid.fdroid",
                "fdroid_index_xml": FDROID_INDEX_FIXTURE
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(result["package"]["updates"][0]["version"], "1.20.0");
    }

    #[test]
    fn provider_package_eval_prefers_repository_luaclass_over_builtin() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        write_fdroid_package_fixture(data_dir, "org.fdroid.fdroid");
        let repo_luaclass = data_dir.join("repo/official/luaclass");
        fs::create_dir_all(&repo_luaclass).unwrap();
        fs::write(
            repo_luaclass.join("fdroid_android.lua"),
            r#"
local fdroid = {}

function fdroid.package(spec)
  if spec.package_name ~= "org.fdroid.fdroid" then
    error("repository override package_name mismatch")
  end
  return package_version {
    name = "repository override",
    source_priority = { "repository-override" },
    updates = {
      {
        version = "9.9.9",
        source = "repository-override",
        artifacts = {
          { name = "override.apk", url = "https://example.invalid/override.apk" },
        },
      },
    },
  }
end

return fdroid
"#,
        )
        .unwrap();

        let result = fdroid_package_eval_json(
            data_dir,
            &json!({
                "repository_id": "official",
                "package_id": "android/f-droid/app/org.fdroid.fdroid"
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(result["provider_calls"], json!([]));
        assert_eq!(result["package"]["name"], "repository override");
        assert_eq!(
            result["package"]["source_priority"],
            json!(["repository-override"])
        );
        assert_eq!(result["package"]["updates"][0]["version"], "9.9.9");
        assert_eq!(
            result["package"]["updates"][0]["source"],
            "repository-override"
        );
    }

    fn write_fdroid_package_fixture(data_dir: &std::path::Path, package_name: &str) {
        let repo_root = data_dir.join("repo/official");
        let package_dir = repo_root.join("android/f-droid/app/org.fdroid.fdroid");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{
  "display_name": "F-Droid",
  "type": "android:app",
  "android": { "package_name": "org.fdroid.fdroid" }
}"#,
        )
        .unwrap();
        let version_script = r#"#!/bin/upa-lua v1
local fdroid = require("luaclass.fdroid_android")
return fdroid.package {
  package_name = "__PACKAGE_NAME__",
}
"#
        .replace("__PACKAGE_NAME__", package_name);
        fs::write(package_dir.join("9999.lua"), version_script).unwrap();
        register_official_repository(data_dir, &repo_root);
    }

    fn write_github_package_fixture(data_dir: &std::path::Path, asset_include: &str) {
        let repo_root = data_dir.join("repo/official");
        let package_dir = repo_root.join("android/app/org.fdroid.fdroid");
        fs::create_dir_all(&package_dir).unwrap();
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
        register_official_repository(data_dir, &repo_root);
    }

    fn write_stable_provider_package_fixture(
        data_dir: &std::path::Path,
        package_id: &str,
        version_script: impl AsRef<str>,
        manifest_body: Option<&str>,
        allow_free_network: bool,
    ) {
        let repo_root = data_dir.join("repo/official");
        let package_dir = repo_root.join(package_id);
        fs::create_dir_all(&package_dir).unwrap();
        let mut metadata = json!({
            "type": "android:app",
            "android": { "package_name": "org.fdroid.fdroid" }
        });
        if allow_free_network {
            metadata["lua"] = json!({
                "9999.lua": { "permission": ["allow_free_network"] }
            });
        }
        fs::write(
            package_dir.join("metadata.jsonc"),
            serde_json::to_string_pretty(&metadata).unwrap(),
        )
        .unwrap();
        if let Some(manifest_body) = manifest_body {
            fs::write(
                package_dir.join(PACKAGE_MANIFEST_FILE),
                format!("{} fixture-body\n", sha512_hex(manifest_body.as_bytes())),
            )
            .unwrap();
        }
        fs::write(package_dir.join("9999.lua"), version_script.as_ref()).unwrap();
        register_official_repository(data_dir, &repo_root);
    }

    fn stable_fdroid_script(package_name: &str) -> String {
        format!(
            r#"#!/bin/upa-lua v1
local result = getter.provider.fdroid.update_candidates {{
  package_name = "{package_name}",
}}
return package_version {{
  source_priority = {{ "fdroid" }},
  updates = result.candidates,
}}
"#
        )
    }

    fn stable_github_script(asset_include: &str) -> String {
        format!(
            r#"#!/bin/upa-lua v1
local result = getter.provider.github.release_candidates {{
  owner = "f-droid",
  repo = "fdroidclient",
  asset = {{ include = "{asset_include}" }},
}}
return package_version {{
  name = "F-Droid",
  source_priority = {{ "github" }},
  updates = result.candidates,
}}
"#
        )
    }

    fn write_fdroid_cache_response(
        data_dir: &std::path::Path,
        source_response_sha512: Vec<String>,
        provenance_schema_version: Option<String>,
    ) {
        let endpoint = FdroidEndpointConfig::default();
        let catalog = getter_providers::parse_fdroid_index_xml(FDROID_INDEX_FIXTURE).unwrap();
        CacheDb::open(data_dir.join("cache.db"))
            .unwrap()
            .upsert_provider_response(&ProviderResponseUpsert {
                cache_key: endpoint.cache_key(),
                provider: FDROID_PROVIDER_ID.to_owned(),
                response_json: serde_json::to_value(catalog).unwrap(),
                source_response_sha512,
                provenance_schema_version,
                freshness_json: json!({}),
            })
            .unwrap();
    }

    fn write_hook(data_dir: &std::path::Path, file_name: &str, source: &str) {
        let hook_dir = data_dir.join("rc/hook");
        fs::create_dir_all(&hook_dir).unwrap();
        fs::write(hook_dir.join(file_name), source).unwrap();
    }

    fn register_official_repository(data_dir: &std::path::Path, repo_root: &std::path::Path) {
        let main_db = MainDb::open(data_dir.join("main.db")).unwrap();
        main_db
            .upsert_repository(
                &RepositoryMetadata {
                    id: RepositoryId::new("official").unwrap(),
                    name: "Official".to_owned(),
                    priority: RepositoryPriority::DEFAULT,
                    api_version: REPO_API_VERSION_V1.to_owned(),
                },
                Some(repo_root),
                None,
            )
            .unwrap();
    }
}
