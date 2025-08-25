use crate::types::*;
use hyper::{body::Bytes, Method, Request, Uri};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use serde_json::{json, Value};
use std::error::Error;
use std::fmt;

type Body = http_body_util::Full<Bytes>;

#[derive(Debug)]
pub struct RpcError {
    pub message: String,
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RPC Error: {}", self.message)
    }
}

impl Error for RpcError {}

pub struct GetterRpcClient {
    client: Client<hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>, Body>,
    server_url: String,
    request_id: std::sync::atomic::AtomicU64,
}

impl GetterRpcClient {
    pub fn new_http(url: &str) -> Self {
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .unwrap()
            .https_or_http()
            .enable_http1()
            .build();
        
        let client = Client::builder(TokioExecutor::new()).build(https);
        
        Self {
            client,
            server_url: url.to_string(),
            request_id: std::sync::atomic::AtomicU64::new(1),
        }
    }

    async fn send_request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.request_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        
        let rpc_request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: Some(params),
            id: json!(id),
        };

        let json_body = serde_json::to_string(&rpc_request)
            .map_err(|e| RpcError { message: format!("JSON serialization error: {}", e) })?;

        let uri: Uri = self.server_url.parse()
            .map_err(|e| RpcError { message: format!("Invalid URL: {}", e) })?;

        let req = Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header("Content-Type", "application/json")
            .body(Body::from(json_body))
            .map_err(|e| RpcError { message: format!("Request build error: {}", e) })?;

        let resp = self.client.request(req).await
            .map_err(|e| RpcError { message: format!("HTTP request error: {}", e) })?;

        let body_bytes = http_body_util::BodyExt::collect(resp.into_body()).await
            .map_err(|e| RpcError { message: format!("Body read error: {}", e) })?
            .to_bytes();

        let rpc_response: RpcResponse = serde_json::from_slice(&body_bytes)
            .map_err(|e| RpcError { message: format!("JSON parse error: {}", e) })?;

        if let Some(error) = rpc_response.error {
            return Err(RpcError { message: error.message });
        }

        Ok(rpc_response.result.unwrap_or(Value::Null))
    }

    pub async fn add_app(&self, app_data: serde_json::Value) -> Result<(), RpcError> {
        // Extract fields from app_data Value
        let app_id = app_data["app_id"].as_str().unwrap_or("").to_string();
        let hub_uuid = app_data["hub_uuid"].as_str().unwrap_or("github").to_string();
        let app_data_obj: std::collections::HashMap<String, String> = 
            serde_json::from_value(app_data["app_data"].clone()).unwrap_or_default();
        let hub_data_obj: std::collections::HashMap<String, String> = 
            serde_json::from_value(app_data["hub_data"].clone()).unwrap_or_default();
        
        let params = json!(AddAppRequest {
            app_id,
            hub_uuid,
            app_data: app_data_obj,
            hub_data: hub_data_obj,
        });
        self.send_request("add_app", params).await?;
        Ok(())
    }

    pub async fn remove_app(&self, app_id: String) -> Result<(), RpcError> {
        let params = json!(RemoveAppRequest { app_id });
        self.send_request("remove_app", params).await?;
        Ok(())
    }

    pub async fn list_apps(&self) -> Result<Vec<String>, RpcError> {
        let result = self.send_request("list_apps", Value::Null).await?;
        serde_json::from_value(result)
            .map_err(|e| RpcError { message: format!("Response parse error: {}", e) })
    }

    pub async fn update_app(&self, app_id: String) -> Result<(), RpcError> {
        let params = json!(UpdateAppRequest { app_id });
        self.send_request("update_app", params).await?;
        Ok(())
    }

    pub async fn get_app_status(&self, app_id: String) -> Result<Option<AppStatusInfo>, RpcError> {
        let params = json!(GetAppStatusRequest { app_id });
        let result = self.send_request("get_app_status", params).await?;
        if result.is_null() {
            Ok(None)
        } else {
            let status = serde_json::from_value(result)
                .map_err(|e| RpcError { message: format!("Response parse error: {}", e) })?;
            Ok(Some(status))
        }
    }

    pub async fn check_app_available(&self, app_id: String) -> Result<bool, RpcError> {
        let params = json!(CheckAppAvailableRequest { app_id });
        let result = self.send_request("check_app_available", params).await?;
        serde_json::from_value(result)
            .map_err(|e| RpcError { message: format!("Response parse error: {}", e) })
    }

    pub async fn get_latest_release(&self, app_id: String) -> Result<Option<ReleaseInfo>, RpcError> {
        let params = json!(GetLatestReleaseRequest { app_id });
        let result = self.send_request("get_latest_release", params).await?;
        if result.is_null() {
            Ok(None)
        } else {
            let release = serde_json::from_value(result)
                .map_err(|e| RpcError { message: format!("Response parse error: {}", e) })?;
            Ok(Some(release))
        }
    }

    pub async fn get_outdated_apps(&self) -> Result<Vec<String>, RpcError> {
        let result = self.send_request("get_outdated_apps", Value::Null).await?;
        serde_json::from_value(result)
            .map_err(|e| RpcError { message: format!("Response parse error: {}", e) })
    }
}