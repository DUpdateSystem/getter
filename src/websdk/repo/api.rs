use std::collections::BTreeMap;

use crate::utils::json::json_to_string;
use crate::cache::init_cache_manager;

use super::controller;

#[warn(dead_code)]
pub async fn init_config(local_cache_path: &str) {
    init_cache_manager(local_cache_path);
}

#[warn(dead_code)]
pub async fn check_app_available<'a>(uuid: &str, id_map: &BTreeMap<&'a str, &'a str>) -> Option<bool> {
    return controller::check_app_available(uuid, id_map).await;
}

#[warn(dead_code)]
pub async fn get_latest_release<'a>(uuid: &str, id_map: &BTreeMap<&'a str, &'a str>) -> Option<String> {
    if let Some(data) = controller::get_latest_release(uuid, id_map).await {
        if let Ok(s) = json_to_string(&data) {
            return Some(s);
        }
    }
    None
}

#[warn(dead_code)]
pub async fn get_releases<'a>(uuid: &str, id_map: &BTreeMap<&'a str, &'a str>) -> Option<String> {
    if let Some(data) = controller::get_releases(uuid, id_map).await {
        if let Ok(s) = json_to_string(&data) {
            return Some(s);
        }
    }
    None
}
