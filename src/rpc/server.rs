use crate::api as api_root;
use crate::websdk::repo::api;
use jsonrpsee::core::traits::ToRpcParams;
use jsonrpsee::server::{RpcModule, Server, ServerHandle};
use jsonrpsee::types::{ErrorCode, ErrorObjectOwned};
use serde::{Deserialize, Serialize};
use serde_json::value::to_raw_value;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

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

pub async fn run_server(
    addr: &str,
    is_running: Arc<AtomicBool>,
) -> Result<(String, ServerHandle), Box<dyn std::error::Error>> {
    let addr = if addr.is_empty() { "127.0.0.1:0" } else { addr };
    let server = Server::builder().build(addr.parse::<SocketAddr>()?).await?;
    let mut module = RpcModule::new(());
    // Register the shutdown method
    let run_flag = is_running.clone();
    module.register_async_method("shutdown", move |_, _| {
        let flag = run_flag.clone();
        async move {
            flag.store(false, Ordering::SeqCst);
        }
    })?;
    module.register_method("ping", |_, _| "pong")?;
    module.register_async_method("init", |params, _| async move {
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
    module.register_async_method("check_app_available", |params, _context| async move {
        let request = params.parse::<RpcAppRequest>()?;
        if let Some(result) =
            api::check_app_available(&request.hub_uuid, &request.app_data, &request.hub_data).await
        {
            Ok(result)
        } else {
            Err(ErrorObjectOwned::owned(
                ErrorCode::ParseError.code(),
                "Parse params error",
                Some(params.as_str().unwrap_or("None").to_string()),
            ))
        }
    })?;
    module.register_async_method("get_latest_release", |params, _context| async move {
        if let Ok(request) = params.parse::<RpcAppRequest>() {
            if let Some(result) =
                api::get_latest_release(&request.hub_uuid, &request.app_data, &request.hub_data)
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
    })?;
    module.register_async_method("get_releases", |params, _context| async move {
        if let Ok(request) = params.parse::<RpcAppRequest>() {
            if let Some(result) =
                api::get_releases(&request.hub_uuid, &request.app_data, &request.hub_data).await
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
    use crate::websdk::repo::data::release::ReleaseData;

    use super::*;
    use jsonrpsee::{core::client::ClientT, http_client::HttpClientBuilder, rpc_params};
    use mockito::Server;
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
        assert_eq!(response.unwrap(), true);
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
        let proxy_url = format!("{} -> {}", "https://api.github.com", server.url());
        let hub_data = BTreeMap::from([("reverse_proxy", proxy_url.as_str())]);

        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        let client = HttpClientBuilder::default().build(url).unwrap();
        let params = RpcAppRequest {
            hub_uuid: "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
            app_data: id_map,
            hub_data,
        };
        println!("{:?}", params);
        let response: Result<bool, _> = client.request("check_app_available", params).await;
        assert_eq!(response.unwrap(), true);
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
        let proxy_url = format!("{} -> {}", "https://api.github.com", server.url());
        let hub_data = BTreeMap::from([("reverse_proxy", proxy_url.as_str())]);

        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        let client = HttpClientBuilder::default().build(url).unwrap();
        let params = RpcAppRequest {
            hub_uuid: "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
            app_data: id_map,
            hub_data,
        };
        println!("{:?}", params);
        let response: Result<ReleaseData, _> = client.request("get_latest_release", params).await;
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
        let proxy_url = format!("{} -> {}", "https://github.com", server.url());
        let hub_data = BTreeMap::from([("reverse_proxy", proxy_url.as_str())]);

        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        let client = HttpClientBuilder::default().build(url).unwrap();
        let params = RpcAppRequest {
            hub_uuid: "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
            app_data: id_map,
            hub_data,
        };
        println!("{:?}", params);
        let response: Result<Vec<ReleaseData>, _> = client.request("get_releases", params).await;
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
}
