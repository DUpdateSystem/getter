use std::collections::BTreeMap;
use std::path::Path;

use crate::cache::init_cache_manager_with_expire;
use crate::core::config::world::{init_world_list, world_list};
use crate::error::Result;
use crate::websdk::repo::api;

use crate::utils::json::json_to_string;

#[allow(dead_code)]
pub async fn init(data_dir: &Path, cache_dir: &Path, global_expire_time: u64) -> Result<()> {
    // world list
    let world_list_path = data_dir.join(world_list::WORLD_CONFIG_LIST_NAME);
    init_world_list(&world_list_path).await?;
    // cache
    let local_cache_path = cache_dir.join("local_cache");
    init_cache_manager_with_expire(
        local_cache_path.as_path(),
        global_expire_time,
    ).await;
    Ok(())
}

#[allow(dead_code)]
pub async fn check_app_available<'a>(
    uuid: &str,
    app_data: &BTreeMap<&'a str, &'a str>,
    hub_data: &BTreeMap<&'a str, &'a str>,
) -> Option<bool> {
    api::check_app_available(uuid, app_data, hub_data).await
}

#[allow(dead_code)]
pub async fn get_latest_release<'a>(
    uuid: &str,
    app_data: &BTreeMap<&'a str, &'a str>,
    hub_data: &BTreeMap<&'a str, &'a str>,
) -> Option<String> {
    api::get_latest_release(uuid, app_data, hub_data)
        .await
        .map(|data| json_to_string(&data).unwrap())
}

#[allow(dead_code)]
pub async fn get_releases<'a>(
    uuid: &str,
    app_data: &BTreeMap<&'a str, &'a str>,
    hub_data: &BTreeMap<&'a str, &'a str>,
) -> Option<String> {
    api::get_releases(uuid, app_data, hub_data)
        .await
        .map(|data| json_to_string(&data).unwrap())
}
