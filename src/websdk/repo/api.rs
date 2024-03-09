use async_fn_traits::AsyncFnOnce2;
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::data::release::ReleaseData;
use super::provider;
use super::provider::base_provider::{AppDataMap, DataMap, FIn, FOut, FunctionType, HubDataMap};
use crate::cache::manager::GroupType;
use crate::get_cache_manager;
use crate::utils::json::{bytes_to_json, json_to_bytes};
use std::collections::HashMap;

async fn process_data<T, F>(
    uuid: &str,
    app_data: &AppDataMap<'_>,
    hub_data: &HubDataMap<'_>,
    func_type: FunctionType,
    provider_func: F,
) -> Option<T>
where
    T: Send + DeserializeOwned + Serialize,
    F: for<'b> AsyncFnOnce2<&'b str, &'b FIn<'b>, Output = Option<FOut<T>>>,
{
    let data_map = DataMap { app_data, hub_data };
    let api_cache_key = data_map.get_hash();
    if let Some(bytes) = get_cache_manager!()
        .get(&GroupType::API, &api_cache_key.to_string(), None)
        .await
    {
        if let Ok(value) = bytes_to_json::<T>(&bytes) {
            return Some(value);
        }
    }
    let cache_keys = provider::get_cache_request_key(uuid, &func_type, &data_map);
    let mut cache_map = HashMap::new();
    if let Some(keys) = cache_keys {
        for key in keys {
            if let Some(value) = get_cache_manager!()
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
                let _ = get_cache_manager!()
                    .save(&GroupType::REPO_INSIDE, &key, value)
                    .await;
            }
        }
        if let Ok(data) = fout.result {
            if let Ok(value) = json_to_bytes(&data) {
                let _ = get_cache_manager!()
                    .save(&GroupType::API, &api_cache_key.to_string(), value)
                    .await;
            }
            return Some(data);
        }
    }
    None
}

pub async fn check_app_available<'a>(
    uuid: &str,
    app_data: &AppDataMap<'a>,
    hub_data: &HubDataMap<'a>,
) -> Option<bool> {
    process_data(
        uuid,
        app_data,
        hub_data,
        FunctionType::CheckAppAvailable,
        provider::check_app_available,
    )
    .await
}

pub async fn get_latest_release<'a>(
    uuid: &str,
    app_data: &AppDataMap<'a>,
    hub_data: &HubDataMap<'a>,
) -> Option<ReleaseData> {
    process_data(
        uuid,
        app_data,
        hub_data,
        FunctionType::GetLatestRelease,
        provider::get_latest_release,
    )
    .await
}

pub async fn get_releases<'a>(
    uuid: &str,
    app_data: &AppDataMap<'a>,
    hub_data: &HubDataMap<'a>,
) -> Option<Vec<ReleaseData>> {
    process_data(
        uuid,
        app_data,
        hub_data,
        FunctionType::GetReleases,
        provider::get_releases,
    )
    .await
}
