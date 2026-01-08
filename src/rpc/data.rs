use crate::downloader::{DownloadState, TaskInfo};
use jsonrpsee::core::traits::ToRpcParams;
use serde::{Deserialize, Serialize};
use serde_json::value::to_raw_value;
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Debug)]
pub struct RpcInitRequest<'a> {
    pub data_path: &'a str,
    pub cache_path: &'a str,
    pub global_expire_time: u64,
}

impl ToRpcParams for RpcInitRequest<'_> {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RpcAppRequest<'a> {
    pub hub_uuid: &'a str,
    pub app_data: BTreeMap<&'a str, &'a str>,
    pub hub_data: BTreeMap<&'a str, &'a str>,
}

impl ToRpcParams for RpcAppRequest<'_> {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RpcCloudConfigRequest<'a> {
    pub api_url: &'a str,
}

impl ToRpcParams for RpcCloudConfigRequest<'_> {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}

// ============================================================================
// Downloader RPC Data Structures
// ============================================================================

/// Request to submit a download task
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcDownloadRequest<'a> {
    pub url: &'a str,
    pub dest_path: &'a str,
    #[serde(default)]
    pub headers: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    pub cookies: Option<std::collections::HashMap<String, String>>,
}

impl ToRpcParams for RpcDownloadRequest<'_> {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}

/// Request to submit multiple download tasks
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcDownloadBatchRequest {
    pub tasks: Vec<RpcDownloadTask>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RpcDownloadTask {
    pub url: String,
    pub dest_path: String,
}

impl ToRpcParams for RpcDownloadBatchRequest {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}

/// Response with task ID
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcTaskIdResponse {
    pub task_id: String,
}

/// Response with multiple task IDs
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcTaskIdsResponse {
    pub task_ids: Vec<String>,
}

/// Request to query task status
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcTaskStatusRequest<'a> {
    pub task_id: &'a str,
}

impl ToRpcParams for RpcTaskStatusRequest<'_> {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}

/// Request to wait for task state change (long-polling)
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcWaitForChangeRequest<'a> {
    pub task_id: &'a str,
    pub timeout_seconds: u64,
}

impl ToRpcParams for RpcWaitForChangeRequest<'_> {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}

/// Request to cancel a task
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcCancelTaskRequest<'a> {
    pub task_id: &'a str,
}

impl ToRpcParams for RpcCancelTaskRequest<'_> {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}

/// Request to pause a task
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcPauseTaskRequest<'a> {
    pub task_id: &'a str,
}

impl ToRpcParams for RpcPauseTaskRequest<'_> {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}

/// Request to resume a task
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcResumeTaskRequest<'a> {
    pub task_id: &'a str,
}

impl ToRpcParams for RpcResumeTaskRequest<'_> {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}

/// Request to query tasks by state
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcTasksByStateRequest {
    pub state: DownloadState,
}

impl ToRpcParams for RpcTasksByStateRequest {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}

/// Response with task information
pub type RpcTaskInfoResponse = TaskInfo;

/// Response with multiple tasks
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcTasksResponse {
    pub tasks: Vec<TaskInfo>,
}
