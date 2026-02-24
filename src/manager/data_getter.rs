use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::database::models::hub::HubRecord;
use crate::websdk::repo::api;
use crate::websdk::repo::data::release::ReleaseData;

/// Result of a batch latest-release request.
/// Each entry is (app_id, Option<ReleaseData>) — None means the app wasn't found.
pub type LatestReleaseResults = Vec<(HashMap<String, Option<String>>, Option<ReleaseData>)>;

/// Fetches release data from hub providers.
///
/// Mirrors Kotlin's `DataGetter`.
pub struct DataGetter {
    /// Per-hub mutex to prevent duplicate concurrent requests.
    hub_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl DataGetter {
    pub fn new() -> Self {
        Self {
            hub_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn hub_lock(&self, hub_uuid: &str) -> Arc<Mutex<()>> {
        let mut locks = self.hub_locks.lock().await;
        locks
            .entry(hub_uuid.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Fetch the latest single release for each app from one hub.
    ///
    /// Returns a vec of `(app_id, Option<ReleaseData>)`.
    /// `None` means the hub didn't return data for that app.
    pub async fn get_latest_releases(
        &self,
        hub: &HubRecord,
        app_ids: &[HashMap<String, Option<String>>],
    ) -> LatestReleaseResults {
        let lock = self.hub_lock(&hub.uuid).await;
        let _guard = lock.lock().await;

        let mut results = Vec::with_capacity(app_ids.len());
        for app_id in app_ids {
            let app_data = build_app_data(app_id, &hub.hub_config.api_keywords);
            if app_data.is_empty() {
                results.push((app_id.clone(), None));
                continue;
            }
            let hub_data = build_hub_data(&hub.auth);
            let release = api::get_latest_release(&hub.uuid, &app_data, &hub_data).await;
            results.push((app_id.clone(), release));
        }
        results
    }

    /// Fetch the full release list for a single app from one hub.
    pub async fn get_release_list(
        &self,
        hub: &HubRecord,
        app_id: &HashMap<String, Option<String>>,
    ) -> Option<Vec<ReleaseData>> {
        let lock = self.hub_lock(&hub.uuid).await;
        let _guard = lock.lock().await;

        let app_data = build_app_data(app_id, &hub.hub_config.api_keywords);
        if app_data.is_empty() {
            return None;
        }
        let hub_data = build_hub_data(&hub.auth);
        api::get_releases(&hub.uuid, &app_data, &hub_data).await
    }
}

fn build_app_data<'a>(
    app_id: &'a HashMap<String, Option<String>>,
    api_keywords: &[String],
) -> BTreeMap<&'a str, &'a str> {
    app_id
        .iter()
        .filter(|(k, v)| api_keywords.contains(k) && v.is_some())
        .map(|(k, v)| (k.as_str(), v.as_deref().unwrap()))
        .collect()
}

fn build_hub_data(auth: &HashMap<String, String>) -> BTreeMap<&str, &str> {
    auth.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
}

impl Default for DataGetter {
    fn default() -> Self {
        Self::new()
    }
}
