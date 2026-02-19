//! External RPC-based downloader implementation
//!
//! Delegates download operations to an external service via HTTP JSON-RPC 2.0.
//! The external service (e.g., a Kotlin-side GooglePlayDownloader) must implement
//! the standard downloader RPC protocol:
//!   - download_submit(url, dest_path, headers?, cookies?) -> {task_id}
//!   - download_get_status(task_id) -> TaskInfo
//!   - download_wait_for_change(task_id, timeout_seconds) -> TaskInfo
//!   - download_pause(task_id) -> bool
//!   - download_resume(task_id) -> bool
//!   - download_cancel(task_id) -> bool

use super::error::{DownloadError, Result};
use super::traits::{Downloader, DownloaderCapabilities, ProgressCallback, RequestOptions};
use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// JSON-RPC 2.0 request structure
#[derive(Serialize)]
struct JsonRpcRequest<'a> {
    jsonrpc: &'static str,
    method: &'a str,
    params: serde_json::Value,
    id: u64,
}

/// JSON-RPC 2.0 response structure
#[derive(Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
    #[allow(dead_code)]
    id: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct JsonRpcError {
    #[allow(dead_code)]
    code: i64,
    message: String,
}

/// Task ID response from external download_submit
#[derive(Deserialize)]
struct ExternalTaskIdResponse {
    task_id: String,
}

/// Task info from external service (subset we care about)
#[derive(Deserialize, Debug)]
struct ExternalTaskInfo {
    #[allow(dead_code)]
    task_id: String,
    state: String,
    progress: ExternalProgress,
    error: Option<String>,
}

#[derive(Deserialize, Debug)]
struct ExternalProgress {
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    #[allow(dead_code)]
    speed_bytes_per_sec: Option<u64>,
    #[allow(dead_code)]
    eta_seconds: Option<u64>,
}

/// Downloader that delegates all operations to an external JSON-RPC service.
///
/// The external service is expected to implement the full downloader protocol
/// (submit, status, wait_for_change, pause, resume, cancel).
pub struct ExternalRpcDownloader {
    rpc_url: String,
    http_client: reqwest::Client,
    /// url -> external task_id (for routing cancel/pause/resume by url)
    task_mapping: RwLock<HashMap<String, String>>,
    /// Atomic request ID counter
    request_id: std::sync::atomic::AtomicU64,
    capabilities: DownloaderCapabilities,
}

impl ExternalRpcDownloader {
    /// Create a new external RPC downloader pointing at the given service URL.
    pub fn new(rpc_url: String) -> Self {
        Self {
            rpc_url,
            http_client: reqwest::Client::new(),
            task_mapping: RwLock::new(HashMap::new()),
            request_id: std::sync::atomic::AtomicU64::new(1),
            capabilities: DownloaderCapabilities::all_enabled(),
        }
    }

    /// Make a JSON-RPC call to the external service.
    async fn rpc_call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<T> {
        let id = self
            .request_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            method,
            params,
            id,
        };

        let response = self
            .http_client
            .post(&self.rpc_url)
            .json(&request)
            .send()
            .await
            .map_err(|e| DownloadError::network(format!("RPC request failed: {}", e)))?;

        let rpc_response: JsonRpcResponse = response
            .json()
            .await
            .map_err(|e| DownloadError::network(format!("RPC response parse failed: {}", e)))?;

        if let Some(err) = rpc_response.error {
            return Err(DownloadError::network(format!(
                "RPC error: {}",
                err.message
            )));
        }

        let result = rpc_response
            .result
            .ok_or_else(|| DownloadError::network("RPC response missing result"))?;

        serde_json::from_value(result)
            .map_err(|e| DownloadError::network(format!("RPC result deserialize failed: {}", e)))
    }
}

#[async_trait]
impl Downloader for ExternalRpcDownloader {
    async fn download(
        &self,
        url: &str,
        dest: &Path,
        progress: Option<ProgressCallback>,
        options: Option<RequestOptions>,
    ) -> Result<()> {
        // 1. Submit download task to external service
        let params = serde_json::json!({
            "url": url,
            "dest_path": dest.to_str().unwrap_or(""),
            "headers": options.as_ref().and_then(|o| o.headers.clone()),
            "cookies": options.as_ref().and_then(|o| o.cookies.clone()),
        });

        let submit_response: ExternalTaskIdResponse =
            self.rpc_call("download_submit", params).await?;
        let external_task_id = submit_response.task_id;

        // 2. Record mapping for cancel/pause/resume
        self.task_mapping
            .write()
            .insert(url.to_string(), external_task_id.clone());

        // 3. Poll for status changes until terminal state
        loop {
            let params = serde_json::json!({
                "task_id": &external_task_id,
                "timeout_seconds": 30_u64,
            });

            let task_info: ExternalTaskInfo = match self
                .rpc_call("download_wait_for_change", params)
                .await
            {
                Ok(info) => info,
                Err(e) => {
                    // On poll error, try a direct status check
                    let status_params = serde_json::json!({
                        "task_id": &external_task_id,
                    });
                    match self
                        .rpc_call::<ExternalTaskInfo>("download_get_status", status_params)
                        .await
                    {
                        Ok(info) => info,
                        Err(_) => {
                            // Both failed, clean up and return error
                            self.task_mapping.write().remove(url);
                            return Err(e);
                        }
                    }
                }
            };

            // Update progress callback
            if let Some(ref cb) = progress {
                cb(
                    task_info.progress.downloaded_bytes,
                    task_info.progress.total_bytes,
                );
            }

            // Check terminal states
            match task_info.state.as_str() {
                "completed" => {
                    self.task_mapping.write().remove(url);
                    return Ok(());
                }
                "failed" => {
                    self.task_mapping.write().remove(url);
                    let msg = task_info
                        .error
                        .unwrap_or_else(|| "External download failed".to_string());
                    return Err(DownloadError::network(msg));
                }
                "cancelled" => {
                    self.task_mapping.write().remove(url);
                    return Err(DownloadError::cancelled("Download was cancelled"));
                }
                _ => {
                    // pending, downloading, stopped — continue polling
                    continue;
                }
            }
        }
    }

    async fn download_batch(&self, tasks: Vec<(String, PathBuf)>) -> Vec<Result<()>> {
        // Simple sequential implementation for external downloaders
        let mut results = Vec::with_capacity(tasks.len());
        for (url, dest) in tasks {
            let result = self.download(&url, &dest, None, None).await;
            results.push(result);
        }
        results
    }

    fn name(&self) -> &str {
        "external_rpc"
    }

    fn capabilities(&self) -> &DownloaderCapabilities {
        &self.capabilities
    }

    async fn cancel(&self, url: &str) -> Result<()> {
        let ext_id = self.task_mapping.read().get(url).cloned();
        if let Some(ext_id) = ext_id {
            let params = serde_json::json!({"task_id": ext_id});
            let _: bool = self.rpc_call("download_cancel", params).await?;
        }
        Ok(())
    }

    async fn pause(&self, url: &str) -> Result<()> {
        let ext_id = self.task_mapping.read().get(url).cloned();
        if let Some(ext_id) = ext_id {
            let params = serde_json::json!({"task_id": ext_id});
            let _: bool = self.rpc_call("download_pause", params).await?;
        }
        Ok(())
    }

    async fn resume(&self, url: &str) -> Result<()> {
        let ext_id = self.task_mapping.read().get(url).cloned();
        if let Some(ext_id) = ext_id {
            let params = serde_json::json!({"task_id": ext_id});
            let _: bool = self.rpc_call("download_resume", params).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_external_downloader() {
        let dl = ExternalRpcDownloader::new("http://127.0.0.1:12345".to_string());
        assert_eq!(dl.name(), "external_rpc");
        assert!(dl.capabilities().supports_pause);
    }
}
