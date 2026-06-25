//! Shared JSON operation helpers for the in-memory Phase D getter runtime.
//!
//! These functions are intentionally thin over [`getter_core::runtime`]. They
//! give embedders (native bridge, future single-process debug CLI, tests) one
//! place to map JSON requests into runtime controls without reintroducing
//! persisted task state or duplicating task/control semantics in Flutter or
//! Android glue.

use getter_core::{
    runtime::{
        GetterRuntime, IssuedAction, RuntimeError, SealedActionPlan, TaskCleanMode, TaskSnapshot,
        UserResult,
    },
    PackageId,
};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum RuntimeOperationError {
    #[error("invalid runtime request: {0}")]
    InvalidRequest(String),
    #[error("runtime error: {0}")]
    Runtime(#[from] RuntimeError),
    #[error("runtime response serialization failed: {0}")]
    Serialize(String),
}

impl RuntimeOperationError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "runtime.invalid_request",
            Self::Runtime(error) => error.code(),
            Self::Serialize(_) => "runtime.serialize_error",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "Getter runtime request is invalid",
            Self::Runtime(_) => "Getter runtime operation failed",
            Self::Serialize(_) => "Getter runtime response serialization failed",
        }
    }

    pub fn detail(&self) -> Option<String> {
        match self {
            Self::InvalidRequest(detail) | Self::Serialize(detail) => Some(detail.clone()),
            Self::Runtime(error) => Some(error.to_string()),
        }
    }
}

pub fn issue_action(runtime: &mut GetterRuntime, plan: SealedActionPlan) -> Value {
    issued_action_json(runtime.issue_action(plan))
}

pub fn submit_action_json(
    runtime: &mut GetterRuntime,
    request_json: &str,
) -> Result<Value, RuntimeOperationError> {
    let request: SubmitActionRequest = parse_request(request_json)?;
    task_json(runtime.submit_action(&request.action_id)?)
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
    use getter_core::{runtime::PackageVersionLuaObject, runtime::RuntimeTaskStatus, UpdateAction};

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

    fn submit_plan(runtime: &mut GetterRuntime, plan: SealedActionPlan) -> String {
        let action = runtime.issue_action(plan);
        runtime.submit_action(&action.action_id).unwrap().task_id
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
