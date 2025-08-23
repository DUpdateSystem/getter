use std::collections::BTreeMap;
use std::path::Path;

use crate::cache::init_cache_manager_with_expire;
use crate::core::app_manager::{get_app_manager, AppManager};
use crate::core::config::world::{init_world_list, world_list};
use crate::error::Result;
use crate::utils::json::json_to_string;
use crate::websdk::repo::data::release::ReleaseData;

/// Initialize the system with data and cache directories
pub async fn init(data_dir: &Path, cache_dir: &Path, global_expire_time: u64) -> Result<()> {
    // Initialize world list configuration
    let world_list_path = data_dir.join(world_list::WORLD_CONFIG_LIST_NAME);
    init_world_list(&world_list_path).await?;
    
    // Initialize cache manager
    let local_cache_path = cache_dir.join("local_cache");
    init_cache_manager_with_expire(local_cache_path.as_path(), global_expire_time).await;
    
    Ok(())
}

/// Library API: Check if app is available (returns bool)
pub async fn check_app_available<'a>(
    uuid: &str,
    app_data: &BTreeMap<&'a str, &'a str>,
    hub_data: &BTreeMap<&'a str, &'a str>,
) -> Option<bool> {
    get_app_manager()
        .check_app_available(uuid, app_data, hub_data)
        .await
        .ok()
}

/// Library API: Get latest release (returns ReleaseData)  
pub async fn get_latest_release<'a>(
    uuid: &str,
    app_data: &BTreeMap<&'a str, &'a str>,
    hub_data: &BTreeMap<&'a str, &'a str>,
) -> Option<ReleaseData> {
    get_app_manager()
        .get_latest_release(uuid, app_data, hub_data)
        .await
        .ok()
}

/// Library API: Get all releases (returns Vec<ReleaseData>)
pub async fn get_releases<'a>(
    uuid: &str,
    app_data: &BTreeMap<&'a str, &'a str>,
    hub_data: &BTreeMap<&'a str, &'a str>,
) -> Option<Vec<ReleaseData>> {
    get_app_manager()
        .get_releases(uuid, app_data, hub_data)
        .await
        .ok()
}

/// CLI/RPC API: Check if app is available (returns JSON string)
pub async fn check_app_available_json<'a>(
    uuid: &str,
    app_data: &BTreeMap<&'a str, &'a str>,
    hub_data: &BTreeMap<&'a str, &'a str>,
) -> Option<String> {
    check_app_available(uuid, app_data, hub_data)
        .await
        .map(|result| json_to_string(&result).unwrap_or_else(|_| "false".to_string()))
}

/// CLI/RPC API: Get latest release (returns JSON string)
pub async fn get_latest_release_json<'a>(
    uuid: &str,
    app_data: &BTreeMap<&'a str, &'a str>,
    hub_data: &BTreeMap<&'a str, &'a str>,
) -> Option<String> {
    get_latest_release(uuid, app_data, hub_data)
        .await
        .map(|data| json_to_string(&data).unwrap_or_else(|_| "null".to_string()))
}

/// CLI/RPC API: Get all releases (returns JSON string)
pub async fn get_releases_json<'a>(
    uuid: &str,
    app_data: &BTreeMap<&'a str, &'a str>,
    hub_data: &BTreeMap<&'a str, &'a str>,
) -> Option<String> {
    get_releases(uuid, app_data, hub_data)
        .await
        .map(|data| json_to_string(&data).unwrap_or_else(|_| "[]".to_string()))
}

/// Update app to specific version (placeholder for future implementation)
pub async fn update_app(app_id: &str, version: &str) -> Result<String> {
    match get_app_manager().update_app(app_id, version).await {
        Ok(result) => Ok(result),
        Err(err) => Err(crate::error::Error::Custom(err)),
    }
}

/// Add new app to management (placeholder for future implementation)
pub async fn add_app(app_config: &str) -> Result<String> {
    match get_app_manager().add_app(app_config).await {
        Ok(result) => Ok(result),
        Err(err) => Err(crate::error::Error::Custom(err)),
    }
}

/// Remove app from management (placeholder for future implementation)
pub async fn remove_app(app_id: &str) -> Result<bool> {
    match get_app_manager().remove_app(app_id).await {
        Ok(result) => Ok(result),
        Err(err) => Err(crate::error::Error::Custom(err)),
    }
}

/// Get reference to the global app manager for advanced usage
pub fn get_manager() -> &'static AppManager {
    get_app_manager()
}
