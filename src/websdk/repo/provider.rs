pub mod base_provider;
pub mod fdroid;
pub mod github;
pub mod gitlab;
pub mod lsposed_repo;
pub mod outside_rpc;

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use self::base_provider::{BaseProvider, DataMap, FIn, FOut, FunctionType};
use self::fdroid::FDroidProvider;
use self::github::GitHubProvider;
use self::gitlab::GitLabProvider;
use self::lsposed_repo::LsposedRepoProvider;
use super::data::release::ReleaseData;

type ProviderMap = HashMap<String, Arc<dyn BaseProvider + Send + Sync>>;

static PROVIDER_MAP: Lazy<Arc<RwLock<ProviderMap>>> = Lazy::new(|| {
    let providers: Vec<Arc<dyn BaseProvider + Send + Sync>> = vec![
        Arc::new(GitHubProvider::new()),
        Arc::new(FDroidProvider::new()),
        Arc::new(GitLabProvider::new()),
        Arc::new(LsposedRepoProvider::new()),
    ];

    let mut map = HashMap::new();

    for provider in providers {
        let uuid = provider.get_uuid().to_string();
        let friendly_name = provider.get_friendly_name();

        // Register by UUID
        map.insert(uuid, provider.clone());

        // Register by friendly name (if not empty)
        if !friendly_name.is_empty() {
            map.insert(friendly_name.to_string(), provider.clone());
        }
    }

    Arc::new(RwLock::new(map))
});

pub fn get_hub_uuid(hub_id: &str) -> String {
    // Try to get provider and extract UUID
    if let Some(provider) = get_provider(hub_id) {
        provider.get_uuid().to_string()
    } else {
        // If no provider found, assume it's already a UUID
        hub_id.to_string()
    }
}

fn get_provider(uuid: &str) -> Option<Arc<dyn BaseProvider + Send + Sync>> {
    let map = PROVIDER_MAP.read().unwrap();
    map.get(uuid).cloned()
}

pub fn add_provider(provider: impl BaseProvider + Send + Sync + 'static) {
    let provider_arc = Arc::new(provider) as Arc<dyn BaseProvider + Send + Sync>;
    let uuid = provider_arc.get_uuid().to_string();
    let friendly_name = provider_arc.get_friendly_name();

    let mut map = PROVIDER_MAP.write().unwrap();

    // Register by UUID
    map.insert(uuid, provider_arc.clone());

    // Register by friendly name (if not empty)
    if !friendly_name.is_empty() {
        map.insert(friendly_name.to_string(), provider_arc.clone());
    }
}

pub fn get_cache_request_key(
    uuid: &str,
    function_type: &FunctionType,
    data_map: &DataMap,
) -> Option<Vec<String>> {
    get_provider(uuid).map(|provider| provider.get_cache_request_key(function_type, data_map))
}

pub async fn check_app_available(uuid: &str, fin: &FIn<'_>) -> Option<FOut<bool>> {
    if let Some(provider) = get_provider(uuid) {
        Some(provider.check_app_available(fin).await)
    } else {
        None
    }
}

pub async fn get_latest_release(uuid: &str, fin: &FIn<'_>) -> Option<FOut<ReleaseData>> {
    if let Some(provider) = get_provider(uuid) {
        Some(provider.get_latest_release(fin).await)
    } else {
        None
    }
}

pub async fn get_releases(uuid: &str, fin: &FIn<'_>) -> Option<FOut<Vec<ReleaseData>>> {
    if let Some(provider) = get_provider(uuid) {
        Some(provider.get_releases(fin).await)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hub_uuid_mapping() {
        // Test known mappings
        assert_eq!(
            get_hub_uuid("github"),
            "fd9b2602-62c5-4d55-bd1e-0d6537714ca0"
        );
        assert_eq!(
            get_hub_uuid("fdroid"),
            "6a6d590b-1809-41bf-8ce3-7e3f6c8da945"
        );
        assert_eq!(
            get_hub_uuid("gitlab"),
            "a84e2fbe-1478-4db5-80ae-75d00454c7eb"
        );
        assert_eq!(
            get_hub_uuid("lsposed"),
            "401e6259-2eab-46f0-8e8a-d2bfafedf5bf"
        );

        // Test unknown names return as-is
        assert_eq!(get_hub_uuid("unknown"), "unknown");
        assert_eq!(
            get_hub_uuid("fd9b2602-62c5-4d55-bd1e-0d6537714ca0"),
            "fd9b2602-62c5-4d55-bd1e-0d6537714ca0"
        );
    }

    #[test]
    fn test_provider_exists() {
        // Test that providers are registered for each UUID
        assert!(get_provider("fd9b2602-62c5-4d55-bd1e-0d6537714ca0").is_some()); // GitHub
        assert!(get_provider("6a6d590b-1809-41bf-8ce3-7e3f6c8da945").is_some()); // FDroid
        assert!(get_provider("a84e2fbe-1478-4db5-80ae-75d00454c7eb").is_some()); // GitLab
        assert!(get_provider("401e6259-2eab-46f0-8e8a-d2bfafedf5bf").is_some()); // Lsposed

        // Test that providers are also accessible by friendly names
        assert!(get_provider("github").is_some()); // GitHub
        assert!(get_provider("fdroid").is_some()); // FDroid
        assert!(get_provider("gitlab").is_some()); // GitLab
        assert!(get_provider("lsposed").is_some()); // Lsposed

        // Test unknown identifier returns None
        assert!(get_provider("unknown-uuid").is_none());
        assert!(get_provider("unknown-name").is_none());
    }
}
