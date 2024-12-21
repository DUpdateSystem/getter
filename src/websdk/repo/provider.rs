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

type ProviderMap = HashMap<&'static str, Arc<dyn BaseProvider + Send + Sync>>;

static PROVIDER_MAP: Lazy<Arc<RwLock<ProviderMap>>> =
    Lazy::new(|| {
        Arc::new(RwLock::new(HashMap::from([
            (
                "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
                Arc::new(GitHubProvider::new()) as Arc<dyn BaseProvider + Send + Sync>,
            ),
            (
                "6a6d590b-1809-41bf-8ce3-7e3f6c8da945",
                Arc::new(FDroidProvider::new()) as Arc<dyn BaseProvider + Send + Sync>,
            ),
            (
                "a84e2fbe-1478-4db5-80ae-75d00454c7eb",
                Arc::new(GitLabProvider::new()) as Arc<dyn BaseProvider + Send + Sync>,
            ),
            (
                "401e6259-2eab-46f0-8e8a-d2bfafedf5bf",
                Arc::new(LsposedRepoProvider::new()) as Arc<dyn BaseProvider + Send + Sync>,
            ),
        ])))
    });

fn get_provider(uuid: &str) -> Option<Arc<dyn BaseProvider + Send + Sync>> {
    let map = PROVIDER_MAP.read().unwrap();
    map.get(uuid).cloned()
}

pub fn add_provider(uuid: &str, provider: impl BaseProvider + Send + Sync + 'static) {
    let mut map = PROVIDER_MAP.write().unwrap();
    let uuid: &'static str = Box::leak(Box::new(uuid.to_string()));
    map.insert(
        uuid,
        Arc::new(provider) as Arc<dyn BaseProvider + Send + Sync>,
    );
}

pub fn get_cache_request_key(
    uuid: &str,
    function_type: &FunctionType,
    data_map: &DataMap,
) -> Option<Vec<String>> {
    get_provider(uuid).map(|provider| provider.get_cache_request_key(function_type, data_map))
}

pub async fn check_app_available<'a>(uuid: &str, fin: &FIn<'a>) -> Option<FOut<bool>> {
    if let Some(provider) = get_provider(uuid) {
        Some(provider.check_app_available(fin).await)
    } else {
        None
    }
}

pub async fn get_latest_release<'a>(uuid: &str, fin: &FIn<'a>) -> Option<FOut<ReleaseData>> {
    if let Some(provider) = get_provider(uuid) {
        Some(provider.get_latest_release(fin).await)
    } else {
        None
    }
}

pub async fn get_releases<'a>(uuid: &str, fin: &FIn<'a>) -> Option<FOut<Vec<ReleaseData>>> {
    if let Some(provider) = get_provider(uuid) {
        Some(provider.get_releases(fin).await)
    } else {
        None
    }
}
