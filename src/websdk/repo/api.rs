use async_fn_traits::AsyncFnOnce2;

use super::data::release::ReleaseData;
use super::provider;
use super::provider::base_provider::{AppDataMap, DataMap, FIn, FOut, FunctionType, HubDataMap};
use crate::cache::manager::GroupType;
use crate::get_cache_manager;
use std::collections::HashMap;

async fn process_data<T, F>(
    uuid: &str,
    app_data: &AppDataMap<'_>,
    hub_data: &HubDataMap<'_>,
    func_type: FunctionType,
    provider_func: F,
) -> Option<T>
where
    T: Send,
    F: for<'a> AsyncFnOnce2<&'a str, &'a FIn<'a>, Output = Option<FOut<T>>>,
{
    let data_map = DataMap { app_data, hub_data };
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
