use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};

use super::android_api;
use super::app_status::AppStatus;
use super::data_getter::DataGetter;
use super::notification::{notify_if_registered, ManagerEvent};
use super::updater::get_release_status;
use super::version_map::VersionMap;
use crate::database::get_db;
use crate::database::models::app::AppRecord;
use crate::error::Result;

pub type UpdateCallback = Arc<dyn Fn(usize, usize) + Send + Sync>;

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
        })
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
    ///
    /// Queries Kotlin for the locally-installed version via `AndroidApi.get_local_version()`
    /// if a callback URL has been registered; otherwise local_version is `None`.
    pub async fn get_app_status(&self, record_id: &str) -> AppStatus {
        let app = match self.apps.read().await.get(record_id).cloned() {
            Some(a) => a,
            None => return AppStatus::AppInactive,
        };

        // Query local version from Android via registered callback
        let local_version: Option<String> = match android_api::get_android_api() {
            Some(api) => api.get_local_version(&app.app_id).await,
            None => None,
        };

        let mut maps = self.version_maps.write().await;
        let vm = maps.entry(record_id.to_string()).or_insert_with(|| {
            VersionMap::new(
                app.invalid_version_number_field_regex.clone(),
                app.include_version_number_field_regex.clone(),
            )
        });
        get_release_status(
            vm,
            local_version.as_deref(),
            app.ignore_version_number.as_deref(),
            true,
        )
    }

    /// Refresh version data for all saved apps.
    ///
    /// Uses a semaphore (max 10 concurrent hub requests) matching Kotlin's logic.
    /// Apps that have version-filter regexes (`need_complete_version`) skip the batch
    /// API entirely and go straight to the full release-list path, mirroring Kotlin's
    /// `simpleMap` / `completeMap` split in `AppManager.renewAppList()`.
    /// Fires `RenewProgress` notifications to Kotlin UI as each app completes.
    pub async fn renew_all(
        &self,
        hubs: &[crate::database::models::hub::HubRecord],
        progress_cb: Option<UpdateCallback>,
    ) {
        let apps = self.get_saved_apps().await;
        let total = apps.len();
        let semaphore = Arc::new(Semaphore::new(10));

        // Group apps by hub, splitting into simple (batch-eligible) vs complete-only.
        let mut hub_simple_map: HashMap<String, Vec<AppRecord>> = HashMap::new();
        let mut hub_complete_map: HashMap<String, Vec<AppRecord>> = HashMap::new();
        for app in &apps {
            let sorted_hubs = app.get_sorted_hub_uuids();
            let effective_hubs: Vec<&str> = if sorted_hubs.is_empty() {
                hubs.iter().map(|h| h.uuid.as_str()).collect()
            } else {
                sorted_hubs.iter().map(String::as_str).collect()
            };
            let dest = if need_complete_version(app) {
                &mut hub_complete_map
            } else {
                &mut hub_simple_map
            };
            for hub_uuid in effective_hubs {
                dest.entry(hub_uuid.to_string())
                    .or_default()
                    .push(app.clone());
            }
        }

        let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut handles = vec![];

        for hub in hubs {
            let simple_apps = hub_simple_map.get(&hub.uuid).cloned().unwrap_or_default();
            let complete_apps = hub_complete_map.get(&hub.uuid).cloned().unwrap_or_default();
            if simple_apps.is_empty() && complete_apps.is_empty() {
                continue;
            }

            let hub = hub.clone();
            let getter = self.data_getter.clone();
            let version_maps = self.version_maps.clone();
            let sem = semaphore.clone();
            let completed = completed.clone();
            let cb = progress_cb.clone();

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();

                // --- Batch path (simple apps only) ---
                let mut need_full: Vec<AppRecord> = complete_apps; // complete-only apps go straight here
                if !simple_apps.is_empty() {
                    let app_ids: Vec<HashMap<String, Option<String>>> =
                        simple_apps.iter().map(|a| a.app_id.clone()).collect();
                    let latest_results = getter.get_latest_releases(&hub, &app_ids).await;

                    for (app, (_, maybe_release)) in simple_apps.iter().zip(latest_results.iter()) {
                        if let Some(release) = maybe_release {
                            let mut maps = version_maps.write().await;
                            let vm = maps.entry(app.id.clone()).or_insert_with(|| {
                                VersionMap::new(
                                    app.invalid_version_number_field_regex.clone(),
                                    app.include_version_number_field_regex.clone(),
                                )
                            });
                            vm.add_single_release(&hub.uuid, release.clone());
                            let done =
                                completed.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                            if let Some(ref f) = cb {
                                f(done, total);
                            }
                            notify_if_registered(ManagerEvent::RenewProgress { done, total }).await;
                        } else {
                            // Batch returned nothing for this app — escalate to full list.
                            need_full.push(app.clone());
                        }
                    }
                }

                // --- Full release-list path ---
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
                    let done = completed.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    if let Some(ref f) = cb {
                        f(done, total);
                    }
                    notify_if_registered(ManagerEvent::RenewProgress { done, total }).await;
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            let _ = handle.await;
        }
    }

    /// Return record IDs of saved apps that have no valid hub configured.
    ///
    /// An app is considered "invalid" when its `enable_hub_list` is non-empty but
    /// none of the listed hub UUIDs exist in `known_hub_uuids`.  Apps with an empty
    /// hub list (meaning "use all hubs") are never reported as invalid.
    pub async fn check_invalid_applications(&self, known_hub_uuids: &[String]) -> Vec<String> {
        let apps = self.apps.read().await;
        apps.values()
            .filter_map(|app| {
                let hub_uuids = app.get_sorted_hub_uuids();
                // Empty list means "match any hub" — not invalid.
                if hub_uuids.is_empty() {
                    return None;
                }
                let has_valid = hub_uuids.iter().any(|uuid| known_hub_uuids.contains(uuid));
                if has_valid {
                    None
                } else {
                    Some(app.id.clone())
                }
            })
            .collect()
    }
}

/// Returns `true` if the app requires the full release list rather than the single
/// latest-release batch API.
///
/// Mirrors Kotlin's `App.needCompleteVersion`:
/// ```kotlin
/// val needCompleteVersion: Boolean
///     get() = db.includeVersionNumberFieldRegexString != null
///          || db.invalidVersionNumberFieldRegexString != null
/// ```
fn need_complete_version(app: &AppRecord) -> bool {
    app.include_version_number_field_regex.is_some()
        || app.invalid_version_number_field_regex.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database;

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
        let mut vm = VersionMap::new(None, None);
        let status = get_release_status(&mut vm, Some("1.0.0"), None, true);
        // Empty version map + is_saved → NetworkError (no hub_status entries)
        assert_eq!(status, AppStatus::NetworkError);
    }

    // -------------------------------------------------------------------------
    // Phase 7A: need_complete_version
    // -------------------------------------------------------------------------

    #[test]
    fn test_need_complete_version_false_when_no_regex() {
        let app = AppRecord::new("App".to_string(), HashMap::new());
        assert!(!need_complete_version(&app));
    }

    #[test]
    fn test_need_complete_version_true_when_invalid_regex() {
        let mut app = AppRecord::new("App".to_string(), HashMap::new());
        app.invalid_version_number_field_regex = Some("alpha|beta".to_string());
        assert!(need_complete_version(&app));
    }

    #[test]
    fn test_need_complete_version_true_when_include_regex() {
        let mut app = AppRecord::new("App".to_string(), HashMap::new());
        app.include_version_number_field_regex = Some(r"\d+\.\d+".to_string());
        assert!(need_complete_version(&app));
    }

    #[test]
    fn test_need_complete_version_true_when_both_regex() {
        let mut app = AppRecord::new("App".to_string(), HashMap::new());
        app.invalid_version_number_field_regex = Some("alpha".to_string());
        app.include_version_number_field_regex = Some(r"\d+".to_string());
        assert!(need_complete_version(&app));
    }

    // -------------------------------------------------------------------------
    // Phase 7B: check_invalid_applications
    // -------------------------------------------------------------------------

    fn make_app_with_hubs(name: &str, hubs: &[&str]) -> AppRecord {
        let mut app = AppRecord::new(name.to_string(), HashMap::new());
        let hub_strs: Vec<String> = hubs.iter().map(|s| s.to_string()).collect();
        app.set_sorted_hub_uuids(&hub_strs);
        app
    }

    async fn app_manager_with_apps(apps: Vec<AppRecord>) -> AppManager {
        let map = apps.into_iter().map(|a| (a.id.clone(), a)).collect();
        AppManager {
            apps: Arc::new(RwLock::new(map)),
            virtual_apps: Arc::new(RwLock::new(vec![])),
            version_maps: Arc::new(RwLock::new(HashMap::new())),
            data_getter: Arc::new(DataGetter::new()),
        }
    }

    #[tokio::test]
    async fn test_check_invalid_no_apps() {
        let mgr = app_manager_with_apps(vec![]).await;
        let invalid = mgr.check_invalid_applications(&["hub-1".to_string()]).await;
        assert!(invalid.is_empty());
    }

    #[tokio::test]
    async fn test_check_invalid_empty_hub_list_not_reported() {
        // App with no hub list means "use all hubs" — never invalid.
        let app = AppRecord::new("NoHubs".to_string(), HashMap::new());
        let mgr = app_manager_with_apps(vec![app]).await;
        let invalid = mgr.check_invalid_applications(&[]).await;
        assert!(invalid.is_empty());
    }

    #[tokio::test]
    async fn test_check_invalid_all_hubs_known() {
        let app = make_app_with_hubs("GoodApp", &["hub-a", "hub-b"]);
        let mgr = app_manager_with_apps(vec![app]).await;
        let known = vec!["hub-a".to_string(), "hub-b".to_string()];
        let invalid = mgr.check_invalid_applications(&known).await;
        assert!(invalid.is_empty());
    }

    #[tokio::test]
    async fn test_check_invalid_one_hub_known_is_valid() {
        // Even if only one of the listed hubs is known, the app is valid.
        let app = make_app_with_hubs("SemiGood", &["hub-a", "hub-unknown"]);
        let mgr = app_manager_with_apps(vec![app]).await;
        let known = vec!["hub-a".to_string()];
        let invalid = mgr.check_invalid_applications(&known).await;
        assert!(invalid.is_empty());
    }

    #[tokio::test]
    async fn test_check_invalid_all_hubs_unknown() {
        let app = make_app_with_hubs("BadApp", &["hub-x", "hub-y"]);
        let app_id = app.id.clone();
        let mgr = app_manager_with_apps(vec![app]).await;
        let known = vec!["hub-a".to_string()];
        let invalid = mgr.check_invalid_applications(&known).await;
        assert_eq!(invalid, vec![app_id]);
    }

    #[tokio::test]
    async fn test_check_invalid_mixed_apps() {
        let good = make_app_with_hubs("Good", &["hub-a"]);
        let bad = make_app_with_hubs("Bad", &["hub-z"]);
        let bad_id = bad.id.clone();
        let no_hub = AppRecord::new("NoHub".to_string(), HashMap::new());
        let mgr = app_manager_with_apps(vec![good, bad, no_hub]).await;
        let known = vec!["hub-a".to_string()];
        let mut invalid = mgr.check_invalid_applications(&known).await;
        invalid.sort();
        assert_eq!(invalid, vec![bad_id]);
    }
}
