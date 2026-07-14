//! Shared JSON operation helpers for the in-memory Phase D getter runtime.
//!
//! These functions are intentionally thin over [`getter_core::runtime`]. They
//! give embedders (native bridge, future single-process debug CLI, tests) one
//! place to map JSON requests into runtime controls without reintroducing
//! persisted task state or duplicating task/control semantics in Flutter or
//! Android glue.

use crate::download::{
    retry_download_task, submit_action_and_download, RuntimeDownloadOperationError,
    RuntimeDownloadTransport, UreqRuntimeDownloadTransport,
};
#[cfg(feature = "lua")]
use crate::github_releases::{GithubReleaseTransport, UreqGithubReleaseTransport};
#[cfg(feature = "lua")]
use crate::lua_provider_host::{
    evaluate_provider_backed_package, LuaProviderHostOperationError,
    ProviderBackedPackageEvalConfig,
};
#[cfg(feature = "lua")]
use crate::provider_cache::{ProviderCacheDiagnostic, ProviderCacheMode};
#[cfg(feature = "lua")]
use getter_core::repository::{
    package_directory_cache_key, RepositoryLoadError, RepositoryPackageDirectoryLayout,
};
use getter_core::{
    runtime::{
        GetterRuntime, IssuedAction, PackageVersionLuaObject, RuntimeError, SealedActionPlan,
        TaskCleanMode, TaskSnapshot, UserResult,
    },
    update::{run_offline_update_check, OfflineUpdateCheckError, OfflineUpdateCheckFixture},
    PackageId,
};
#[cfg(feature = "lua")]
use getter_core::{update::check_updates_offline, update::UpdateSelectionPolicy, RepositoryId};
#[cfg(feature = "lua")]
use getter_providers::StaticPackageUpdatesProvider;
#[cfg(feature = "lua")]
use getter_storage::{MainDb, StorageError};
use serde::Deserialize;
#[cfg(feature = "lua")]
use serde::Serialize;
use serde_json::{json, Value};
#[cfg(feature = "lua")]
use std::path::PathBuf;
#[cfg(feature = "lua")]
use std::rc::Rc;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeOperationError {
    #[error("invalid runtime request: {0}")]
    InvalidRequest(String),
    #[error("runtime error: {0}")]
    Runtime(#[from] RuntimeError),
    #[error("update check failed: {0}")]
    UpdateCheck(#[from] OfflineUpdateCheckError),
    #[cfg(feature = "lua")]
    #[error("storage operation failed: {0}")]
    Storage(#[from] StorageError),
    #[cfg(feature = "lua")]
    #[error("repository operation failed: {0}")]
    Repository(#[from] RepositoryLoadError),
    #[cfg(feature = "lua")]
    #[error("package evaluation failed: {0}")]
    PackageEval(String),
    #[cfg(feature = "lua")]
    #[error("provider-backed package evaluation failed: {0}")]
    ProviderPackageEval(#[from] LuaProviderHostOperationError),
    #[error("runtime response serialization failed: {0}")]
    Serialize(String),
}

impl From<RuntimeDownloadOperationError> for RuntimeOperationError {
    fn from(error: RuntimeDownloadOperationError) -> Self {
        match error {
            RuntimeDownloadOperationError::Runtime(error) => Self::Runtime(error),
        }
    }
}

impl RuntimeOperationError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "runtime.invalid_request",
            Self::Runtime(error) => error.code(),
            Self::UpdateCheck(_) => "update.check_error",
            #[cfg(feature = "lua")]
            Self::Storage(_) => "storage.error",
            #[cfg(feature = "lua")]
            Self::Repository(_) => "repository.error",
            #[cfg(feature = "lua")]
            Self::PackageEval(_) => "package.eval_error",
            #[cfg(feature = "lua")]
            Self::ProviderPackageEval(error) => match error {
                LuaProviderHostOperationError::InvalidRequest(_) => "runtime.invalid_request",
                LuaProviderHostOperationError::Storage(_) => "storage.error",
                LuaProviderHostOperationError::Repository(_) => "repository.error",
                LuaProviderHostOperationError::PackageEval(_) => "package.eval_error",
                LuaProviderHostOperationError::Fdroid(_) => "provider.fdroid.error",
                LuaProviderHostOperationError::Github(_)
                | LuaProviderHostOperationError::GithubProvider(_) => "provider.github.error",
                LuaProviderHostOperationError::Serialization(_) => "runtime.serialize_error",
                LuaProviderHostOperationError::ReadManifest { .. }
                | LuaProviderHostOperationError::InvalidManifest { .. } => "package.manifest_error",
            },
            Self::Serialize(_) => "runtime.serialize_error",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "Getter runtime request is invalid",
            Self::Runtime(_) => "Getter runtime operation failed",
            Self::UpdateCheck(_) => "Getter update check failed",
            #[cfg(feature = "lua")]
            Self::Storage(_) => "Getter storage operation failed",
            #[cfg(feature = "lua")]
            Self::Repository(_) => "Getter repository operation failed",
            #[cfg(feature = "lua")]
            Self::PackageEval(_) | Self::ProviderPackageEval(_) => {
                "Getter package evaluation failed"
            }
            Self::Serialize(_) => "Getter runtime response serialization failed",
        }
    }

    pub fn detail(&self) -> Option<String> {
        match self {
            Self::InvalidRequest(detail) | Self::Serialize(detail) => Some(detail.clone()),
            Self::Runtime(error) => Some(error.to_string()),
            Self::UpdateCheck(error) => Some(error.to_string()),
            #[cfg(feature = "lua")]
            Self::Storage(error) => Some(error.to_string()),
            #[cfg(feature = "lua")]
            Self::Repository(error) => Some(error.to_string()),
            #[cfg(feature = "lua")]
            Self::PackageEval(detail) => Some(detail.clone()),
            #[cfg(feature = "lua")]
            Self::ProviderPackageEval(error) => Some(error.to_string()),
        }
    }
}

pub fn issue_action(runtime: &mut GetterRuntime, plan: SealedActionPlan) -> Value {
    issued_action_json(runtime.issue_action(plan))
}

pub fn issue_action_from_offline_update_check_json(
    runtime: &mut GetterRuntime,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    let request: OfflineUpdateActionRequest = parse_request(request_json)?;
    let update = run_offline_update_check(request.fixture)?;
    let action = if update.actions.is_empty() {
        None
    } else {
        Some(
            runtime.issue_action(SealedActionPlan {
                package_id: update.package_id.clone(),
                actions: update.actions.clone(),
                lua_object: PackageVersionLuaObject {
                    object_id: format!("offline-update:{}", update.package_id),
                    dependency_digest: request
                        .dependency_digest
                        .unwrap_or_else(|| format!("offline-update:{}", update.package_id)),
                },
            }),
        )
    };
    Ok(json!({
        "update": update,
        "action": action.map(issued_action_json),
    }))
}

#[cfg(feature = "lua")]
pub fn issue_action_from_registered_package_json(
    runtime: &mut GetterRuntime,
    data_dir: &std::path::Path,
    db: &MainDb,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    issue_action_from_registered_package_json_with_github_transport(
        runtime,
        data_dir,
        db,
        request_json,
        Some(Rc::new(UreqGithubReleaseTransport::new())),
    )
}

#[cfg(feature = "lua")]
pub fn issue_action_from_registered_package_json_with_github_transport(
    runtime: &mut GetterRuntime,
    data_dir: &std::path::Path,
    db: &MainDb,
    request_json: &str,
    github_release_transport: Option<Rc<dyn GithubReleaseTransport>>,
) -> Result<Value, RuntimeOperationError> {
    let request: RegisteredPackageUpdateActionRequest = parse_request(request_json)?;
    let result = issue_action_from_registered_package_with_github_transport(
        runtime,
        data_dir,
        db,
        request,
        github_release_transport,
    )?;
    serde_json::to_value(result)
        .map_err(|source| RuntimeOperationError::Serialize(source.to_string()))
}

#[cfg(feature = "lua")]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct RegisteredPackageActionResult {
    pub package: getter_core::ResolvedPackage,
    pub update: getter_core::update::OfflineUpdateCheckResult,
    pub action: Option<IssuedAction>,
    pub provider_calls: Vec<Value>,
    pub runtime_hooks: Vec<PathBuf>,
}

#[cfg(feature = "lua")]
pub(crate) fn issue_action_from_registered_package_with_github_transport(
    runtime: &mut GetterRuntime,
    data_dir: &std::path::Path,
    db: &MainDb,
    request: RegisteredPackageUpdateActionRequest,
    github_release_transport: Option<Rc<dyn GithubReleaseTransport>>,
) -> Result<RegisteredPackageActionResult, RuntimeOperationError> {
    let evaluated = evaluate_registered_package(
        data_dir,
        db,
        &request,
        ProviderCacheMode::UseCached,
        github_release_transport,
    )?;
    issue_action_from_registered_evaluation(runtime, request, evaluated)
}

#[cfg(feature = "lua")]
pub(crate) fn issue_action_from_registered_evaluation(
    runtime: &mut GetterRuntime,
    request: RegisteredPackageUpdateActionRequest,
    evaluated: RegisteredPackageEvaluation,
) -> Result<RegisteredPackageActionResult, RuntimeOperationError> {
    let candidates = StaticPackageUpdatesProvider.check_updates(&evaluated.package);
    let update = check_updates_offline(
        evaluated.package.id.clone(),
        request.installed_version,
        candidates,
        UpdateSelectionPolicy {
            pin_version: request.pin_version,
        },
    )?;
    let action = (!update.actions.is_empty()).then(|| {
        runtime.issue_action(SealedActionPlan {
            package_id: update.package_id.clone(),
            actions: update.actions.clone(),
            lua_object: PackageVersionLuaObject {
                object_id: format!("package-update:{}", update.package_id),
                dependency_digest: evaluated.dependency_digest,
            },
        })
    });
    Ok(RegisteredPackageActionResult {
        package: evaluated.package,
        update,
        action,
        provider_calls: evaluated.provider_calls,
        runtime_hooks: evaluated.runtime_hooks,
    })
}

pub fn submit_action_json(
    runtime: &mut GetterRuntime,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    let request: SubmitActionRequest = parse_request(request_json)?;
    task_json(runtime.submit_action(&request.action_id)?)
}

pub fn submit_action_and_download_json(
    runtime: &mut GetterRuntime,
    data_dir: &std::path::Path,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    submit_action_and_download_json_with_transport(
        runtime,
        data_dir,
        request_json,
        &UreqRuntimeDownloadTransport::new(),
    )
}

pub fn submit_action_and_download_json_with_transport<T>(
    runtime: &mut GetterRuntime,
    data_dir: &std::path::Path,
    request_json: &str,
    transport: &T,
) -> Result<Value, RuntimeOperationError>
where
    T: RuntimeDownloadTransport + ?Sized,
{
    let request: SubmitActionRequest = parse_request(request_json)?;
    task_json(submit_action_and_download(
        runtime,
        data_dir,
        &request.action_id,
        transport,
    )?)
}

pub fn task_get_json(
    runtime: &GetterRuntime,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    let request: TaskIdRequest = parse_request(request_json)?;
    task_json(runtime.task(&request.task_id)?)
}

pub fn task_list_json(
    runtime: &GetterRuntime,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    let request: TaskListRequest = parse_request(request_json)?;
    let tasks = if let Some(package_id) = request.package_id.as_ref() {
        runtime.tasks_for_package(package_id)
    } else if request.active {
        runtime.active_tasks()
    } else {
        runtime.tasks()
    };
    tasks_json(tasks)
}

pub fn task_start_json(
    runtime: &mut GetterRuntime,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    let request: TaskIdRequest = parse_request(request_json)?;
    task_json(runtime.start_task(&request.task_id)?)
}

pub fn task_download_progress_json(
    runtime: &mut GetterRuntime,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    let request: DownloadProgressRequest = parse_request(request_json)?;
    task_json(runtime.set_download_progress(
        &request.task_id,
        request.current_bits,
        request.total_bits,
    )?)
}

pub fn task_complete_download_json(
    runtime: &mut GetterRuntime,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    let request: TaskIdRequest = parse_request(request_json)?;
    task_json(runtime.complete_download(&request.task_id)?)
}

pub fn task_pause_json(
    runtime: &mut GetterRuntime,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    let request: TaskIdRequest = parse_request(request_json)?;
    task_json(runtime.pause_task(&request.task_id)?)
}

pub fn task_resume_json(
    runtime: &mut GetterRuntime,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    let request: TaskIdRequest = parse_request(request_json)?;
    task_json(runtime.resume_task(&request.task_id)?)
}

pub fn task_user_result_json(
    runtime: &mut GetterRuntime,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    let request: UserResultRequest = parse_request(request_json)?;
    task_json(runtime.user_result(&request.task_id, request.result, request.reason.as_deref())?)
}

pub fn task_cancel_json(
    runtime: &mut GetterRuntime,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    let request: TaskIdRequest = parse_request(request_json)?;
    task_json(runtime.cancel_task(&request.task_id)?)
}

pub fn task_retry_json(
    runtime: &mut GetterRuntime,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    let request: TaskIdRequest = parse_request(request_json)?;
    task_json(runtime.retry_task(&request.task_id)?)
}

pub fn task_retry_download_json(
    runtime: &mut GetterRuntime,
    data_dir: &std::path::Path,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    task_retry_download_json_with_transport(
        runtime,
        data_dir,
        request_json,
        &UreqRuntimeDownloadTransport::new(),
    )
}

pub fn task_retry_download_json_with_transport<T>(
    runtime: &mut GetterRuntime,
    data_dir: &std::path::Path,
    request_json: &str,
    transport: &T,
) -> Result<Value, RuntimeOperationError>
where
    T: RuntimeDownloadTransport + ?Sized,
{
    let request: TaskIdRequest = parse_request(request_json)?;
    task_json(retry_download_task(
        runtime,
        data_dir,
        &request.task_id,
        transport,
    )?)
}

pub fn task_remove_json(
    runtime: &mut GetterRuntime,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    let request: TaskIdRequest = parse_request(request_json)?;
    task_json(runtime.remove_task(&request.task_id)?)
}

pub fn task_clean_json(
    runtime: &mut GetterRuntime,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    let request: CleanTasksRequest = parse_request(request_json)?;
    tasks_json(runtime.clean_tasks(request.mode()?))
}

fn parse_request<T>(request_json: &str) -> Result<T, RuntimeOperationError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str(request_json)
        .map_err(|source| RuntimeOperationError::InvalidRequest(source.to_string()))
}

fn issued_action_json(action: IssuedAction) -> Value {
    json!({
        "action_id": action.action_id,
        "package_id": action.package_id,
    })
}

fn task_json(task: TaskSnapshot) -> Result<Value, RuntimeOperationError> {
    serde_json::to_value(task)
        .map_err(|source| RuntimeOperationError::Serialize(source.to_string()))
}

fn tasks_json(tasks: Vec<TaskSnapshot>) -> Result<Value, RuntimeOperationError> {
    Ok(json!({ "tasks": tasks }))
}

#[derive(Debug, Deserialize)]
struct OfflineUpdateActionRequest {
    fixture: OfflineUpdateCheckFixture,
    #[serde(default)]
    dependency_digest: Option<String>,
}

#[cfg(feature = "lua")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegisteredPackageUpdateActionRequest {
    pub package_id: PackageId,
    #[serde(default)]
    pub repository_id: Option<RepositoryId>,
    #[serde(default)]
    pub installed_version: Option<String>,
    #[serde(default, alias = "ignored_version")]
    pub pin_version: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitActionRequest {
    action_id: String,
}

#[derive(Debug, Deserialize)]
struct TaskIdRequest {
    task_id: String,
}

#[derive(Debug, Default, Deserialize)]
struct TaskListRequest {
    #[serde(default)]
    active: bool,
    #[serde(default)]
    package_id: Option<PackageId>,
}

#[derive(Debug, Deserialize)]
struct DownloadProgressRequest {
    task_id: String,
    current_bits: u64,
    #[serde(default)]
    total_bits: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct UserResultRequest {
    task_id: String,
    result: UserResult,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CleanTasksRequest {
    #[serde(default)]
    mode: Option<String>,
}

#[cfg(feature = "lua")]
pub(crate) struct RegisteredPackageEvaluation {
    pub package: getter_core::ResolvedPackage,
    pub diagnostics: Vec<ProviderCacheDiagnostic>,
    dependency_digest: String,
    provider_calls: Vec<Value>,
    runtime_hooks: Vec<PathBuf>,
}

#[cfg(feature = "lua")]
pub(crate) fn evaluate_registered_package(
    data_dir: &std::path::Path,
    db: &MainDb,
    request: &RegisteredPackageUpdateActionRequest,
    cache_mode: ProviderCacheMode,
    github_release_transport: Option<Rc<dyn GithubReleaseTransport>>,
) -> Result<RegisteredPackageEvaluation, RuntimeOperationError> {
    let repositories = db.repositories()?;
    let mut missing_path = None;
    for repository in repositories {
        if request
            .repository_id
            .as_ref()
            .is_some_and(|requested| requested != &repository.id)
        {
            continue;
        }
        let Some(root) = repository.path.as_ref() else {
            missing_path = Some(repository.id.to_string());
            continue;
        };
        let root = PathBuf::from(root);
        let layout = RepositoryPackageDirectoryLayout::load(&root)?;
        let Some(package_directory) = layout.package(&request.package_id) else {
            continue;
        };
        let metadata = layout.package_metadata(package_directory)?;
        let script = layout.unambiguous_version_script(package_directory)?;
        let provider_eval = evaluate_provider_backed_package(
            data_dir,
            &repository.id,
            package_directory,
            &metadata,
            script,
            ProviderBackedPackageEvalConfig {
                mode: cache_mode,
                fdroid_endpoint_id: None,
                fdroid_endpoint_url: None,
                fdroid_index_xml: None,
                github_endpoint_id: None,
                github_api_base_url: None,
                github_releases_json: None,
                github_release_transport: github_release_transport.clone(),
                github_include_prereleases: false,
            },
        )?;
        let cache_key = package_directory_cache_key(&repository.id, package_directory, script)?;
        let dependency_digest = format!(
            "repo:{}:package:{}:hash:{}",
            cache_key.repository_id, cache_key.package_id, cache_key.package_file_hash
        );
        let diagnostics = provider_eval
            .provider_calls
            .iter()
            .filter_map(|call| call.get("diagnostics"))
            .filter_map(Value::as_array)
            .flatten()
            .filter_map(|diagnostic| serde_json::from_value(diagnostic.clone()).ok())
            .collect();
        return Ok(RegisteredPackageEvaluation {
            package: provider_eval.package,
            diagnostics,
            dependency_digest,
            provider_calls: provider_eval.provider_calls,
            runtime_hooks: provider_eval.runtime_hooks,
        });
    }
    let detail = if let Some(repository_id) = request.repository_id.as_ref() {
        format!(
            "package '{}' was not found in registered repository '{}'{}",
            request.package_id,
            repository_id,
            missing_path
                .map(|id| format!("; repository '{id}' has no path"))
                .unwrap_or_default()
        )
    } else {
        format!(
            "package '{}' was not found in any registered repository{}",
            request.package_id,
            missing_path
                .map(|id| format!("; repository '{id}' has no path"))
                .unwrap_or_default()
        )
    };
    Err(RuntimeOperationError::PackageEval(detail))
}

impl CleanTasksRequest {
    fn mode(self) -> Result<TaskCleanMode, RuntimeOperationError> {
        match self.mode.as_deref().unwrap_or("default") {
            "default" => Ok(TaskCleanMode::Default),
            "failed" => Ok(TaskCleanMode::Failed),
            "all_inactive" => Ok(TaskCleanMode::AllInactive),
            other => Err(RuntimeOperationError::InvalidRequest(format!(
                "unsupported task clean mode '{other}'"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "lua")]
    use crate::fdroid_catalog::{FdroidEndpointConfig, FDROID_PROVIDER_ID};
    #[cfg(feature = "lua")]
    use crate::github_releases::{
        GithubReleaseConfig, GithubReleaseTransport, GithubReleaseTransportError,
        GithubReleaseTransportRequest, GithubReleaseTransportResponse, DEFAULT_GITHUB_API_BASE_URL,
        GITHUB_PROVIDER_ID,
    };
    #[cfg(feature = "lua")]
    use crate::provider_cache::PROVIDER_RESPONSE_PROVENANCE_SCHEMA_V1;
    #[cfg(feature = "lua")]
    use getter_core::repository::{RepositoryMetadata, REPO_API_VERSION_V1};
    #[cfg(feature = "lua")]
    use getter_core::RepositoryPriority;
    use getter_core::{
        runtime::RuntimeTaskStatus, update::OFFLINE_UPDATE_CHECK_FORMAT,
        update::OFFLINE_UPDATE_CHECK_VERSION, UpdateAction, UpdateArtifact, UpdateCandidate,
    };
    #[cfg(feature = "lua")]
    use getter_providers::{parse_fdroid_index_xml, parse_github_releases_json};
    #[cfg(feature = "lua")]
    use getter_storage::{CacheDb, ProviderResponseUpsert};
    #[cfg(feature = "lua")]
    use sha2::{Digest, Sha512};
    use std::cell::RefCell;
    use std::fs;
    #[cfg(feature = "lua")]
    use std::rc::Rc;

    #[cfg(feature = "lua")]
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

    #[cfg(feature = "lua")]
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

    #[cfg(feature = "lua")]
    const UPGRADEALL_GITHUB_RELEASES_SNAPSHOT: &str =
        include_str!("../../../tests/files/web/github_api_release.json");

    #[cfg(feature = "lua")]
    const UPGRADEALL_OLD_GITHUB_RELEASES_NORMALIZED: &str =
        include_str!("../../../tests/files/data/provider_github_release.json");

    #[cfg(feature = "lua")]
    #[test]
    fn registered_package_update_check_issues_action_from_package_directory_static_updates() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        write_package_directory_static_update_repo(&repo_root);
        let db = MainDb::open_in_memory().unwrap();
        db.upsert_repository(
            &RepositoryMetadata {
                id: "autogen".parse().unwrap(),
                name: "Autogen".to_owned(),
                priority: RepositoryPriority::new(-1),
                api_version: REPO_API_VERSION_V1.to_owned(),
            },
            Some(&repo_root),
            None,
        )
        .unwrap();
        let mut runtime = GetterRuntime::new();

        let issued = issue_action_from_registered_package_json(
            &mut runtime,
            temp.path(),
            &db,
            &json!({
                "package_id": "android/app/com.example.autogen",
                "installed_version": "1.0.0"
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(issued["package"]["repository"], "autogen");
        assert_eq!(issued["package"]["name"], "Example Autogen");
        assert_eq!(issued["update"]["status"], "update_available");
        assert_eq!(issued["provider_calls"], json!([]));
        assert_eq!(issued["runtime_hooks"], json!([]));
        let action_id = issued["action"]["action_id"].as_str().unwrap();
        let submitted =
            submit_action_json(&mut runtime, &json!({ "action_id": action_id }).to_string())
                .unwrap();
        assert_eq!(submitted["package_id"], "android/app/com.example.autogen");
    }

    #[cfg(feature = "lua")]
    #[test]
    fn registered_package_update_check_without_update_does_not_issue_action() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        write_package_directory_static_update_repo(&repo_root);
        let db = MainDb::open_in_memory().unwrap();
        db.upsert_repository(
            &RepositoryMetadata {
                id: "autogen".parse().unwrap(),
                name: "Autogen".to_owned(),
                priority: RepositoryPriority::new(-1),
                api_version: REPO_API_VERSION_V1.to_owned(),
            },
            Some(&repo_root),
            None,
        )
        .unwrap();
        let mut runtime = GetterRuntime::new();

        let issued = issue_action_from_registered_package_json(
            &mut runtime,
            temp.path(),
            &db,
            &json!({
                "package_id": "android/app/com.example.autogen",
                "installed_version": "1.2.0"
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(issued["update"]["status"], "up_to_date");
        assert!(issued["action"].is_null());
    }

    #[cfg(feature = "lua")]
    #[test]
    fn registered_package_update_check_issues_action_from_fdroid_provider_luaclass() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let repo_root = data_dir.join("repo/official");
        write_fdroid_provider_package_repo(&repo_root, Some(FDROID_INDEX_FIXTURE), false);
        let db = MainDb::open(data_dir.join("main.db")).unwrap();
        register_repository(&db, "official", "Official", 0, &repo_root);
        seed_fdroid_provider_cache(data_dir, FDROID_INDEX_FIXTURE);
        let mut runtime = GetterRuntime::new();

        let issued = issue_action_from_registered_package_json(
            &mut runtime,
            data_dir,
            &db,
            &json!({
                "package_id": "android/f-droid/app/org.fdroid.fdroid",
                "installed_version": "1.0.0"
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(issued["package"]["source_priority"], json!(["fdroid"]));
        assert_eq!(issued["provider_calls"][0]["provider"], "fdroid");
        assert_eq!(issued["provider_calls"][0]["source"], "cache");
        assert_eq!(issued["update"]["status"], "update_available");
        assert_eq!(
            issued["update"]["selected"]["candidate"]["version"],
            "1.20.0"
        );
        assert_eq!(
            issued["update"]["actions"][0]["url"],
            "https://f-droid.org/repo/org.fdroid.fdroid_1020000.apk"
        );
        let action_id = issued["action"]["action_id"].as_str().unwrap();
        let submitted =
            submit_action_json(&mut runtime, &json!({ "action_id": action_id }).to_string())
                .unwrap();
        assert_eq!(
            submitted["package_id"],
            "android/f-droid/app/org.fdroid.fdroid"
        );
    }

    #[cfg(feature = "lua")]
    #[test]
    fn registered_package_update_check_issues_action_from_github_provider_luaclass() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let repo_root = data_dir.join("repo/official");
        write_github_provider_package_repo(&repo_root, GITHUB_RELEASES_FIXTURE);
        let db = MainDb::open(data_dir.join("main.db")).unwrap();
        register_repository(&db, "official", "Official", 0, &repo_root);
        seed_github_provider_cache(data_dir, GITHUB_RELEASES_FIXTURE);
        let mut runtime = GetterRuntime::new();

        let issued = issue_action_from_registered_package_json(
            &mut runtime,
            data_dir,
            &db,
            &json!({
                "package_id": "android/app/org.fdroid.fdroid",
                "installed_version": "v1.0.0"
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(issued["package"]["source_priority"], json!(["github"]));
        assert_eq!(issued["provider_calls"][0]["provider"], "github");
        assert_eq!(issued["provider_calls"][0]["source"], "cache");
        assert_eq!(issued["update"]["status"], "update_available");
        assert_eq!(
            issued["update"]["selected"]["candidate"]["version"],
            "v1.20.0"
        );
        assert_eq!(issued["update"]["actions"][0]["file_name"], "F-Droid.apk");
    }

    #[cfg(feature = "lua")]
    #[test]
    fn registered_package_update_check_refreshes_github_cache_on_miss_with_getter_transport() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let repo_root = data_dir.join("repo/official");
        write_github_provider_package_repo(&repo_root, GITHUB_RELEASES_FIXTURE);
        let db = MainDb::open(data_dir.join("main.db")).unwrap();
        register_repository(&db, "official", "Official", 0, &repo_root);
        let transport =
            github_transport_with_body(GITHUB_RELEASES_FIXTURE, Some("W/\"runtime-github\""));
        let mut runtime = GetterRuntime::new();

        let issued = issue_action_from_registered_package_json_with_github_transport(
            &mut runtime,
            data_dir,
            &db,
            &json!({
                "package_id": "android/app/org.fdroid.fdroid",
                "installed_version": "v1.0.0"
            })
            .to_string(),
            Some(transport.clone()),
        )
        .unwrap();

        assert_eq!(transport.requests.borrow().len(), 1);
        assert_eq!(transport.requests.borrow()[0].owner, "f-droid");
        assert_eq!(transport.requests.borrow()[0].repo, "fdroidclient");
        assert_eq!(issued["provider_calls"][0]["provider"], "github");
        assert_eq!(issued["provider_calls"][0]["source"], "refreshed");
        assert_eq!(issued["update"]["status"], "update_available");
        assert_eq!(
            issued["update"]["selected"]["candidate"]["version"],
            "v1.20.0"
        );
        let config = GithubReleaseConfig {
            api_base_url: DEFAULT_GITHUB_API_BASE_URL.to_owned(),
            owner: "f-droid".to_owned(),
            repo: "fdroidclient".to_owned(),
        };
        let cached = CacheDb::open(data_dir.join("cache.db"))
            .unwrap()
            .provider_response(&config.cache_key())
            .unwrap()
            .unwrap();
        assert_eq!(cached.response_json[0]["tag_name"], "v1.20.0");
        assert_eq!(cached.freshness_json["etag"], "W/\"runtime-github\"");
    }

    #[cfg(feature = "lua")]
    #[test]
    fn registered_package_update_check_live_github_refresh_allows_free_network_without_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let repo_root = data_dir.join("repo/official");
        let package_dir = repo_root.join("android/app/org.fdroid.fdroid");
        write_github_provider_package_with_options(
            &package_dir,
            None,
            true,
            "F-Droid",
            "org.fdroid.fdroid",
            "f-droid",
            "fdroidclient",
        );
        let db = MainDb::open(data_dir.join("main.db")).unwrap();
        register_repository(&db, "official", "Official", 0, &repo_root);
        let transport = github_transport_with_body(GITHUB_RELEASES_FIXTURE, None);
        let mut runtime = GetterRuntime::new();

        let issued = issue_action_from_registered_package_json_with_github_transport(
            &mut runtime,
            data_dir,
            &db,
            &json!({
                "package_id": "android/app/org.fdroid.fdroid",
                "installed_version": "v1.0.0"
            })
            .to_string(),
            Some(transport.clone()),
        )
        .unwrap();

        assert_eq!(transport.requests.borrow().len(), 1);
        assert_eq!(issued["provider_calls"][0]["source"], "refreshed");
        assert_eq!(issued["update"]["status"], "update_available");
        assert_eq!(
            issued["update"]["selected"]["candidate"]["version"],
            "v1.20.0"
        );
    }

    #[cfg(feature = "lua")]
    #[test]
    fn registered_package_update_check_rejects_unmanifested_live_github_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let repo_root = data_dir.join("repo/official");
        let package_dir = repo_root.join("android/app/org.fdroid.fdroid");
        write_github_provider_package_with_options(
            &package_dir,
            None,
            false,
            "F-Droid",
            "org.fdroid.fdroid",
            "f-droid",
            "fdroidclient",
        );
        let db = MainDb::open(data_dir.join("main.db")).unwrap();
        register_repository(&db, "official", "Official", 0, &repo_root);
        let transport = github_transport_with_body(GITHUB_RELEASES_FIXTURE, None);
        let mut runtime = GetterRuntime::new();

        let err = issue_action_from_registered_package_json_with_github_transport(
            &mut runtime,
            data_dir,
            &db,
            &json!({
                "package_id": "android/app/org.fdroid.fdroid",
                "installed_version": "v1.0.0"
            })
            .to_string(),
            Some(transport),
        )
        .unwrap_err();

        assert_eq!(err.code(), "package.eval_error");
        assert!(err
            .to_string()
            .contains("package.provider.response_not_in_manifest"));
        assert_eq!(runtime.tasks().len(), 0);
    }

    #[cfg(feature = "lua")]
    #[test]
    fn registered_package_update_check_matches_old_upgradeall_github_release_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let repo_root = data_dir.join("repo/official");
        write_upgradeall_github_provider_package_repo(
            &repo_root,
            UPGRADEALL_GITHUB_RELEASES_SNAPSHOT,
        );
        let db = MainDb::open(data_dir.join("main.db")).unwrap();
        register_repository(&db, "official", "Official", 0, &repo_root);
        seed_github_provider_cache_for(
            data_dir,
            "DUpdateSystem",
            "UpgradeAll",
            UPGRADEALL_GITHUB_RELEASES_SNAPSHOT,
        );
        let old_releases: Value =
            serde_json::from_str(UPGRADEALL_OLD_GITHUB_RELEASES_NORMALIZED).unwrap();
        let expected_release = &old_releases[0];
        let expected_asset = &expected_release["assets"][0];
        let mut runtime = GetterRuntime::new();

        let issued = issue_action_from_registered_package_json(
            &mut runtime,
            data_dir,
            &db,
            &json!({
                "package_id": "android/app/net.xzos.upgradeall",
                "installed_version": "0.13-beta.3"
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(issued["package"]["source_priority"], json!(["github"]));
        assert_eq!(
            issued["package"]["installed"][0],
            json!({ "kind": "android_package", "package_name": "net.xzos.upgradeall" })
        );
        assert_eq!(issued["provider_calls"][0]["provider"], "github");
        assert_eq!(issued["provider_calls"][0]["owner"], "DUpdateSystem");
        assert_eq!(issued["provider_calls"][0]["repo"], "UpgradeAll");
        assert_eq!(issued["provider_calls"][0]["source"], "cache");
        assert_eq!(issued["update"]["status"], "update_available");
        assert_eq!(
            issued["update"]["selected"]["candidate"]["version"],
            expected_release["version_number"]
        );
        assert_eq!(
            issued["update"]["selected"]["candidate"]["changelog"],
            expected_release["changelog"]
        );
        assert_eq!(
            issued["update"]["selected"]["artifact"]["name"],
            expected_asset["file_name"]
        );
        assert_eq!(
            issued["update"]["selected"]["artifact"]["content_type"],
            expected_asset["file_type"]
        );
        assert_eq!(
            issued["update"]["actions"][0]["url"],
            expected_asset["download_url"]
        );
        assert_eq!(
            issued["update"]["actions"][0]["file_name"],
            expected_asset["file_name"]
        );
        let action_id = issued["action"]["action_id"].as_str().unwrap();
        let submitted =
            submit_action_json(&mut runtime, &json!({ "action_id": action_id }).to_string())
                .unwrap();
        assert_eq!(submitted["package_id"], "android/app/net.xzos.upgradeall");
    }

    #[cfg(feature = "lua")]
    #[test]
    fn registered_package_update_check_does_not_accept_provider_fixture_payload() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let repo_root = data_dir.join("repo/official");
        write_fdroid_provider_package_repo(&repo_root, Some(FDROID_INDEX_FIXTURE), false);
        let db = MainDb::open(data_dir.join("main.db")).unwrap();
        register_repository(&db, "official", "Official", 0, &repo_root);
        let mut runtime = GetterRuntime::new();

        let err = issue_action_from_registered_package_json(
            &mut runtime,
            data_dir,
            &db,
            &json!({
                "package_id": "android/f-droid/app/org.fdroid.fdroid",
                "installed_version": "1.0.0",
                "fdroid_index_xml": FDROID_INDEX_FIXTURE
            })
            .to_string(),
        )
        .unwrap_err();

        assert_eq!(err.code(), "runtime.invalid_request");
        assert!(err.to_string().contains("unknown field `fdroid_index_xml`"));
        assert_eq!(runtime.tasks().len(), 0);
    }

    #[cfg(feature = "lua")]
    #[test]
    fn registered_package_update_check_rejects_unmanifested_provider_cache() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let repo_root = data_dir.join("repo/official");
        write_fdroid_provider_package_repo(&repo_root, None, false);
        let db = MainDb::open(data_dir.join("main.db")).unwrap();
        register_repository(&db, "official", "Official", 0, &repo_root);
        seed_fdroid_provider_cache(data_dir, FDROID_INDEX_FIXTURE);
        let mut runtime = GetterRuntime::new();

        let err = issue_action_from_registered_package_json(
            &mut runtime,
            data_dir,
            &db,
            &json!({
                "package_id": "android/f-droid/app/org.fdroid.fdroid",
                "installed_version": "1.0.0"
            })
            .to_string(),
        )
        .unwrap_err();

        assert_eq!(err.code(), "package.eval_error");
        assert!(err
            .to_string()
            .contains("package.provider.response_not_in_manifest"));
        assert_eq!(runtime.tasks().len(), 0);
    }

    #[cfg(feature = "lua")]
    #[test]
    fn registered_package_update_check_accepts_manifest_compatible_provider_cache() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let repo_root = data_dir.join("repo/official");
        write_fdroid_provider_package_repo(&repo_root, Some(FDROID_INDEX_FIXTURE), false);
        let db = MainDb::open(data_dir.join("main.db")).unwrap();
        register_repository(&db, "official", "Official", 0, &repo_root);
        seed_fdroid_provider_cache(data_dir, FDROID_INDEX_FIXTURE);
        let mut runtime = GetterRuntime::new();

        let issued = issue_action_from_registered_package_json(
            &mut runtime,
            data_dir,
            &db,
            &json!({
                "package_id": "android/f-droid/app/org.fdroid.fdroid",
                "installed_version": "1.0.0"
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(issued["provider_calls"][0]["source"], "cache");
        assert_eq!(issued["update"]["status"], "update_available");
        assert_eq!(
            issued["update"]["selected"]["candidate"]["version"],
            "1.20.0"
        );
    }

    #[test]
    fn offline_update_check_issues_getter_owned_action_id() {
        let mut runtime = GetterRuntime::new();

        let issued = issue_action_from_offline_update_check_json(
            &mut runtime,
            &json!({
                "fixture": update_fixture("android/org.fdroid.fdroid", Some("1.0.0"), vec!["1.2.0"]),
                "dependency_digest": "sha256:fixture"
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(issued["update"]["status"], "update_available");
        let action_id = issued["action"]["action_id"].as_str().unwrap();
        let submitted =
            submit_action_json(&mut runtime, &json!({ "action_id": action_id }).to_string())
                .unwrap();
        assert_eq!(submitted["package_id"], "android/org.fdroid.fdroid");
        assert_eq!(submitted["status"], "queued");
    }

    #[test]
    fn json_submit_and_download_writes_file_through_injected_transport() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = GetterRuntime::new();
        let issued = issue_action(&mut runtime, plan_without_install("generic/example"));
        let action_id = issued["action_id"].as_str().unwrap();
        let transport = RecordingDownloadTransport {
            requests: RefCell::new(Vec::new()),
        };

        let completed = submit_action_and_download_json_with_transport(
            &mut runtime,
            temp.path(),
            &json!({ "action_id": action_id }).to_string(),
            &transport,
        )
        .unwrap();

        assert_eq!(
            transport.requests.borrow().as_slice(),
            &["https://example.invalid/archive.zip"]
        );
        assert_eq!(completed["status"], "completed");
        assert_eq!(completed["downloaded_file"]["file_name"], "archive.zip");
        assert_eq!(completed["downloaded_file"]["size_bytes"], 4);
        let local_path = completed["downloaded_file"]["local_path"].as_str().unwrap();
        assert_eq!(fs::read(local_path).unwrap(), b"data");
        assert!(runtime.submit_action(action_id).is_err());
    }

    #[test]
    fn json_submit_rejects_product_supplied_download_fields() {
        let mut runtime = GetterRuntime::new();
        let issued = issue_action(&mut runtime, plan_without_install("generic/example"));
        let action_id = issued["action_id"].as_str().unwrap();

        let error = submit_action_json(
            &mut runtime,
            &json!({
                "action_id": action_id,
                "url": "https://example.invalid/other.bin"
            })
            .to_string(),
        )
        .unwrap_err();

        assert_eq!(error.code(), "runtime.invalid_request");
        assert!(error.detail().unwrap().contains("unknown field `url`"));
        assert_eq!(runtime.tasks().len(), 0);
    }

    #[test]
    fn offline_update_check_without_update_does_not_issue_action() {
        let mut runtime = GetterRuntime::new();

        let issued = issue_action_from_offline_update_check_json(
            &mut runtime,
            &json!({
                "fixture": update_fixture("android/org.fdroid.fdroid", Some("1.2.0"), vec!["1.2.0"]),
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(issued["update"]["status"], "up_to_date");
        assert!(issued["action"].is_null());
        assert_eq!(runtime.tasks().len(), 0);
    }

    #[test]
    fn json_submit_and_user_result_round_trip_consumes_action() {
        let mut runtime = GetterRuntime::new();
        let issued = issue_action(&mut runtime, plan("android/org.fdroid.fdroid"));
        let action_id = issued["action_id"].as_str().unwrap();

        let submitted =
            submit_action_json(&mut runtime, &json!({ "action_id": action_id }).to_string())
                .unwrap();

        assert_eq!(submitted["status"], "queued");
        let error =
            submit_action_json(&mut runtime, &json!({ "action_id": action_id }).to_string())
                .unwrap_err();
        assert_eq!(error.code(), "action.not_found");

        let task_id = submitted["task_id"].as_str().unwrap();
        task_start_json(&mut runtime, &json!({ "task_id": task_id }).to_string()).unwrap();
        task_complete_download_json(&mut runtime, &json!({ "task_id": task_id }).to_string())
            .unwrap();
        let completed = task_user_result_json(
            &mut runtime,
            &json!({ "task_id": task_id, "result": "accepted" }).to_string(),
        )
        .unwrap();

        assert_eq!(completed["status"], "completed");
        assert_eq!(
            runtime.task(task_id).unwrap().status,
            RuntimeTaskStatus::Completed
        );
    }

    #[test]
    fn json_list_filters_and_clean_modes_follow_runtime_semantics() {
        let mut runtime = GetterRuntime::new();
        let fdroid = submit_plan(&mut runtime, plan("android/org.fdroid.fdroid"));
        let termux = submit_plan(&mut runtime, plan("android/com.termux"));
        runtime.cancel_task(&fdroid).unwrap();
        runtime.start_task(&termux).unwrap();

        let active = task_list_json(&runtime, &json!({ "active": true }).to_string()).unwrap();
        assert_eq!(active["tasks"].as_array().unwrap().len(), 1);
        assert_eq!(active["tasks"][0]["task_id"], termux);

        let package = task_list_json(
            &runtime,
            &json!({ "package_id": "android/org.fdroid.fdroid" }).to_string(),
        )
        .unwrap();
        assert_eq!(package["tasks"].as_array().unwrap().len(), 1);
        assert_eq!(package["tasks"][0]["task_id"], fdroid);

        let removed = task_clean_json(&mut runtime, "{}").unwrap();
        assert_eq!(removed["tasks"].as_array().unwrap().len(), 1);
        assert_eq!(runtime.task(&fdroid).unwrap_err().code(), "task.not_found");
        assert_eq!(
            runtime.task(&termux).unwrap().status,
            RuntimeTaskStatus::Running
        );
    }

    #[test]
    fn json_clean_rejects_unknown_mode_without_mutating_tasks() {
        let mut runtime = GetterRuntime::new();
        let task_id = submit_plan(&mut runtime, plan_without_install("generic/example"));
        runtime.start_task(&task_id).unwrap();
        runtime.complete_download(&task_id).unwrap();

        let error =
            task_clean_json(&mut runtime, &json!({ "mode": "all" }).to_string()).unwrap_err();

        assert_eq!(error.code(), "runtime.invalid_request");
        assert_eq!(
            runtime.task(&task_id).unwrap().status,
            RuntimeTaskStatus::Completed
        );
    }

    struct RecordingDownloadTransport {
        requests: RefCell<Vec<String>>,
    }

    impl crate::download::RuntimeDownloadTransport for RecordingDownloadTransport {
        fn fetch(
            &self,
            request: &crate::download::RuntimeDownloadTransportRequest<'_>,
            sink: &mut dyn crate::download::RuntimeDownloadSink,
        ) -> Result<(), crate::download::RuntimeDownloadTransportError> {
            self.requests.borrow_mut().push(request.url.to_owned());
            sink.set_total_bytes(Some(4))
                .map_err(crate::download::RuntimeDownloadTransportError::Sink)?;
            sink.write_chunk(b"data")
                .map_err(crate::download::RuntimeDownloadTransportError::Sink)?;
            Ok(())
        }
    }

    fn submit_plan(runtime: &mut GetterRuntime, plan: SealedActionPlan) -> String {
        let action = runtime.issue_action(plan);
        runtime.submit_action(&action.action_id).unwrap().task_id
    }

    #[cfg(feature = "lua")]
    fn seed_fdroid_provider_cache(data_dir: &std::path::Path, fixture_body: &str) {
        let endpoint = FdroidEndpointConfig::default();
        let catalog = parse_fdroid_index_xml(fixture_body).unwrap();
        let db = CacheDb::open(data_dir.join("cache.db")).unwrap();
        db.upsert_provider_response(&ProviderResponseUpsert {
            cache_key: endpoint.cache_key(),
            provider: FDROID_PROVIDER_ID.to_owned(),
            response_json: serde_json::to_value(catalog).unwrap(),
            source_response_sha512: vec![sha512_hex(fixture_body.as_bytes())],
            provenance_schema_version: Some(PROVIDER_RESPONSE_PROVENANCE_SCHEMA_V1.to_owned()),
            freshness_json: json!({}),
        })
        .unwrap();
    }

    #[cfg(feature = "lua")]
    fn seed_github_provider_cache(data_dir: &std::path::Path, fixture_body: &str) {
        seed_github_provider_cache_for(data_dir, "f-droid", "fdroidclient", fixture_body);
    }

    #[cfg(feature = "lua")]
    fn seed_github_provider_cache_for(
        data_dir: &std::path::Path,
        owner: &str,
        repo: &str,
        fixture_body: &str,
    ) {
        let config = GithubReleaseConfig {
            api_base_url: DEFAULT_GITHUB_API_BASE_URL.to_owned(),
            owner: owner.to_owned(),
            repo: repo.to_owned(),
        };
        let releases = parse_github_releases_json(fixture_body).unwrap();
        let db = CacheDb::open(data_dir.join("cache.db")).unwrap();
        db.upsert_provider_response(&ProviderResponseUpsert {
            cache_key: config.cache_key(),
            provider: GITHUB_PROVIDER_ID.to_owned(),
            response_json: serde_json::to_value(releases).unwrap(),
            source_response_sha512: vec![sha512_hex(fixture_body.as_bytes())],
            provenance_schema_version: Some(PROVIDER_RESPONSE_PROVENANCE_SCHEMA_V1.to_owned()),
            freshness_json: json!({}),
        })
        .unwrap();
    }

    #[cfg(feature = "lua")]
    fn write_fdroid_provider_package_repo(
        root: &std::path::Path,
        manifest_body: Option<&str>,
        allow_free_network: bool,
    ) {
        let package_dir = root.join("android/f-droid/app/org.fdroid.fdroid");
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
        if let Some(body) = manifest_body {
            fs::write(
                package_dir.join("Manifest"),
                format!("{} fixture-body\n", sha512_hex(body.as_bytes())),
            )
            .unwrap();
        }
        fs::write(
            package_dir.join("9999.lua"),
            r#"#!/bin/upa-lua v1
local fdroid = require("luaclass.fdroid_android")
return fdroid.package {
  package_name = "org.fdroid.fdroid",
}
"#,
        )
        .unwrap();
    }

    #[cfg(feature = "lua")]
    fn write_github_provider_package_repo(root: &std::path::Path, manifest_body: &str) {
        let package_dir = root.join("android/app/org.fdroid.fdroid");
        write_github_provider_package(
            &package_dir,
            manifest_body,
            "F-Droid",
            "org.fdroid.fdroid",
            "f-droid",
            "fdroidclient",
        );
    }

    #[cfg(feature = "lua")]
    fn write_upgradeall_github_provider_package_repo(root: &std::path::Path, manifest_body: &str) {
        let package_dir = root.join("android/app/net.xzos.upgradeall");
        write_github_provider_package(
            &package_dir,
            manifest_body,
            "UpgradeAll",
            "net.xzos.upgradeall",
            "DUpdateSystem",
            "UpgradeAll",
        );
    }

    #[cfg(feature = "lua")]
    fn write_github_provider_package(
        package_dir: &std::path::Path,
        manifest_body: &str,
        name: &str,
        android_package: &str,
        owner: &str,
        repo: &str,
    ) {
        write_github_provider_package_with_options(
            package_dir,
            Some(manifest_body),
            false,
            name,
            android_package,
            owner,
            repo,
        );
    }

    #[cfg(feature = "lua")]
    fn write_github_provider_package_with_options(
        package_dir: &std::path::Path,
        manifest_body: Option<&str>,
        allow_free_network: bool,
        name: &str,
        android_package: &str,
        owner: &str,
        repo: &str,
    ) {
        fs::create_dir_all(package_dir).unwrap();
        let mut metadata = json!({
            "type": "android:app",
            "android": { "package_name": android_package }
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
                package_dir.join("Manifest"),
                format!("{} fixture-body\n", sha512_hex(manifest_body.as_bytes())),
            )
            .unwrap();
        }
        fs::write(
            package_dir.join("9999.lua"),
            format!(
                r#"#!/bin/upa-lua v1
local github_android = require("luaclass.github_android_apk")
return github_android.package {{
  name = "{name}",
  android_package = "{android_package}",
  owner = "{owner}",
  repo = "{repo}",
  asset = {{ include = "[.]apk$" }},
}}
"#
            ),
        )
        .unwrap();
    }

    #[cfg(feature = "lua")]
    fn register_repository(
        db: &MainDb,
        id: &str,
        name: &str,
        priority: i32,
        root: &std::path::Path,
    ) {
        db.upsert_repository(
            &RepositoryMetadata {
                id: id.parse().unwrap(),
                name: name.to_owned(),
                priority: RepositoryPriority::new(priority),
                api_version: REPO_API_VERSION_V1.to_owned(),
            },
            Some(root),
            None,
        )
        .unwrap();
    }

    #[cfg(feature = "lua")]
    fn sha512_hex(body: &[u8]) -> String {
        let mut hasher = Sha512::new();
        hasher.update(body);
        format!("{:x}", hasher.finalize())
    }

    #[cfg(feature = "lua")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RecordedGithubReleaseRequest {
        api_base_url: String,
        owner: String,
        repo: String,
    }

    #[cfg(feature = "lua")]
    struct MockGithubReleaseTransport {
        response: RefCell<Result<GithubReleaseTransportResponse, GithubReleaseTransportError>>,
        requests: RefCell<Vec<RecordedGithubReleaseRequest>>,
    }

    #[cfg(feature = "lua")]
    fn github_transport_with_body(
        body: &str,
        etag: Option<&str>,
    ) -> Rc<MockGithubReleaseTransport> {
        Rc::new(MockGithubReleaseTransport {
            response: RefCell::new(Ok(GithubReleaseTransportResponse {
                body: body.to_owned(),
                etag: etag.map(str::to_owned),
                last_modified: Some("Wed, 01 Jul 2026 00:00:00 GMT".to_owned()),
            })),
            requests: RefCell::new(Vec::new()),
        })
    }

    #[cfg(feature = "lua")]
    impl GithubReleaseTransport for MockGithubReleaseTransport {
        fn fetch_releases(
            &self,
            request: &GithubReleaseTransportRequest<'_>,
        ) -> Result<GithubReleaseTransportResponse, GithubReleaseTransportError> {
            self.requests
                .borrow_mut()
                .push(RecordedGithubReleaseRequest {
                    api_base_url: request.api_base_url.to_owned(),
                    owner: request.owner.to_owned(),
                    repo: request.repo.to_owned(),
                });
            self.response.borrow().clone()
        }
    }

    #[cfg(feature = "lua")]
    fn write_package_directory_static_update_repo(root: &std::path::Path) {
        let package_dir = root.join("android/app/com.example.autogen");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{
  "type": "android:app",
  "display_name": "Example Autogen",
  "android": { "package_name": "com.example.autogen" }
}"#,
        )
        .unwrap();
        fs::write(package_dir.join("Manifest"), "").unwrap();
        fs::write(
            package_dir.join("9999.lua"),
            r#"#!/bin/upa-lua v1
return package_version {
  updates = {
    {
      version = "1.2.0",
      artifacts = {
        {
          name = "app.apk",
          url = "https://example.invalid/app.apk",
          file_name = "app.apk",
        },
      },
    },
  },
}
"#,
        )
        .unwrap();
    }

    fn update_fixture(
        package_id: &str,
        installed_version: Option<&str>,
        versions: Vec<&str>,
    ) -> OfflineUpdateCheckFixture {
        OfflineUpdateCheckFixture {
            format: OFFLINE_UPDATE_CHECK_FORMAT.to_owned(),
            version: OFFLINE_UPDATE_CHECK_VERSION,
            package_id: package_id.parse().unwrap(),
            installed_version: installed_version.map(str::to_owned),
            pin_version: None,
            candidates: versions
                .into_iter()
                .map(|version| UpdateCandidate {
                    version: version.to_owned(),
                    version_code: None,
                    changelog: None,
                    channel: None,
                    source: None,
                    artifacts: vec![UpdateArtifact {
                        name: "app.apk".to_owned(),
                        url: "https://example.invalid/app.apk".to_owned(),
                        content_type: None,
                        file_name: Some("app.apk".to_owned()),
                        sha256: None,
                        size: None,
                    }],
                })
                .collect(),
        }
    }

    fn plan(package_id: &str) -> SealedActionPlan {
        SealedActionPlan {
            package_id: package_id.parse().unwrap(),
            actions: vec![
                UpdateAction::Download {
                    url: "https://example.invalid/app.apk".to_owned(),
                    file_name: "app.apk".to_owned(),
                },
                UpdateAction::Install {
                    installer: "android_package".to_owned(),
                    file: "app.apk".to_owned(),
                },
            ],
            lua_object: lua_object(package_id),
        }
    }

    fn plan_without_install(package_id: &str) -> SealedActionPlan {
        SealedActionPlan {
            package_id: package_id.parse().unwrap(),
            actions: vec![UpdateAction::Download {
                url: "https://example.invalid/archive.zip".to_owned(),
                file_name: "archive.zip".to_owned(),
            }],
            lua_object: lua_object(package_id),
        }
    }

    fn lua_object(package_id: &str) -> PackageVersionLuaObject {
        PackageVersionLuaObject {
            object_id: format!("lua:{package_id}"),
            dependency_digest: "sha256:test".to_owned(),
        }
    }
}
