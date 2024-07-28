use crate::websdk::repo::data::release::ReleaseData;

use super::data::*;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::core::client::Error;
use jsonrpsee::http_client::HttpClient;
use jsonrpsee::http_client::HttpClientBuilder;
use std::collections::BTreeMap;

pub struct Client {
    client: HttpClient,
}

impl Client {
    pub fn new(url: impl AsRef<str>) -> Result<Self, Error> {
        let client = HttpClientBuilder::default().build(url)?;
        Ok(Self { client })
    }

    pub async fn check_app_available(
        &self,
        hub_uuid: &str,
        app_data: BTreeMap<&str, &str>,
        hub_data: BTreeMap<&str, &str>,
    ) -> Result<bool, Error> {
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
    ) -> Result<ReleaseData, Error> {
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
    ) -> Result<Vec<ReleaseData>, Error> {
        let data = RpcAppRequest {
            hub_uuid,
            app_data,
            hub_data,
        };
        let response = self.client.request("get_releases", data).await;
        Ok(response?)
    }
}
