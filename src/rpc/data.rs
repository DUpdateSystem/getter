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
