use std::collections::BTreeMap;

use crate::cache::init_cache_manager;
use crate::core::config::world::{init_world_list, world_list};
use crate::error::Result;
use crate::locale::all_dir;
use crate::websdk::repo::api;

use crate::utils::json::json_to_string;

#[allow(dead_code)]
pub fn init() -> Result<()> {
    let dir = all_dir().map_err(|e| {
        crate::error::GetterError::new("api", "init: get dir path failed", Box::new(e))
    })?;
    // world list
    let world_list_path = dir.data_dir.join(world_list::WORLD_CONFIG_LIST_NAME);
    init_world_list(
        world_list_path
            .to_str()
            .ok_or(crate::error::GetterError::new_nobase(
                "api",
                "init: world_list_path to_str failed",
            ))?,
    )?;
    // cache
    let local_cache_path = dir.cache_dir.join("local_cache");
    init_cache_manager(
        local_cache_path
            .to_str()
            .ok_or(crate::error::GetterError::new_nobase(
                "api",
                "init: local_cache_path to_str failed",
            ))?,
    );
    Ok(())
}

#[allow(dead_code)]
pub async fn check_app_available<'a>(
    uuid: &str,
    id_map: &BTreeMap<&'a str, &'a str>,
) -> Option<bool> {
    api::check_app_available(uuid, id_map).await
}

#[allow(dead_code)]
pub async fn get_latest_release<'a>(
    uuid: &str,
    id_map: &BTreeMap<&'a str, &'a str>,
) -> Option<String> {
    api::get_latest_release(uuid, id_map)
        .await
        .map(|data| json_to_string(&data).unwrap())
}

#[allow(dead_code)]
pub async fn get_releases<'a>(uuid: &str, id_map: &BTreeMap<&'a str, &'a str>) -> Option<String> {
    api::get_releases(uuid, id_map)
        .await
        .map(|data| json_to_string(&data).unwrap())
}
