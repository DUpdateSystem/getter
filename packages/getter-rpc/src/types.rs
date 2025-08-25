use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub params: Option<serde_json::Value>,
    pub id: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<RpcError>,
    pub id: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl RpcResponse {
    pub fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: Some(result),
            error: None,
            id,
        }
    }

    pub fn error(id: serde_json::Value, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(RpcError { code, message }),
            id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo {
    pub id: String,
    pub name: String,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub version: String,
    pub download_url: String,
    pub release_notes: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStatusInfo {
    pub id: String,
    pub status: String,
    pub last_check: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddAppRequest {
    pub app_id: String,
    pub hub_uuid: String,
    pub app_data: std::collections::HashMap<String, String>,
    pub hub_data: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateAppRequest {
    pub app_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveAppRequest {
    pub app_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAppStatusRequest {
    pub app_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetLatestReleaseRequest {
    pub app_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckAppAvailableRequest {
    pub app_id: String,
}
