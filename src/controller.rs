use crate::cache::GroupType::{API, REPO_INSIDE};
use crate::cache::{get_cache_manager, CACHE_MANAGER};
use crate::provider::base_provider::{CacheMap, FIn, FunctionType, IdMap};
use crate::provider::*;
use futures::future::join_all;
use std::collections::{BTreeMap, HashMap};

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
        if let Some(cache_key_list) = get_cache_request_key(uuid, function_type, id) {
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

pub async fn batch_check_app_available<'a>(
    uuid: &str,
    id_map_list: Vec<&'a IdMap<'a>>,
) -> Vec<Option<bool>> {
    let groups = group_requests(uuid, &id_map_list, &FunctionType::CheckAppAvailable);
    let mut groups_sorted = BTreeMap::new();
    for item in groups.iter() {
        let items = groups_sorted
            .entry(item.cache_key_list)
            .or_insert_with(Vec::new);
        items.push(item);
    }

    let mut result_map = HashMap::new();
    let _check_app_available = |fin: FIn, cache_map: &CacheMap<_, _>| async move {
        fin.cache_map = &cache_map.extend(&fin.cache_map);
        let result = check_app_available(uuid, &fin).await;
        return result;
    };
    for (cache_key_list, groups) in groups_sorted.iter() {
        for group in groups.iter() {
            let requests_id = group.requests_id;
            let cache_manager = get_cache_manager().lock().unwrap();
            let mut cache_map = CacheMap::new();
            for key in cache_key_list.iter() {
                if let Some(value) = cache_manager.get(REPO_INSIDE, key) {
                    cache_map.set(key, value);
                }
            }
            let fin_list = requests_id
                .iter()
                .map(|id| FIn {
                    id_map: id,
                    cache_map: &cache_map,
                })
                .collect::<Vec<FIn>>();
            // Get the first result for cache
            for fin in fin_list.iter() {
                if let Some(result) = check_app_available(uuid, fin).await {
                    if result.result.is_ok() {
                        if let Some(result_cache_map) = result.cached_map {
                            cache_map = result_cache_map;
                        }
                        break;
                    }
                }
            }
            let futures = fin_list
                .iter()
                .map(|fin| check_app_available(uuid, fin))
                .collect::<Vec<_>>();
            let results = join_all(futures).await;
            for result in results.iter() {
                let mut break_flag = false;
                if let Ok(result) = result {
                    if let Some(result) = result.result {
                        for id in requests_id.iter() {
                            result_map.insert(id.get("id").unwrap(), Some(result));
                        }
                        break_flag = true;
                    }
                }
                if let Some(result) = result {
                    if let Some(cached_map) = result.cached_map {
                        for (key, value) in cached_map.map.iter() {
                            cache_manager.save(REPO_INSIDE, key, value);
                        }
                        break_flag = true;
                    }
                }
                if break_flag {
                    break;
                }
            }
            for (i, result) in results.iter().enumerate() {
                let id = requests_id[i].get("id").unwrap();
                if let Some(result) = result {
                    result_map.insert(id, result);
                } else {
                    result_map.insert(id, None);
                }
            }
        }
    }
}
