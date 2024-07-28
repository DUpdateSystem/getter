use async_fn_traits::AsyncFnOnce2;
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::data::release::ReleaseData;
use super::provider::base_provider::{AppDataMap, DataMap, FIn, FOut, FunctionType, HubDataMap};
use super::provider::outside_rpc::OutsideProvider;
use super::provider::{self, add_provider};
use crate::cache::get_cache_manager;
use crate::cache::manager::GroupType;
use crate::utils::json::{bytes_to_json, json_to_bytes};
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct ErrorProviderNotFound;

impl std::fmt::Display for ErrorProviderNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Provider not found for this request.")
    }
}

impl std::error::Error for ErrorProviderNotFound {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

async fn call_func<T, F>(
    uuid: &str,
    app_data: &AppDataMap<'_>,
    hub_data: &HubDataMap<'_>,
    func_type: FunctionType,
    provider_func: F,
) -> Result<Option<T>, ErrorProviderNotFound>
where
    T: Send + DeserializeOwned + Serialize,
    F: for<'b> AsyncFnOnce2<&'b str, &'b FIn<'b>, Output = Option<FOut<T>>>,
{
    let cache_manager = get_cache_manager().await;
    let data_map = DataMap { app_data, hub_data };
    let api_cache_key = data_map.get_hash();
    if let Some(bytes) = cache_manager
        .lock()
        .await
        .get(&GroupType::API, &api_cache_key.to_string(), None)
        .await
    {
        if let Ok(value) = bytes_to_json::<T>(&bytes) {
            return Ok(Some(value));
        }
    }
    let cache_keys = provider::get_cache_request_key(uuid, &func_type, &data_map);
    let mut cache_map = HashMap::new();
    if let Some(keys) = cache_keys {
        for key in keys {
            if let Some(value) = cache_manager
                .lock()
                .await
                .get(&GroupType::REPO_INSIDE, &key, None)
                .await
            {
                cache_map.insert(key, value);
            }
        }
    }

    let fin = FIn::new(data_map, Some(cache_map));
    if let Some(fout) = provider_func(uuid, &fin).await {
        if let Some(cached_map) = fout.cached_map {
            for (key, value) in cached_map {
                let _ = cache_manager
                    .lock()
                    .await
                    .save(&GroupType::REPO_INSIDE, &key, value)
                    .await;
            }
        }
        if let Ok(data) = fout.result {
            if let Ok(value) = json_to_bytes(&data) {
                let _ = cache_manager
                    .lock()
                    .await
                    .save(&GroupType::API, &api_cache_key.to_string(), value)
                    .await;
            }
            return Ok(Some(data));
        } else {
            return Ok(None);
        }
    } else {
        return Err(ErrorProviderNotFound);
    }
}

pub async fn check_app_available<'a>(
    uuid: &str,
    app_data: &AppDataMap<'a>,
    hub_data: &HubDataMap<'a>,
) -> Option<bool> {
    call_func(
        uuid,
        app_data,
        hub_data,
        FunctionType::CheckAppAvailable,
        provider::check_app_available,
    )
    .await
    .unwrap_or(None)
}

pub async fn get_latest_release<'a>(
    uuid: &str,
    app_data: &AppDataMap<'a>,
    hub_data: &HubDataMap<'a>,
) -> Option<ReleaseData> {
    call_func(
        uuid,
        app_data,
        hub_data,
        FunctionType::GetLatestRelease,
        provider::get_latest_release,
    )
    .await
    .unwrap_or(None)
}

pub async fn get_releases<'a>(
    uuid: &str,
    app_data: &AppDataMap<'a>,
    hub_data: &HubDataMap<'a>,
) -> Option<Vec<ReleaseData>> {
    call_func(
        uuid,
        app_data,
        hub_data,
        FunctionType::GetReleases,
        provider::get_releases,
    )
    .await
    .unwrap_or(None)
}

pub fn add_outside_provider(uuid: &str, url: &str) {
    let provider = OutsideProvider {
        uuid: uuid.to_string(),
        url: url.to_string(),
    };
    add_provider(uuid, provider);
}
