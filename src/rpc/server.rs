use crate::websdk::repo::api;
use jsonrpsee::server::{RpcModule, Server, ServerHandle};
use jsonrpsee::types::{ErrorCode, ErrorObjectOwned};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::net::SocketAddr;

#[derive(Serialize, Deserialize)]
pub struct RpcAppRequest<'a> {
    pub uuid: &'a str,
    pub app_data: BTreeMap<&'a str, &'a str>,
    pub hub_data: BTreeMap<&'a str, &'a str>,
}

pub async fn run_serser(addr: &str) -> Result<(String, ServerHandle), Box<dyn std::error::Error>> {
    let server = Server::builder().build(addr.parse::<SocketAddr>()?).await?;
    let mut module = RpcModule::new(());
    module.register_async_method("check_app_available", |params, _context| async move {
        let request = params.parse::<RpcAppRequest>()?;
        if let Some(result) =
            api::check_app_available(&request.uuid, &request.app_data, &request.hub_data).await
        {
            Ok(json!(result))
        } else {
            Err(ErrorObjectOwned::borrowed(
                ErrorCode::InvalidParams.code(),
                "Invalid params",
                None,
            ))
        }
    })?;
    module.register_async_method("get_latest_release", |params, _context| async move {
        if let Ok(request) = params.parse::<RpcAppRequest>() {
            if let Some(result) =
                api::get_latest_release(&request.uuid, &request.app_data, &request.hub_data).await
            {
                Ok(json!(result))
            } else {
                Err(ErrorObjectOwned::borrowed(
                    ErrorCode::InvalidParams.code(),
                    "Invalid params",
                    None,
                ))
            }
        } else {
            Err(ErrorObjectOwned::borrowed(
                ErrorCode::InvalidParams.code(),
                "Parse params error",
                None,
            ))
        }
    })?;
    module.register_async_method("get_releases", |params, _context| async move {
        if let Ok(request) = params.parse::<RpcAppRequest>() {
            if let Some(result) =
                api::get_releases(&request.uuid, &request.app_data, &request.hub_data).await
            {
                Ok(json!(result))
            } else {
                Err(ErrorObjectOwned::borrowed(
                    ErrorCode::InvalidParams.code(),
                    "Invalid params",
                    None,
                ))
            }
        } else {
            Err(ErrorObjectOwned::borrowed(
                ErrorCode::InvalidParams.code(),
                "Parse params error",
                None,
            ))
        }
    })?;
    let addr = server.local_addr()?;
    let handle = server.start(module);
    Ok((format!("http://{}", addr), handle))
}
