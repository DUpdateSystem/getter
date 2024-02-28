use std::collections::BTreeMap;

use super::provider;
use super::provider::base_provider::FIn;
use super::data::release::ReleaseData;

#[allow(dead_code)]
pub async fn check_app_available<'a>(
    uuid: &str,
    id_map: &BTreeMap<&'a str, &'a str>,
) -> Option<bool> {
    let fin = FIn::new(id_map, None);
    if let Some(fout) = provider::check_app_available(uuid, &fin).await {
        if let Ok(data) = fout.result {
            return Some(data);
        }
    }
    None
}

#[allow(dead_code)]
pub async fn get_latest_release<'a>(
    uuid: &str,
    id_map: &BTreeMap<&'a str, &'a str>,
) -> Option<ReleaseData> {
    if let Some(fout) = provider::get_latest_release(uuid, &FIn::new(id_map, None)).await {
        if let Ok(data) = fout.result {
            return Some(data);
        }
    }
    None
}

#[allow(dead_code)]
pub async fn get_releases<'a>(uuid: &str, id_map: &BTreeMap<&'a str, &'a str>) -> Option<Vec<ReleaseData>> {
    if let Some(fout) = provider::get_releases(uuid, &FIn::new(id_map, None)).await {
        if let Ok(data) = fout.result {
            return Some(data);
        }
    }
    None
}
