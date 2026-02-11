use super::data::*;
use crate::api as api_root;
use crate::downloader::{DownloadConfig, DownloadTaskManager};
use crate::websdk::cloud_rules::cloud_rules_manager::CloudRules;
use crate::websdk::repo::api;
use jsonrpsee::server::{RpcModule, Server, ServerHandle};
use jsonrpsee::types::{ErrorCode, ErrorObjectOwned};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

// Default 2GB size limit for WebSocket messages
// Can be overridden at compile time by setting MAX_WS_MESSAGE_SIZE environment variable
// Example: MAX_WS_MESSAGE_SIZE=1073741824 cargo build (for 1GB)
const DEFAULT_MAX_SIZE: u32 = 2 * 1024 * 1024 * 1024; // 2GB

fn get_max_message_size() -> u32 {
    // Allow compile-time configuration via environment variable
    match option_env!("MAX_WS_MESSAGE_SIZE") {
        Some(size_str) => size_str.parse().unwrap_or(DEFAULT_MAX_SIZE),
        None => DEFAULT_MAX_SIZE,
    }
}

pub async fn run_server(
    addr: &str,
    is_running: Arc<AtomicBool>,
) -> Result<(String, ServerHandle), Box<dyn std::error::Error>> {
    let addr = if addr.is_empty() { "127.0.0.1:0" } else { addr };
    let max_size = get_max_message_size();
    let server = Server::builder()
        .max_request_body_size(max_size)
        .max_response_body_size(max_size)
        .build(addr.parse::<SocketAddr>()?).await?;
    let mut module = RpcModule::new(());
    // Register the shutdown method
    let run_flag = is_running.clone();
    module.register_async_method("shutdown", move |_, _, _| {
        let flag = run_flag.clone();
        async move {
            flag.store(false, Ordering::SeqCst);
        }
    })?;
    module.register_method("ping", |_, _, _| "pong")?;
    module.register_async_method("init", |params, _, _| async move {
        let request = params.parse::<RpcInitRequest>()?;
        let data_dir = Path::new(request.data_path);
        let cache_dir = Path::new(request.cache_path);
        api_root::init(data_dir, cache_dir, request.global_expire_time)
            .await
            .map(|_| true)
            .map_err(|e| {
                ErrorObjectOwned::owned(
                    ErrorCode::InternalError.code(),
                    "Internal error",
                    Some(e.to_string()),
                )
            })
    })?;
    module.register_async_method(
        "check_app_available",
        |params, _context, _extensions| async move {
            let request = params.parse::<RpcAppRequest>()?;
            if let Some(result) =
                api::check_app_available(request.hub_uuid, &request.app_data, &request.hub_data)
                    .await
            {
                Ok(result)
            } else {
                Err(ErrorObjectOwned::owned(
                    ErrorCode::ParseError.code(),
                    "Parse params error",
                    Some(params.as_str().unwrap_or("None").to_string()),
                ))
            }
        },
    )?;
    module.register_async_method(
        "get_latest_release",
        |params, _context, _extensions| async move {
            if let Ok(request) = params.parse::<RpcAppRequest>() {
                if let Some(result) =
                    api::get_latest_release(request.hub_uuid, &request.app_data, &request.hub_data)
                        .await
                {
                    Ok(result)
                } else {
                    Err(ErrorObjectOwned::borrowed(
                        ErrorCode::InvalidParams.code(),
                        "Invalid params",
                        None,
                    ))
                }
            } else {
                Err(ErrorObjectOwned::owned(
                    ErrorCode::ParseError.code(),
                    "Parse params error",
                    Some(params.as_str().unwrap_or("None").to_string()),
                ))
            }
        },
    )?;
    module.register_async_method("get_releases", |params, _context, _extensions| async move {
        if let Ok(request) = params.parse::<RpcAppRequest>() {
            if let Some(result) =
                api::get_releases(request.hub_uuid, &request.app_data, &request.hub_data).await
            {
                Ok(result)
            } else {
                Err(ErrorObjectOwned::borrowed(
                    ErrorCode::InvalidParams.code(),
                    "Invalid params",
                    None,
                ))
            }
        } else {
            Err(ErrorObjectOwned::owned(
                ErrorCode::ParseError.code(),
                "Parse params error",
                Some(params.as_str().unwrap_or("None").to_string()),
            ))
        }
    })?;

    module.register_async_method(
        "get_cloud_config",
        |params, _context, _extensions| async move {
            if let Ok(request) = params.parse::<RpcCloudConfigRequest>() {
                let mut cloud_rules = CloudRules::new(request.api_url);
                if let Err(e) = cloud_rules.renew().await {
                    return Err(ErrorObjectOwned::owned(
                        ErrorCode::InternalError.code(),
                        "Download cloud config failed",
                        Some(e.to_string()),
                    ));
                }
                Ok(cloud_rules.get_config_list().to_owned())
            } else {
                Err(ErrorObjectOwned::owned(
                    ErrorCode::ParseError.code(),
                    "Parse params error",
                    Some(params.as_str().unwrap_or("None").to_string()),
                ))
            }
        },
    )?;

    // ========================================================================
    // Downloader RPC Methods
    // ========================================================================

    // Create download task manager
    let download_config = DownloadConfig::from_env();
    let task_manager = Arc::new(DownloadTaskManager::from_config(&download_config));

    // download_submit: Submit a single download task
    let manager_clone = task_manager.clone();
    module.register_async_method("download_submit", move |params, _context, _extensions| {
        let manager = manager_clone.clone();
        async move {
            let request = params.parse::<RpcDownloadRequest>()?;
            match manager.submit_task_with_options(
                request.url,
                request.dest_path,
                request.headers,
                request.cookies,
            ) {
                Ok(task_id) => Ok(RpcTaskIdResponse { task_id }),
                Err(e) => Err(ErrorObjectOwned::owned(
                    ErrorCode::InternalError.code(),
                    "Failed to submit download task",
                    Some(e.message),
                )),
            }
        }
    })?;

    // download_submit_batch: Submit multiple download tasks
    let manager_clone = task_manager.clone();
    module.register_async_method(
        "download_submit_batch",
        move |params, _context, _extensions| {
            let manager = manager_clone.clone();
            async move {
                let request = params.parse::<RpcDownloadBatchRequest>()?;
                let tasks: Vec<(String, String)> = request
                    .tasks
                    .into_iter()
                    .map(|t| (t.url, t.dest_path))
                    .collect();

                match manager.submit_batch(tasks) {
                    Ok(task_ids) => Ok(RpcTaskIdsResponse { task_ids }),
                    Err(e) => Err(ErrorObjectOwned::owned(
                        ErrorCode::InternalError.code(),
                        "Failed to submit batch download tasks",
                        Some(e.message),
                    )),
                }
            }
        },
    )?;

    // download_get_status: Get status of a download task
    let manager_clone = task_manager.clone();
    module.register_async_method(
        "download_get_status",
        move |params, _context, _extensions| {
            let manager = manager_clone.clone();
            async move {
                let request = params.parse::<RpcTaskStatusRequest>()?;
                match manager.get_task(request.task_id) {
                    Ok(task_info) => Ok(task_info),
                    Err(e) => Err(ErrorObjectOwned::owned(
                        ErrorCode::InvalidParams.code(),
                        "Task not found",
                        Some(e.message),
                    )),
                }
            }
        },
    )?;

    // download_wait_for_change: Long-polling for task state change
    let manager_clone = task_manager.clone();
    module.register_async_method(
        "download_wait_for_change",
        move |params, _context, _extensions| {
            let manager = manager_clone.clone();
            async move {
                let request = params.parse::<RpcWaitForChangeRequest>()?;
                let timeout = Duration::from_secs(request.timeout_seconds);

                match manager.wait_for_change(request.task_id, timeout).await {
                    Ok(task_info) => Ok(task_info),
                    Err(e) => Err(ErrorObjectOwned::owned(
                        ErrorCode::InvalidParams.code(),
                        "Failed to wait for task change",
                        Some(e.message),
                    )),
                }
            }
        },
    )?;

    // download_cancel: Cancel a download task
    let manager_clone = task_manager.clone();
    module.register_async_method("download_cancel", move |params, _context, _extensions| {
        let manager = manager_clone.clone();
        async move {
            let request = params.parse::<RpcCancelTaskRequest>()?;
            match manager.cancel_task(request.task_id) {
                Ok(_) => Ok(true),
                Err(e) => Err(ErrorObjectOwned::owned(
                    ErrorCode::InternalError.code(),
                    "Failed to cancel task",
                    Some(e.message),
                )),
            }
        }
    })?;

    // download_pause: Pause a download task
    let manager_clone = task_manager.clone();
    module.register_async_method("download_pause", move |params, _context, _extensions| {
        let manager = manager_clone.clone();
        async move {
            let request = params.parse::<RpcPauseTaskRequest>()?;
            match manager.pause_task(request.task_id).await {
                Ok(_) => Ok(true),
                Err(e) => Err(ErrorObjectOwned::owned(
                    ErrorCode::InternalError.code(),
                    "Failed to pause task",
                    Some(e.message),
                )),
            }
        }
    })?;

    // download_resume: Resume a paused download task
    let manager_clone = task_manager.clone();
    module.register_async_method("download_resume", move |params, _context, _extensions| {
        let manager = manager_clone.clone();
        async move {
            let request = params.parse::<RpcResumeTaskRequest>()?;
            match manager.resume_task(request.task_id).await {
                Ok(_) => Ok(true),
                Err(e) => Err(ErrorObjectOwned::owned(
                    ErrorCode::InternalError.code(),
                    "Failed to resume task",
                    Some(e.message),
                )),
            }
        }
    })?;

    // download_get_capabilities: Get downloader capabilities
    let manager_clone = task_manager.clone();
    module.register_method(
        "download_get_capabilities",
        move |_, _context, _extensions| {
            let caps = manager_clone.get_capabilities();
            Ok::<_, ErrorObjectOwned>(caps.clone())
        },
    )?;

    // download_get_all_tasks: Get all tasks
    let manager_clone = task_manager.clone();
    module.register_async_method("download_get_all_tasks", move |_, _context, _extensions| {
        let manager = manager_clone.clone();
        async move {
            Ok::<RpcTasksResponse, ErrorObjectOwned>(RpcTasksResponse {
                tasks: manager.get_all_tasks(),
            })
        }
    })?;

    // download_get_active_tasks: Get active tasks
    let manager_clone = task_manager.clone();
    module.register_async_method(
        "download_get_active_tasks",
        move |_, _context, _extensions| {
            let manager = manager_clone.clone();
            async move {
                Ok::<RpcTasksResponse, ErrorObjectOwned>(RpcTasksResponse {
                    tasks: manager.get_active_tasks(),
                })
            }
        },
    )?;

    // download_get_tasks_by_state: Get tasks by state
    let manager_clone = task_manager.clone();
    module.register_async_method(
        "download_get_tasks_by_state",
        move |params, _context, _extensions| {
            let manager = manager_clone.clone();
            async move {
                let request = params.parse::<RpcTasksByStateRequest>()?;
                Ok::<RpcTasksResponse, ErrorObjectOwned>(RpcTasksResponse {
                    tasks: manager.get_tasks_by_state(request.state),
                })
            }
        },
    )?;

    let addr = server.local_addr()?;
    let handle = server.start(module);
    tokio::spawn(handle.clone().stopped());
    Ok((format!("http://{}", addr), handle))
}

#[allow(dead_code)]
pub async fn run_server_hanging<T>(
    addr: &str,
    callback: impl Fn(&str) -> Result<T, Box<dyn std::error::Error>>,
) -> Result<T, Box<dyn std::error::Error>> {
    let is_running = Arc::new(AtomicBool::new(true));
    let (url, handle) = match run_server(addr, is_running.clone()).await {
        Ok((url, handle)) => (url, handle),
        Err(e) => {
            eprintln!("Failed to start server: {}", e);
            return Err(e);
        }
    };
    let result = callback(&url)?;
    while is_running.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    handle.stop()?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use crate::rpc::client::Client;
    use crate::websdk::repo::provider::github;
    use crate::websdk::{
        cloud_rules::data::config_list::ConfigList, repo::data::release::ReleaseData,
    };

    use super::*;
    use jsonrpsee::{core::client::ClientT, http_client::HttpClientBuilder, rpc_params};
    use mockito::Server;
    use std::collections::BTreeMap;
    use std::fs;
    use tokio::time::timeout;

    #[tokio::test]
    async fn test_server_start() {
        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        assert!(url.starts_with("http://"));
        assert!(url.split(":").last().unwrap().parse::<u16>().unwrap() > 0);
        handle.stop().unwrap();
        let port = 33333;
        let addr = format!("127.0.0.1:{}", port);
        let (url, handle) = run_server(&addr, Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        assert!(url.starts_with("http://"));
        assert!(url.split(":").last().unwrap().parse::<u16>().unwrap() == port);
        handle.stop().unwrap();
    }

    #[tokio::test]
    async fn test_ping() {
        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        let client = HttpClientBuilder::default().build(url).unwrap();
        let response: Result<String, _> = client.request("ping", rpc_params![]).await;
        assert_eq!(response.unwrap(), "pong");
        handle.stop().unwrap();
    }

    #[tokio::test]
    async fn test_init() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/DUpdateSystem/UpgradeAll")
            .with_status(200)
            .create_async()
            .await;

        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        let client = HttpClientBuilder::default().build(url).unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_dir_path = temp_dir.path().to_str().unwrap();
        let params = RpcInitRequest {
            data_path: &format!("{}/data", temp_dir_path),
            cache_path: &format!("{}/cache", temp_dir_path),
            global_expire_time: 3600,
        };
        println!("{:?}", params);
        let response: Result<bool, _> = client.request("init", params).await;
        assert!(response.unwrap());
        handle.stop().unwrap();
    }
    #[tokio::test]
    async fn test_check_app_available() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/DUpdateSystem/UpgradeAll")
            .with_status(200)
            .create_async()
            .await;

        let id_map = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
        let proxy_url = format!("{} -> {}", github::GITHUB_API_URL, server.url());
        let hub_data = BTreeMap::from([("reverse_proxy", proxy_url.as_str())]);

        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        let params = RpcAppRequest {
            hub_uuid: "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
            app_data: id_map,
            hub_data,
        };
        println!("{:?}", params);
        let client = Client::new(url).unwrap();
        let response: Result<bool, _> = client
            .check_app_available(params.hub_uuid, params.app_data, params.hub_data)
            .await;
        assert!(response.unwrap());
        handle.stop().unwrap();
    }

    #[tokio::test]
    async fn test_get_latest_release() {
        let body = fs::read_to_string("tests/files/web/github_api_release.json").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/DUpdateSystem/UpgradeAll/releases")
            .with_status(200)
            .with_body(body)
            .create();

        let id_map = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
        let proxy_url = format!("{} -> {}", github::GITHUB_API_URL, server.url());
        let hub_data = BTreeMap::from([("reverse_proxy", proxy_url.as_str())]);

        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        let params = RpcAppRequest {
            hub_uuid: "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
            app_data: id_map,
            hub_data,
        };
        println!("{:?}", params);
        let client = Client::new(url).unwrap();
        let response: Result<ReleaseData, _> = client
            .get_latest_release(params.hub_uuid, params.app_data, params.hub_data)
            .await;
        let release = response.unwrap();
        assert!(!release.version_number.is_empty());
        handle.stop().unwrap();
    }

    #[tokio::test]
    async fn test_get_releases() {
        let body = fs::read_to_string("tests/files/web/github_api_release.json").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/DUpdateSystem/UpgradeAll/releases")
            .with_status(200)
            .with_body(body)
            .create();

        let id_map = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
        let proxy_url = format!("{} -> {}", github::GITHUB_API_URL, server.url());
        let hub_data = BTreeMap::from([("reverse_proxy", proxy_url.as_str())]);

        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        let params = RpcAppRequest {
            hub_uuid: "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
            app_data: id_map,
            hub_data,
        };
        println!("{:?}", params);
        let client = Client::new(url).unwrap();
        let response: Result<Vec<ReleaseData>, _> = client
            .get_releases(params.hub_uuid, params.app_data, params.hub_data)
            .await;
        let releases = response.unwrap();
        assert!(!releases.is_empty());
        handle.stop().unwrap();
    }

    #[tokio::test]
    async fn test_run_server_hanging() {
        let addr = "127.0.0.1:33334";
        let server_task = tokio::spawn(async move {
            // This should run the server and wait for the shutdown command
            run_server_hanging(addr, |url| {
                println!("Server started at {}", url);
                Ok(())
            })
            .await
            .expect("Server failed to run");
        });

        // Allow some time for the server to start up
        tokio::time::sleep(Duration::from_millis(500)).await;

        // The callback should print the URL, but since we cannot capture that output easily in a test,
        // we assume the server starts correctly if no error happens till now.
        // Here, manually create a client and send a shutdown request
        let client = HttpClientBuilder::default()
            .build(format!("http://{}", addr))
            .expect("Failed to build client");

        let response: Result<(), _> = client.request("shutdown", rpc_params![]).await;
        assert!(response.is_ok(), "Failed to shutdown server");

        // Allow some time for the server to shut down
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Check if the shutdown was successful by confirming the server task is done
        if timeout(Duration::from_secs(1), server_task).await.is_err() {
            panic!("The server did not shut down within the expected time");
        }

        let response: Result<(), _> = client.request("shutdown", rpc_params![]).await;
        assert!(response.is_err(), "Server should not be running");
    }

    #[tokio::test]
    async fn test_get_cloud_config() {
        let body = fs::read_to_string("tests/files/web/cloud_config.json").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/cloud_config.json")
            .with_status(200)
            .with_body(body)
            .create();

        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        let client = HttpClientBuilder::default().build(url).unwrap();
        let url = format!("{}/cloud_config.json", server.url());
        let params = RpcCloudConfigRequest { api_url: &url };
        println!("{:?}", params);
        let response: Result<ConfigList, _> = client.request("get_cloud_config", params).await;
        let config = response.unwrap();
        assert!(!config.app_config_list.is_empty());
        assert!(!config.hub_config_list.is_empty());
        handle.stop().unwrap();
    }
}
