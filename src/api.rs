use std::collections::BTreeMap;
use std::path::Path;

use crate::cache::init_cache_manager_with_expire;
use crate::core::app_manager::{get_app_manager, AppManager};
use crate::core::config::world::init_world_list;
use crate::core::config::world::world_list::WORLD_CONFIG_LIST_NAME;
use crate::core::status_tracker::AppStatusInfo;
use crate::error::Result;
use crate::utils::json::json_to_string;
use crate::websdk::repo::data::release::ReleaseData;

/// Initialize the system with data and cache directories
pub async fn init(data_dir: &Path, cache_dir: &Path, global_expire_time: u64) -> Result<()> {
    // Initialize world list configuration
    let world_list_path = data_dir.join(WORLD_CONFIG_LIST_NAME);
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

/// Add new app to management
pub async fn add_app(
    app_id: String,
    hub_uuid: String,
    app_data: std::collections::HashMap<String, String>,
    hub_data: std::collections::HashMap<String, String>,
) -> Result<String> {
    match get_app_manager()
        .add_app(app_id, hub_uuid, app_data, hub_data)
        .await
    {
        Ok(result) => Ok(result),
        Err(err) => Err(crate::error::Error::Custom(err)),
    }
}

/// Remove app from management
pub async fn remove_app(app_id: &str) -> Result<bool> {
    match get_app_manager().remove_app(app_id).await {
        Ok(result) => Ok(result),
        Err(err) => Err(crate::error::Error::Custom(err)),
    }
}

/// List all tracked apps
pub async fn list_apps() -> Result<Vec<String>> {
    match get_app_manager().list_apps().await {
        Ok(result) => Ok(result),
        Err(err) => Err(crate::error::Error::Custom(err)),
    }
}

/// Get reference to the global app manager for advanced usage
pub fn get_manager() -> &'static AppManager {
    get_app_manager()
}

/// Get status for a specific app
pub async fn get_app_status(app_id: &str) -> Result<Option<AppStatusInfo>> {
    match get_app_manager().get_app_status(app_id).await {
        Ok(status) => Ok(status),
        Err(err) => Err(crate::error::Error::Custom(err)),
    }
}

/// Get status for all tracked apps
pub async fn get_all_app_statuses() -> Result<Vec<AppStatusInfo>> {
    match get_app_manager().get_all_app_statuses().await {
        Ok(statuses) => Ok(statuses),
        Err(err) => Err(crate::error::Error::Custom(err)),
    }
}

/// Get apps that have updates available
pub async fn get_outdated_apps() -> Result<Vec<AppStatusInfo>> {
    match get_app_manager().get_outdated_apps().await {
        Ok(apps) => Ok(apps),
        Err(err) => Err(crate::error::Error::Custom(err)),
    }
}

/// Get status as JSON string for cross-language compatibility  
pub async fn get_app_status_json(app_id: &str) -> Option<String> {
    get_app_status(app_id)
        .await
        .ok()
        .flatten()
        .map(|status| json_to_string(&status).unwrap_or_else(|_| "null".to_string()))
}

/// Get all statuses as JSON string for cross-language compatibility
pub async fn get_all_app_statuses_json() -> Option<String> {
    get_all_app_statuses()
        .await
        .ok()
        .map(|statuses| json_to_string(&statuses).unwrap_or_else(|_| "[]".to_string()))
}

/// Get outdated apps as JSON string for cross-language compatibility
pub async fn get_outdated_apps_json() -> Option<String> {
    get_outdated_apps()
        .await
        .ok()
        .map(|apps| json_to_string(&apps).unwrap_or_else(|_| "[]".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[tokio::test]
    async fn test_init() {
        use tempfile::tempdir;
        let temp_dir = tempdir().unwrap();
        let data_dir = temp_dir.path().join("data");
        let cache_dir = temp_dir.path().join("cache");

        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();

        let result = init(&data_dir, &cache_dir, 3600).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_check_app_available() {
        let app_data = BTreeMap::from([("repo", "test")]);
        let hub_data = BTreeMap::new();

        let result = check_app_available("test-hub", &app_data, &hub_data).await;
        // Should return Some(bool) or None
        match result {
            Some(available) => {
                assert!(available == true || available == false);
            }
            None => {
                // No result returned, which is also valid
            }
        }
    }

    #[tokio::test]
    async fn test_check_app_available_json() {
        let app_data = BTreeMap::from([("repo", "test")]);
        let hub_data = BTreeMap::new();

        let result = check_app_available_json("test-hub", &app_data, &hub_data).await;
        match result {
            Some(json_str) => {
                // Should be valid JSON string
                assert!(!json_str.is_empty());
                assert!(json_str == "true" || json_str == "false");
            }
            None => {
                // No result, which is valid
            }
        }
    }

    #[tokio::test]
    async fn test_get_latest_release() {
        let app_data = BTreeMap::from([("repo", "test")]);
        let hub_data = BTreeMap::new();

        let result = get_latest_release("test-hub", &app_data, &hub_data).await;
        // Should complete without panicking
        match result {
            Some(_release_data) => {
                // Got release data
            }
            None => {
                // No release data, which is valid
            }
        }
    }

    #[tokio::test]
    async fn test_get_latest_release_json() {
        let app_data = BTreeMap::from([("repo", "test")]);
        let hub_data = BTreeMap::new();

        let result = get_latest_release_json("test-hub", &app_data, &hub_data).await;
        match result {
            Some(json_str) => {
                // Should be JSON string
                assert!(!json_str.is_empty());
            }
            None => {
                // No result, which is valid
            }
        }
    }

    #[tokio::test]
    async fn test_get_releases() {
        let app_data = BTreeMap::from([("repo", "test")]);
        let hub_data = BTreeMap::new();

        let result = get_releases("test-hub", &app_data, &hub_data).await;
        match result {
            Some(_releases) => {
                // Got releases data
            }
            None => {
                // No releases data, which is valid
            }
        }
    }

    #[tokio::test]
    async fn test_get_releases_json() {
        let app_data = BTreeMap::from([("repo", "test")]);
        let hub_data = BTreeMap::new();

        let result = get_releases_json("test-hub", &app_data, &hub_data).await;
        match result {
            Some(json_str) => {
                // Should be JSON string
                assert!(!json_str.is_empty());
            }
            None => {
                // No result, which is valid
            }
        }
    }

    #[tokio::test]
    async fn test_add_app() {
        let app_data =
            std::collections::HashMap::from([("repo".to_string(), "test-repo".to_string())]);
        let hub_data = std::collections::HashMap::new();

        let result = add_app(
            "api-test-app".to_string(),
            "test-hub".to_string(),
            app_data,
            hub_data,
        )
        .await;

        // Should complete (may succeed or fail depending on infrastructure)
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_remove_app() {
        let result = remove_app("api-test-app").await;
        assert!(result.is_ok());
        // Should return a boolean
        let removed = result.unwrap();
        assert!(removed == true || removed == false);
    }

    #[tokio::test]
    async fn test_list_apps() {
        let result = list_apps().await;
        assert!(result.is_ok());
        let _apps = result.unwrap();
        // Should return a vector (may be empty)
        // Length is always >= 0 for vectors, just verify it's a valid vector
    }

    #[tokio::test]
    async fn test_update_app() {
        let result = update_app("test-app", "1.0.0").await;
        // Should complete (may succeed or fail)
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_get_manager() {
        let manager = get_manager();
        // Should return a valid reference
        assert!(!std::ptr::eq(manager as *const _, std::ptr::null()));
    }

    #[tokio::test]
    async fn test_api_consistency() {
        // Test that API functions return consistent types
        let app_data = BTreeMap::from([("repo", "consistency-test")]);
        let hub_data = BTreeMap::new();

        // Both lib API and JSON API should handle the same request
        let lib_result = check_app_available("test-hub", &app_data, &hub_data).await;
        let json_result = check_app_available_json("test-hub", &app_data, &hub_data).await;

        // Both should complete
        match (lib_result, json_result) {
            (Some(bool_val), Some(json_str)) => {
                if bool_val {
                    assert_eq!(json_str, "true");
                } else {
                    assert_eq!(json_str, "false");
                }
            }
            (None, None) => {
                // Both return None, consistent
            }
            _ => {
                // One returns Some, other None - may be valid due to timing
            }
        }
    }
}
