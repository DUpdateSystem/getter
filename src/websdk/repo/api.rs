use super::data::release::ReleaseData;
use super::provider;
use super::provider::base_provider::{AppDataMap, FIn, HubDataMap};

#[allow(dead_code)]
pub async fn check_app_available<'a>(
    uuid: &str,
    app_data: &AppDataMap<'a>,
    hub_data: &HubDataMap<'a>,
) -> Option<bool> {
    let fin = FIn::new(app_data, hub_data, None);
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
    app_data: &AppDataMap<'a>,
    hub_data: &HubDataMap<'a>,
) -> Option<ReleaseData> {
    if let Some(fout) =
        provider::get_latest_release(uuid, &FIn::new(app_data, hub_data, None)).await
    {
        if let Ok(data) = fout.result {
            return Some(data);
        }
    }
    None
}

#[allow(dead_code)]
pub async fn get_releases<'a>(
    uuid: &str,
    app_data: &AppDataMap<'a>,
    hub_data: &HubDataMap<'a>,
) -> Option<Vec<ReleaseData>> {
    if let Some(fout) = provider::get_releases(uuid, &FIn::new(app_data, hub_data, None)).await {
        if let Ok(data) = fout.result {
            return Some(data);
        }
    }
    None
}
