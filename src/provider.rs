pub mod base_provider;
pub mod github;

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;

use self::base_provider::*;
use self::github::GithubProvider;
use crate::data::release::ReleaseData;

static PROVIDER_MAP: Lazy<Arc<HashMap<&'static str, Arc<dyn BaseProvider + Send + Sync>>>> =
    Lazy::new(|| {
        let mut m = HashMap::new();
        m.insert(
            "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
            Arc::new(GithubProvider::new(HashMap::new())) as Arc<dyn BaseProvider + Send + Sync>,
        );
        Arc::new(m)
    });

pub fn get_provider_list() -> Vec<String> {
    PROVIDER_MAP
        .keys()
        .map(|k| k.to_string())
        .collect::<Vec<String>>()
}

fn get_provider(uuid: &str) -> Option<&Arc<dyn BaseProvider + Send + Sync>> {
    PROVIDER_MAP.get(uuid)
}

pub async fn check_app_available(
    uuid: &str,
    id_map: &HashMap<&str, &str>,
) -> Option<bool> {
    if let Some(provider) = get_provider(uuid) {
        provider.check_app_available(&FIn::new(id_map)).await.data
    } else {
        None
    }
}

pub async fn get_latest_release(
    uuid: &str,
    id_map: &HashMap<&str, &str>,
) -> Option<ReleaseData> {
    if let Some(provider) = get_provider(uuid) {
        provider.get_latest_release(&FIn::new(id_map)).await.data
    } else {
        None
    }
}

pub async fn get_releases(
    uuid: &str,
    id_map: &HashMap<&str, &str>,
) -> Option<Vec<ReleaseData>> {
    if let Some(provider) = get_provider(uuid) {
        provider.get_releases(&FIn::new(id_map)).await.data
    } else {
        None
    }
}
