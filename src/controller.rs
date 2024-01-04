use crate::cache::convert::*;
use crate::get_cache_manager;
use crate::cache::manager::GroupType::{API, REPO_INSIDE};
use crate::provider;
use crate::provider::base_provider::{FIn, FOut, FunctionType, IdMap};
use bytes::Bytes;
use std::collections::HashMap;
use std::future::Future;
use crate::data::release::ReleaseData;

struct RequestGroup<'a> {
    pub cache_key_list: Vec<String>,
    pub requests_id: Vec<&'a IdMap<'a>>,
}

fn group_requests<'a>(
    uuid: &str,
    id_map_list: &Vec<&'a IdMap>,
    function_type: &FunctionType,
) -> Vec<RequestGroup<'a>> {
    let mut groups: HashMap<Vec<String>, Vec<&IdMap>> = HashMap::new();
    for id in id_map_list {
        if let Some(cache_key_list) = provider::get_cache_request_key(uuid, function_type, id) {
            if let Some(group) = groups.get_mut(&cache_key_list) {
                group.push(id);
            } else {
                groups.insert(cache_key_list, vec![id]);
            }
        }
    }
    groups
        .into_iter()
        .map(|(cache_key_list, requests_id)| RequestGroup {
            cache_key_list,
            requests_id,
        })
        .collect()
}

async fn get_fin<'a>(uuid: &str, id_map: &'a IdMap<'a>, function_type: &FunctionType) -> FIn<'a> {
    let cache_map = if let Some(cache_key_list) =
        provider::get_cache_request_key(uuid, function_type, id_map)
    {
        get_cache_manager!().get_cache_map(&REPO_INSIDE, &cache_key_list, None).await
    } else {
        None
    };
    FIn::new(id_map, cache_map)
}

async fn detach_result<R>(fout: Option<FOut<R>>) -> Option<R> {
    if let Some(fout) = fout {
        if let Some(cache_map) = fout.cached_map {
            for (key, value) in cache_map {
                get_cache_manager!().save(&REPO_INSIDE, &key, value).await;
            }
        }
        fout.result.ok()
    } else {
        None
    }
}

async fn cache_future_return<T>(
    f: impl Future<Output = Option<T>>,
    key: &str,
    expire_time: Option<u64>,
    encoder: fn(&T) -> Result<Bytes, serde_json::Error>,
    decoder: fn(&Bytes) -> Result<T, serde_json::Error>,
) -> Option<T> {
    if let Some(value) = get_cache_manager!().get(&API, key, expire_time).await {
        decoder(&value).ok()
    } else {
        let result = f.await;
        if let Some(result) = result {
            if let Ok(value) = encoder(&result) {
                get_cache_manager!().save(&API, key, value);
            }
            Some(result)
        } else {
            None
        }
    }
}

async fn _check_app_available<'a>(uuid: &str, id_map: &IdMap<'a>) -> Option<bool> {
    let fin = get_fin(uuid, id_map, &FunctionType::CheckAppAvailable).await;
    let fout = provider::check_app_available(uuid, &fin).await;
    detach_result(fout).await
}

pub async fn check_app_available<'a>(uuid: &str, id_map: &IdMap<'a>) -> Option<bool> {
    let key = format!("check_app_available_{}_{:?}", &uuid, id_map);
    let expire_time = Some(60 * 60 * 24);
    cache_future_return(
        _check_app_available(uuid, id_map),
        &key,
        expire_time,
        |data| Ok(bool_to_bytes(data)),
        |bytes| Ok(bytes_to_bool(bytes)),
    )
    .await
}

async fn _get_latest_release<'a>(uuid: &str, id_map: &IdMap<'a>) -> Option<ReleaseData> {
    let fin = get_fin(uuid, id_map, &FunctionType::GetLatestRelease).await;
    let fout = provider::get_latest_release(uuid, &fin).await;
    detach_result(fout).await
}

pub async fn get_latest_release<'a>(uuid: &str, id_map: &IdMap<'a>) -> Option<ReleaseData> {
    let key = format!("get_latest_release_{}_{:?}", &uuid, id_map);
    let expire_time = Some(60 * 60 * 24);
    cache_future_return(
        _get_latest_release(uuid, id_map),
        &key,
        expire_time,
        |data| json_to_bytes(&data),
        |bytes| bytes_to_json(bytes),
    )
    .await
}

async fn _get_releases<'a>(uuid: &str, id_map: &IdMap<'a>) -> Option<Vec<ReleaseData>> {
    let fin = get_fin(uuid, id_map, &FunctionType::GetReleases).await;
    let fout = provider::get_releases(uuid, &fin).await;
    detach_result(fout).await
}

pub async fn get_releases<'a>(uuid: &str, id_map: &IdMap<'a>) -> Option<Vec<ReleaseData>> {
    let key = format!("get_releases_{}_{:?}", &uuid, id_map);
    let expire_time = Some(60 * 60 * 24);
    cache_future_return(
        _get_releases(uuid, id_map),
        &key,
        expire_time,
        |data| json_to_bytes(&data),
        |bytes| bytes_to_json(bytes),
    )
    .await
}
