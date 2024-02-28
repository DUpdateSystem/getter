pub mod base_provider;
pub mod github;

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;

use self::base_provider::{BaseProvider, FunctionType, IdMap, FIn, FOut};
use self::github::GithubProvider;
use super::data::release::ReleaseData;

static PROVIDER_MAP: Lazy<Arc<HashMap<&'static str, Arc<dyn BaseProvider + Send + Sync>>>> =
    Lazy::new(|| {
        let mut m = HashMap::new();
        m.insert(
            "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
            Arc::new(GithubProvider::new(HashMap::new())) as Arc<dyn BaseProvider + Send + Sync>,
        );
        Arc::new(m)
    });

fn get_provider(uuid: &str) -> Option<&Arc<dyn BaseProvider + Send + Sync>> {
    PROVIDER_MAP.get(uuid)
}

pub fn get_cache_request_key(
    uuid: &str,
    function_type: &FunctionType,
    id_map: &IdMap,
) -> Option<Vec<String>> {
    if let Some(provider) = get_provider(uuid) {
        Some(provider.get_cache_request_key(function_type, id_map))
    } else {
        None
    }
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
