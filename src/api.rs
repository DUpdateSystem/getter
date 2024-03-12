use std::collections::BTreeMap;
use std::path::Path;

use crate::cache::init_cache_manager_with_expire;
use crate::core::config::world::{init_world_list, world_list};
use crate::error::Result;
use crate::websdk::repo::api;

use crate::utils::json::json_to_string;

#[allow(dead_code)]
pub fn init(data_dir: &Path, cache_dir: &Path, global_expire_time: u64) -> Result<()> {
    // world list
    let world_list_path = data_dir.join(world_list::WORLD_CONFIG_LIST_NAME);
    init_world_list(
        world_list_path
            .to_str()
            .ok_or(crate::error::GetterError::new_nobase(
                "api",
                "init: world_list_path to_str failed",
            ))?,
    )?;
    // cache
    let local_cache_path = cache_dir.join("local_cache");
    init_cache_manager_with_expire(
        local_cache_path
            .to_str()
            .ok_or(crate::error::GetterError::new_nobase(
                "api",
                "init: local_cache_path to_str failed",
            ))?,
        global_expire_time,
    );
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

#[cfg(test)]
mod tests {
    use crate::get_cache_manager;

    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_github_check_app_available() {
        let temp_dir = tempdir().unwrap();
        let cache_dir = temp_dir.path().join("cache");
        let data_dir = temp_dir.path().join("data");
        init(&data_dir, &cache_dir, 100).unwrap();
        get_cache_manager!();
        let uuid = "fd9b2602-62c5-4d55-bd1e-0d6537714ca0";
        let id_map = BTreeMap::from([("repo", "UpgradeAll"), ("owner", "DUpdateSystem")]);
        let hub_data = BTreeMap::new();
        let result = check_app_available(uuid, &id_map, &hub_data).await;
        assert_eq!(result, Some(true));
    }
}
