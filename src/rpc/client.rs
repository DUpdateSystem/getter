use crate::websdk::repo::data::release::ReleaseData;

use super::data::*;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::core::client::Error;
use jsonrpsee::http_client::HttpClient;
use std::collections::BTreeMap;

pub struct Client {
    client: HttpClient,
}

impl Client {
    pub fn new(url: &str) -> Result<Self, Error> {
        let client = HttpClient::builder().build(url)?;
        Ok(Self { client })
    }

    pub async fn check_app_available(
        &self,
        hub_uuid: &str,
        app_data: BTreeMap<&str, &str>,
        hub_data: BTreeMap<&str, &str>,
    ) -> Result<Option<bool>, Error> {
        let data = RpcAppRequest {
            hub_uuid,
            app_data,
            hub_data,
        };
        let response = self.client.request("check_app_available", data).await;
        Ok(response?)
    }

    pub async fn get_latest_release(
        &self,
        hub_uuid: &str,
        app_data: BTreeMap<&str, &str>,
        hub_data: BTreeMap<&str, &str>,
    ) -> Result<Option<ReleaseData>, Error> {
        let data = RpcAppRequest {
            hub_uuid,
            app_data,
            hub_data,
        };
        let response = self.client.request("get_latest_release", data).await;
        Ok(response?)
    }

    pub async fn get_releases(
        &self,
        hub_uuid: &str,
        app_data: BTreeMap<&str, &str>,
        hub_data: BTreeMap<&str, &str>,
    ) -> Result<Option<Vec<ReleaseData>>, Error> {
        let data = RpcAppRequest {
            hub_uuid,
            app_data,
            hub_data,
        };
        let response = self.client.request("get_releases", data).await;
        Ok(response?)
    }
}
