use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};

use super::app_status::AppStatus;
use super::data_getter::DataGetter;
use super::updater::get_release_status;
use super::version_map::VersionMap;
use crate::database::get_db;
use crate::database::models::app::AppRecord;
use crate::error::Result;

/// Update event emitted when an app's status changes during a renew cycle.
#[derive(Debug, Clone)]
pub struct UpdateEvent {
    pub app_id: String,
    pub status: AppStatus,
}

pub type UpdateCallback = Arc<dyn Fn(UpdateEvent) + Send + Sync>;

/// Manages all tracked apps and their version data.
///
/// Mirrors Kotlin's `AppManager`.
pub struct AppManager {
    /// Saved apps from the database (keyed by record id)
    apps: Arc<RwLock<HashMap<String, AppRecord>>>,
    /// Virtual apps from Android installed packages (not persisted)
    virtual_apps: Arc<RwLock<Vec<AppRecord>>>,
    /// In-memory version maps keyed by app record id
    version_maps: Arc<RwLock<HashMap<String, VersionMap>>>,
    data_getter: Arc<DataGetter>,
    /// Optional callback invoked on each status change during renew
    update_callback: Option<UpdateCallback>,
}

impl AppManager {
    pub fn load() -> Result<Self> {
        let records = get_db().load_apps()?;
        let apps = records.into_iter().map(|a| (a.id.clone(), a)).collect();
        Ok(Self {
            apps: Arc::new(RwLock::new(apps)),
            virtual_apps: Arc::new(RwLock::new(vec![])),
            version_maps: Arc::new(RwLock::new(HashMap::new())),
            data_getter: Arc::new(DataGetter::new()),
            update_callback: None,
        })
    }

    pub fn set_update_callback(&mut self, cb: UpdateCallback) {
        self.update_callback = Some(cb);
    }

    /// Replace the virtual app list (installed Android packages).
    pub async fn set_virtual_apps(&self, apps: Vec<AppRecord>) {
        *self.virtual_apps.write().await = apps;
    }

    pub async fn get_all_apps(&self) -> Vec<AppRecord> {
        let mut result: Vec<AppRecord> = self.apps.read().await.values().cloned().collect();
        result.extend(self.virtual_apps.read().await.clone());
        result
    }

    pub async fn get_saved_apps(&self) -> Vec<AppRecord> {
        self.apps.read().await.values().cloned().collect()
    }

    pub async fn find_app_by_id(
        &self,
        app_id: &HashMap<String, Option<String>>,
    ) -> Option<AppRecord> {
        self.apps
            .read()
            .await
            .values()
            .find(|a| &a.app_id == app_id)
            .cloned()
    }

    pub async fn get_app(&self, record_id: &str) -> Option<AppRecord> {
        self.apps.read().await.get(record_id).cloned()
    }

    /// Persist (insert or update) an app record.
    pub async fn save_app(&self, mut record: AppRecord) -> Result<AppRecord> {
        if record.id.is_empty() {
            record.id = uuid::Uuid::new_v4().to_string();
        }
        get_db().upsert_app(&record)?;
        self.apps
            .write()
            .await
            .insert(record.id.clone(), record.clone());
        Ok(record)
    }

    /// Remove a saved app.
    pub async fn remove_app(&self, record_id: &str) -> Result<bool> {
        let deleted = get_db().delete_app(record_id)?;
        self.apps.write().await.remove(record_id);
        self.version_maps.write().await.remove(record_id);
        Ok(deleted)
    }

    /// Return the current AppStatus for an app.
    pub async fn get_app_status(&self, record_id: &str) -> AppStatus {
        let app = match self.apps.read().await.get(record_id).cloned() {
            Some(a) => a,
            None => return AppStatus::AppInactive,
        };
        let mut maps = self.version_maps.write().await;
        let vm = maps.entry(record_id.to_string()).or_insert_with(|| {
            VersionMap::new(
                app.invalid_version_number_field_regex.clone(),
                app.include_version_number_field_regex.clone(),
            )
        });
        get_release_status(vm, None, app.ignore_version_number.as_deref(), true)
    }

    /// Refresh version data for all saved apps.
    ///
    /// Uses a semaphore (max 10 concurrent hub requests) matching Kotlin's logic.
    pub async fn renew_all(
        &self,
        hubs: &[crate::database::models::hub::HubRecord],
        progress_cb: Option<&dyn Fn(usize, usize)>,
    ) {
        let apps = self.get_saved_apps().await;
        let total = apps.len();
        let semaphore = Arc::new(Semaphore::new(10));

        // Group apps by hub
        let mut hub_app_map: HashMap<String, Vec<AppRecord>> = HashMap::new();
        for app in &apps {
            let sorted_hubs = app.get_sorted_hub_uuids();
            let effective_hubs: Vec<&str> = if sorted_hubs.is_empty() {
                hubs.iter().map(|h| h.uuid.as_str()).collect()
            } else {
                sorted_hubs.iter().map(String::as_str).collect()
            };
            for hub_uuid in effective_hubs {
                hub_app_map
                    .entry(hub_uuid.to_string())
                    .or_default()
                    .push(app.clone());
            }
        }

        let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut handles = vec![];

        for hub in hubs {
            let hub_apps = match hub_app_map.get(&hub.uuid) {
                Some(v) => v.clone(),
                None => continue,
            };
            let hub = hub.clone();
            let getter = self.data_getter.clone();
            let version_maps = self.version_maps.clone();
            let _apps_map = self.apps.clone();
            let sem = semaphore.clone();
            let completed = completed.clone();
            let cb_arc: Option<Arc<dyn Fn(usize, usize) + Send + Sync>> = progress_cb.map(|_| {
                // Can't capture non-Send closure; skip progress in async context
                Arc::new(|_: usize, _: usize| {}) as Arc<dyn Fn(usize, usize) + Send + Sync>
            });

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();

                // Try batch latest-release first
                let app_ids: Vec<HashMap<String, Option<String>>> =
                    hub_apps.iter().map(|a| a.app_id.clone()).collect();
                let latest_results = getter.get_latest_releases(&hub, &app_ids).await;

                // Apps that got a result via batch: add single release
                let mut need_full: Vec<AppRecord> = vec![];
                for (app, (_, maybe_release)) in hub_apps.iter().zip(latest_results.iter()) {
                    if let Some(release) = maybe_release {
                        let mut maps = version_maps.write().await;
                        let vm = maps.entry(app.id.clone()).or_insert_with(|| {
                            VersionMap::new(
                                app.invalid_version_number_field_regex.clone(),
                                app.include_version_number_field_regex.clone(),
                            )
                        });
                        vm.add_single_release(&hub.uuid, release.clone());
                    } else {
                        need_full.push(app.clone());
                    }
                }

                // Apps that need full list
                for app in need_full {
                    if let Some(releases) = getter.get_release_list(&hub, &app.app_id).await {
                        let mut maps = version_maps.write().await;
                        let vm = maps.entry(app.id.clone()).or_insert_with(|| {
                            VersionMap::new(
                                app.invalid_version_number_field_regex.clone(),
                                app.include_version_number_field_regex.clone(),
                            )
                        });
                        vm.add_release_list(&hub.uuid, releases);
                    } else {
                        let mut maps = version_maps.write().await;
                        let vm = maps.entry(app.id.clone()).or_insert_with(|| {
                            VersionMap::new(
                                app.invalid_version_number_field_regex.clone(),
                                app.include_version_number_field_regex.clone(),
                            )
                        });
                        vm.set_error(&hub.uuid);
                    }
                    completed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if let Some(ref cb) = cb_arc {
                        cb(completed.load(std::sync::atomic::Ordering::SeqCst), total);
                    }
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            let _ = handle.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database;
    use std::collections::HashMap;

    #[test]
    fn test_app_record_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let db = database::Database::open(dir.path()).unwrap();

        let app = AppRecord::new(
            "MyApp".to_string(),
            HashMap::from([("owner".to_string(), Some("alice".to_string()))]),
        );
        db.upsert_app(&app).unwrap();
        let loaded = db.load_apps().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "MyApp");
    }

    #[tokio::test]
    async fn test_version_map_get_status_no_data() {
        let dir = tempfile::tempdir().unwrap();
        // Use Database directly to avoid global init conflict
        let db = database::Database::open(dir.path()).unwrap();
        let app = AppRecord::new("App".to_string(), HashMap::new());
        db.upsert_app(&app).unwrap();

        // Simulate with a fresh VersionMap
        let mut vm = VersionMap::new(None, None);
        let status = get_release_status(&mut vm, Some("1.0.0"), None, true);
        // Empty version map + is_saved → NetworkError (no hub_status entries)
        assert_eq!(status, AppStatus::NetworkError);
    }
}
