use crate::websdk::repo::api;
use crate::api as api_root;
use jsonrpsee::core::traits::ToRpcParams;
use jsonrpsee::server::{RpcModule, Server, ServerHandle};
use jsonrpsee::types::{ErrorCode, ErrorObjectOwned};
use serde::{Deserialize, Serialize};
use serde_json::value::to_raw_value;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;

#[derive(Serialize, Deserialize, Debug)]
pub struct RpcInitRequest<'a> {
    pub data_dir: &'a str,
    pub cache_dir: &'a str,
    pub global_expire_time: u64,
}

impl ToRpcParams for RpcInitRequest<'_> {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RpcAppRequest<'a> {
    pub uuid: &'a str,
    pub app_data: BTreeMap<&'a str, &'a str>,
    pub hub_data: BTreeMap<&'a str, &'a str>,
}

impl ToRpcParams for RpcAppRequest<'_> {
    fn to_rpc_params(self) -> Result<Option<Box<serde_json::value::RawValue>>, serde_json::Error> {
        to_raw_value(&self).map(Some)
    }
}

pub async fn run_serser(addr: &str) -> Result<(String, ServerHandle), Box<dyn std::error::Error>> {
    let addr = if addr.is_empty() { "127.0.0.1:0" } else { addr };
    let server = Server::builder().build(addr.parse::<SocketAddr>()?).await?;
    let mut module = RpcModule::new(());
    module.register_async_method("init", |params, _| async move {
        let request = params.parse::<RpcInitRequest>()?;
        let data_dir = Path::new(request.data_dir);
        let cache_dir = Path::new(request.cache_dir);
        api_root::init(data_dir, cache_dir, request.global_expire_time).await.map_err(|e| {
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
            api::check_app_available(&request.uuid, &request.app_data, &request.hub_data).await
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
                api::get_latest_release(&request.uuid, &request.app_data, &request.hub_data).await
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
                api::get_releases(&request.uuid, &request.app_data, &request.hub_data).await
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
    Ok((format!("http://{}", addr), handle))
}

#[cfg(test)]
mod tests {
    use crate::websdk::repo::data::release::ReleaseData;

    use super::*;
    use jsonrpsee::{core::client::ClientT, http_client::HttpClientBuilder};
    use mockito::Server;
    use std::fs;

    #[tokio::test]
    async fn test_server_start() {
        let (url, handle) = run_serser("").await.unwrap();
        println!("Server started at {}", url);
        assert!(url.starts_with("http://"));
        assert!(url.split(":").last().unwrap().parse::<u16>().unwrap() > 0);
        handle.stop().unwrap();
        let port = 33333;
        let addr = format!("127.0.0.1:{}", port);
        let (url, handle) = run_serser(&addr).await.unwrap();
        println!("Server started at {}", url);
        assert!(url.starts_with("http://"));
        assert!(url.split(":").last().unwrap().parse::<u16>().unwrap() == port);
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
        let proxy_url = format!("{} -> {}", "https://github.com", server.url());
        let hub_data = BTreeMap::from([("reverse_proxy", proxy_url.as_str())]);

        let (url, handle) = run_serser("").await.unwrap();
        println!("Server started at {}", url);
        assert!(url.starts_with("http://"));
        assert!(url.split(":").last().unwrap().parse::<u16>().unwrap() > 0);
        let client = HttpClientBuilder::default().build(url).unwrap();
        let params = RpcAppRequest {
            uuid: "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
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
        let proxy_url = format!("{} -> {}", "https://github.com", server.url());
        let hub_data = BTreeMap::from([("reverse_proxy", proxy_url.as_str())]);

        let (url, handle) = run_serser("").await.unwrap();
        println!("Server started at {}", url);
        assert!(url.starts_with("http://"));
        assert!(url.split(":").last().unwrap().parse::<u16>().unwrap() > 0);
        let client = HttpClientBuilder::default().build(url).unwrap();
        let params = RpcAppRequest {
            uuid: "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
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

        let (url, handle) = run_serser("").await.unwrap();
        println!("Server started at {}", url);
        assert!(url.starts_with("http://"));
        assert!(url.split(":").last().unwrap().parse::<u16>().unwrap() > 0);
        let client = HttpClientBuilder::default().build(url).unwrap();
        let params = RpcAppRequest {
            uuid: "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
            app_data: id_map,
            hub_data,
        };
        println!("{:?}", params);
        let response: Result<Vec<ReleaseData>, _> = client.request("get_releases", params).await;
        let releases = response.unwrap();
        assert!(!releases.is_empty());
        handle.stop().unwrap();
    }
}
