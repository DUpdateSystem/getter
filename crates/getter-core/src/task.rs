//! Offline task lifecycle DTOs for getter-owned download/install workflows.
//!
//! These types are transport/domain shapes. They describe getter task state,
//! pollable events, and abstract platform install handoffs without performing
//! live network downloads or Android installation.

use crate::{PackageId, UpdateAction};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

pub const DOWNLOAD_REQUEST_FORMAT: &str = "getter-download-request";
pub const DOWNLOAD_REQUEST_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadTaskRequest {
    pub format: String,
    pub version: u32,
    pub package_id: PackageId,
    #[serde(default)]
    pub actions: Vec<UpdateAction>,
    pub executor: TaskExecutor,
}

impl DownloadTaskRequest {
    pub fn download_action(&self) -> Option<&UpdateAction> {
        self.actions
            .iter()
            .find(|action| matches!(action, UpdateAction::Download { .. }))
    }

    pub fn install_action(&self) -> Option<&UpdateAction> {
        self.actions
            .iter()
            .find(|action| matches!(action, UpdateAction::Install { .. }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskExecutor {
    Fake,
}

impl TaskExecutor {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fake => "fake",
        }
    }
}

impl FromStr for TaskExecutor {
    type Err = TaskModelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "fake" => Ok(Self::Fake),
            other => Err(TaskModelError::InvalidExecutor(other.to_owned())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadTaskStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Canceled,
}

impl DownloadTaskStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Canceled)
    }
}

impl FromStr for DownloadTaskStatus {
    type Err = TaskModelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "canceled" => Ok(Self::Canceled),
            other => Err(TaskModelError::InvalidTaskStatus(other.to_owned())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEventKind {
    TaskCreated,
    TaskStarted,
    TaskSucceeded,
    TaskFailed,
    TaskCanceled,
    InstallHandoffRequested,
    InstallResultRecorded,
}

impl TaskEventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TaskCreated => "task_created",
            Self::TaskStarted => "task_started",
            Self::TaskSucceeded => "task_succeeded",
            Self::TaskFailed => "task_failed",
            Self::TaskCanceled => "task_canceled",
            Self::InstallHandoffRequested => "install_handoff_requested",
            Self::InstallResultRecorded => "install_result_recorded",
        }
    }
}

impl FromStr for TaskEventKind {
    type Err = TaskModelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "task_created" => Ok(Self::TaskCreated),
            "task_started" => Ok(Self::TaskStarted),
            "task_succeeded" => Ok(Self::TaskSucceeded),
            "task_failed" => Ok(Self::TaskFailed),
            "task_canceled" => Ok(Self::TaskCanceled),
            "install_handoff_requested" => Ok(Self::InstallHandoffRequested),
            "install_result_recorded" => Ok(Self::InstallResultRecorded),
            other => Err(TaskModelError::InvalidEventKind(other.to_owned())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadTaskSummary {
    pub id: String,
    pub package_id: PackageId,
    pub status: DownloadTaskStatus,
    pub executor: TaskExecutor,
    pub actions: Vec<UpdateAction>,
    pub download_file_name: String,
    #[serde(default)]
    pub downloaded_file: Option<String>,
    #[serde(default)]
    pub failure_message: Option<String>,
    #[serde(default)]
    pub install_handoff_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEvent {
    pub cursor: u64,
    pub task_id: String,
    pub kind: TaskEventKind,
    #[serde(default)]
    pub status: Option<DownloadTaskStatus>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEventPage {
    pub events: Vec<TaskEvent>,
    pub next_cursor: u64,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskCancelResult {
    pub task_id: String,
    pub status: DownloadTaskStatus,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRunResult {
    pub task: DownloadTaskSummary,
    #[serde(default)]
    pub install_handoff: Option<InstallHandoffSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSubmitResult {
    pub task: DownloadTaskSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallResultRecord {
    pub handoff: InstallHandoffSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallHandoffStatus {
    Requested,
    Accepted,
    Succeeded,
    Failed,
    Canceled,
}

impl InstallHandoffStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Accepted => "accepted",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }
}

impl FromStr for InstallHandoffStatus {
    type Err = TaskModelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "requested" => Ok(Self::Requested),
            "accepted" => Ok(Self::Accepted),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "canceled" => Ok(Self::Canceled),
            other => Err(TaskModelError::InvalidInstallHandoffStatus(
                other.to_owned(),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallHandoffSummary {
    pub id: String,
    pub task_id: String,
    pub package_id: PackageId,
    pub installer: String,
    pub file: String,
    pub status: InstallHandoffStatus,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskModelError {
    #[error("invalid task executor '{0}'")]
    InvalidExecutor(String),
    #[error("invalid download task status '{0}'")]
    InvalidTaskStatus(String),
    #[error("invalid task event kind '{0}'")]
    InvalidEventKind(String),
    #[error("invalid install handoff status '{0}'")]
    InvalidInstallHandoffStatus(String),
}
