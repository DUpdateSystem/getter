pub mod base_provider;
pub mod github;

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;

use self::base_provider::BaseProvider;
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

fn get_provider(uuid: &str) -> Option<&Arc<dyn BaseProvider + Send + Sync>> {
    PROVIDER_MAP.get(uuid)
}

pub async fn check_app_available(
    uuid: &str,
    id_map: &HashMap<String, String>,
) -> Option<bool> {
    if let Some(provider) = get_provider(uuid) {
        provider.check_app_available(id_map).await
    } else {
        None
    }
}

pub async fn get_latest_release(
    uuid: &str,
    id_map: &HashMap<String, String>,
) -> Option<ReleaseData> {
    if let Some(provider) = get_provider(uuid) {
        provider.get_latest_release(id_map).await
    } else {
        None
    }
}

pub async fn get_releases(
    uuid: &str,
    id_map: &HashMap<String, String>,
) -> Option<Vec<ReleaseData>> {
    if let Some(provider) = get_provider(uuid) {
        provider.get_releases(id_map).await
    } else {
        None
    }
}
