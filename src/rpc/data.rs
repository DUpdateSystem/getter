use crate::downloader::{DownloadState, TaskInfo};
use jsonrpsee::core::traits::ToRpcParams;
use serde::{Deserialize, Serialize};
use serde_json::value::to_raw_value;
use std::collections::{BTreeMap, HashMap};

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
// Provider Registration RPC Data Structures
// ============================================================================

/// Request to register an external provider
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcRegisterProviderRequest<'a> {
    pub hub_uuid: &'a str,
    pub url: &'a str,
}

impl ToRpcParams for RpcRegisterProviderRequest<'_> {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}

/// Request to get download info for an app
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcDownloadInfoRequest<'a> {
    pub hub_uuid: &'a str,
    pub app_data: BTreeMap<&'a str, &'a str>,
    pub hub_data: BTreeMap<&'a str, &'a str>,
    pub asset_index: Vec<i32>,
}

impl ToRpcParams for RpcDownloadInfoRequest<'_> {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}

/// Download item data returned by get_download
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadItemData {
    pub name: Option<String>,
    pub url: String,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub cookies: Option<HashMap<String, String>>,
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
    /// Hub UUID for routing to registered external downloaders.
    /// When set, routes to external downloader; when None, uses default HTTP downloader.
    #[serde(default)]
    pub hub_uuid: Option<String>,
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

// ============================================================================
// Downloader Registration RPC Data Structures
// ============================================================================

/// Request to register an external downloader for a hub_uuid
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcRegisterDownloaderRequest<'a> {
    pub hub_uuid: &'a str,
    pub rpc_url: &'a str,
}

impl ToRpcParams for RpcRegisterDownloaderRequest<'_> {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}

/// Request to unregister an external downloader for a hub_uuid
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcUnregisterDownloaderRequest<'a> {
    pub hub_uuid: &'a str,
}

impl ToRpcParams for RpcUnregisterDownloaderRequest<'_> {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}

// ============================================================================
// Manager RPC Data Structures
// ============================================================================

use crate::database::models::app::AppRecord;
use crate::database::models::extra_hub::ExtraHubRecord;
use crate::database::models::hub::HubRecord;
use crate::manager::app_status::AppStatus;

/// Response wrapping a list of apps with their current status.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppWithStatus {
    pub record: AppRecord,
    pub status: AppStatus,
}

/// Request to save/update an app record.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcSaveAppRequest {
    pub record: AppRecord,
}

/// Request to delete an app by record id.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcDeleteAppRequest {
    pub record_id: String,
}

/// Request to get a single app by record id.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcGetAppRequest {
    pub record_id: String,
}

/// Request to save/update a hub record.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcSaveHubRequest {
    pub record: HubRecord,
}

/// Request to delete a hub by UUID.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcDeleteHubRequest {
    pub hub_uuid: String,
}

/// Request to get a hub by UUID.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcGetHubRequest {
    pub hub_uuid: String,
}

/// Request to set the applications mode for a hub.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcSetApplicationsModeRequest {
    pub hub_uuid: String,
    pub enable: bool,
}

/// Request to ignore/unignore an app in a hub.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcHubIgnoreAppRequest {
    pub hub_uuid: String,
    pub app_id: HashMap<String, Option<String>>,
    pub ignore: bool,
}

/// Request to set virtual (installed) apps list from Android.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcSetVirtualAppsRequest {
    pub apps: Vec<AppRecord>,
}

/// Request to get app status.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcGetAppStatusRequest {
    pub record_id: String,
}

/// Request to save/update an ExtraHub record.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcSaveExtraHubRequest {
    pub record: ExtraHubRecord,
}

/// Request to get an ExtraHub by id.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcGetExtraHubRequest {
    pub id: String,
}

// ============================================================================
// Android API / Notification Registration RPC Data Structures
// ============================================================================

/// Request to register the Kotlin Android API callback URL.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcRegisterAndroidApiRequest {
    pub url: String,
}

/// Request to register the Kotlin notification callback URL.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcRegisterNotificationRequest {
    pub url: String,
}

// ============================================================================
// ExtraApp RPC Data Structures
// ============================================================================

use crate::database::models::extra_app::ExtraAppRecord;

/// Request to get an ExtraApp record by app_id map.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcGetExtraAppRequest {
    pub app_id: HashMap<String, Option<String>>,
}

/// Request to save/update an ExtraApp record.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcSaveExtraAppRequest {
    pub record: ExtraAppRecord,
}

/// Request to delete an ExtraApp by database id.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcDeleteExtraAppRequest {
    pub id: String,
}

// ============================================================================
// Cloud Config Manager RPC Data Structures
// ============================================================================

/// Request to apply a specific cloud hub/app config by UUID.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcCloudConfigApplyRequest {
    pub uuid: String,
}

/// Request to initialise the CloudConfigGetter with an API URL.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcCloudConfigInitRequest {
    pub api_url: String,
}

/// Request to update the auth map for a hub.
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcUpdateHubAuthRequest {
    pub hub_uuid: String,
    pub auth: HashMap<String, String>,
}

impl ToRpcParams for RpcUpdateHubAuthRequest {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}
