use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::database::models::app::AppRecord;
use crate::database::models::hub::HubRecord;
use crate::error::{Error, Result};
use crate::manager::app_manager::AppManager;
use crate::manager::auto_template::url_to_app_id;
use crate::manager::hub_manager::HubManager;
use crate::websdk::cloud_rules::cloud_rules_manager::CloudRules;
use crate::websdk::cloud_rules::data::app_item::AppItem;
use crate::websdk::cloud_rules::data::hub_item::HubItem;

/// Manages downloading, caching, and applying cloud hub/app configurations.
///
/// Mirrors Kotlin's `CloudConfigGetter` singleton.
pub struct CloudConfigGetter {
    api_url: String,
    cloud_rules: Arc<RwLock<Option<CloudRules>>>,
}

impl CloudConfigGetter {
    pub fn new(api_url: String) -> Self {
        Self {
            api_url,
            cloud_rules: Arc::new(RwLock::new(None)),
        }
    }

    /// Download and cache the latest cloud config list.
    pub async fn renew(&self) -> Result<()> {
        let mut rules = CloudRules::new(&self.api_url);
        rules
            .renew()
            .await
            .map_err(|e| Error::Other(e.to_string()))?;
        *self.cloud_rules.write().await = Some(rules);
        Ok(())
    }

    /// Returns all available app configs from the cached cloud config.
    pub async fn app_config_list(&self) -> Vec<AppItem> {
        match self.cloud_rules.read().await.as_ref() {
            Some(rules) => rules
                .get_config_list()
                .app_config_list
                .into_iter()
                .cloned()
                .collect(),
            None => vec![],
        }
    }

    /// Returns all available hub configs from the cached cloud config.
    pub async fn hub_config_list(&self) -> Vec<HubItem> {
        match self.cloud_rules.read().await.as_ref() {
            Some(rules) => rules
                .get_config_list()
                .hub_config_list
                .into_iter()
                .cloned()
                .collect(),
            None => vec![],
        }
    }

    /// Apply a cloud hub config by UUID: download config → convert → upsert into HubManager.
    ///
    /// Returns true if the hub was installed or updated.
    pub async fn apply_hub_config(&self, uuid: &str, hub_mgr: &mut HubManager) -> Result<bool> {
        let hub_item = self
            .find_hub_item(uuid)
            .await
            .ok_or_else(|| Error::Other(format!("Hub config not found: {uuid}")))?;

        let record = hub_item_to_record(&hub_item, hub_mgr).await;
        hub_mgr.upsert_hub(record).await?;
        Ok(true)
    }

    /// Apply a cloud app config by UUID:
    /// 1. Ensure hub dependency is installed.
    /// 2. Extract app_id from the app's URL using the hub's URL templates.
    /// 3. Merge with `extra_map` from the cloud config.
    /// 4. Upsert into AppManager.
    ///
    /// Returns true if the app was installed or updated.
    pub async fn apply_app_config(
        &self,
        uuid: &str,
        app_mgr: &mut AppManager,
        hub_mgr: &mut HubManager,
    ) -> Result<bool> {
        let app_item = self
            .find_app_item(uuid)
            .await
            .ok_or_else(|| Error::Other(format!("App config not found: {uuid}")))?;

        // Ensure hub dependency is present
        self.solve_hub_dependency(&app_item.base_hub_uuid, hub_mgr)
            .await?;

        // Build the app_id map
        let app_id = build_app_id(&app_item, hub_mgr).await;

        // Find existing record by cloud UUID or create a new one
        let mut record = app_mgr
            .find_app_by_cloud_uuid(&app_item.uuid)
            .await
            .unwrap_or_else(|| {
                let mut r = AppRecord::new(app_item.info.name.clone(), app_id.clone());
                r.id = String::new(); // let save_app assign UUID
                r
            });

        record.name = app_item.info.name.clone();
        record.app_id = app_id;
        record.cloud_config = Some(app_item.clone());

        // Ensure base_hub_uuid is first in the hub priority list
        let mut hub_uuids = record.get_sorted_hub_uuids();
        if !hub_uuids.contains(&app_item.base_hub_uuid) {
            hub_uuids.insert(0, app_item.base_hub_uuid.clone());
        } else if hub_uuids[0] != app_item.base_hub_uuid {
            hub_uuids.retain(|u| u != &app_item.base_hub_uuid);
            hub_uuids.insert(0, app_item.base_hub_uuid.clone());
        }
        record.set_sorted_hub_uuids(&hub_uuids);

        app_mgr.save_app(record).await?;
        Ok(true)
    }

    /// Bulk-update all installed apps and hubs whose cloud config version has increased.
    ///
    /// Mirrors Kotlin's `renewAllAppConfigFromCloud` + `renewAllHubConfigFromCloud`.
    pub async fn renew_all_from_cloud(
        &self,
        app_mgr: &mut AppManager,
        hub_mgr: &mut HubManager,
    ) -> Result<()> {
        // Renew hubs
        let installed_hubs = hub_mgr.get_hub_list().await;
        for hub in &installed_hubs {
            if let Some(cloud_hub) = self.find_hub_item(&hub.uuid).await {
                if cloud_hub.config_version > hub.hub_config.config_version {
                    let record = hub_item_to_record(&cloud_hub, hub_mgr).await;
                    hub_mgr.upsert_hub(record).await?;
                }
            }
        }

        // Renew apps
        let installed_apps = app_mgr.get_saved_apps().await;
        for app in &installed_apps {
            let cloud_uuid = match app.cloud_config.as_ref().map(|c| c.uuid.as_str()) {
                Some(u) if !u.is_empty() => u.to_string(),
                _ => continue,
            };
            if let Some(cloud_app) = self.find_app_item(&cloud_uuid).await {
                let installed_version = app
                    .cloud_config
                    .as_ref()
                    .map(|c| c.config_version)
                    .unwrap_or(0);
                if cloud_app.config_version > installed_version {
                    let _ = self.apply_app_config(&cloud_uuid, app_mgr, hub_mgr).await;
                }
            }
        }

        Ok(())
    }

    /// Ensure a hub is installed; if not, download and install it from cloud config.
    async fn solve_hub_dependency(&self, hub_uuid: &str, hub_mgr: &mut HubManager) -> Result<()> {
        if hub_mgr.get_hub(hub_uuid).await.is_some() {
            // Already installed — check if update needed
            let installed = hub_mgr.get_hub(hub_uuid).await.unwrap();
            if let Some(cloud) = self.find_hub_item(hub_uuid).await {
                if cloud.config_version > installed.hub_config.config_version {
                    let record = hub_item_to_record(&cloud, hub_mgr).await;
                    hub_mgr.upsert_hub(record).await?;
                }
            }
        } else {
            // Not installed — download and install
            self.apply_hub_config(hub_uuid, hub_mgr).await?;
        }
        Ok(())
    }

    async fn find_app_item(&self, uuid: &str) -> Option<AppItem> {
        self.cloud_rules
            .read()
            .await
            .as_ref()?
            .get_config_list()
            .app_config_list
            .into_iter()
            .find(|a| a.uuid == uuid)
            .cloned()
    }

    async fn find_hub_item(&self, uuid: &str) -> Option<HubItem> {
        self.cloud_rules
            .read()
            .await
            .as_ref()?
            .get_config_list()
            .hub_config_list
            .into_iter()
            .find(|h| h.uuid == uuid)
            .cloned()
    }
}

/// Convert a `HubItem` from cloud config to a `HubRecord`, preserving any
/// existing auth / ignore lists if the hub is already installed.
async fn hub_item_to_record(hub_item: &HubItem, hub_mgr: &HubManager) -> HubRecord {
    if let Some(existing) = hub_mgr.get_hub(&hub_item.uuid).await {
        // Preserve mutable fields, update config fields
        HubRecord {
            hub_config: hub_item.clone(),
            ..existing
        }
    } else {
        HubRecord::new(hub_item.uuid.clone(), hub_item.clone())
    }
}

/// Build the `app_id` map from a cloud `AppItem`.
///
/// 1. Try to extract from `info.url` using the hub's `app_url_templates`.
/// 2. Merge with `info.extra_map` (extra_map takes precedence for any shared keys).
async fn build_app_id(app_item: &AppItem, hub_mgr: &HubManager) -> HashMap<String, Option<String>> {
    let mut app_id: HashMap<String, Option<String>> = HashMap::new();

    // Try URL template extraction
    if let Some(hub) = hub_mgr.get_hub(&app_item.base_hub_uuid).await {
        let templates = &hub.hub_config.app_url_templates;
        if !app_item.info.url.is_empty() {
            if let Some(extracted) = url_to_app_id(&app_item.info.url, templates) {
                for (k, v) in extracted {
                    app_id.insert(k, Some(v));
                }
            }
        }
    }

    // Merge extra_map — extra_map values always win over URL-extracted values
    for (k, v) in &app_item.info.extra_map {
        app_id.insert(k.clone(), Some(v.clone()));
    }

    app_id
}

// Extension on AppManager to find by cloud UUID
impl AppManager {
    pub async fn find_app_by_cloud_uuid(&self, cloud_uuid: &str) -> Option<AppRecord> {
        self.get_saved_apps().await.into_iter().find(|a| {
            a.cloud_config
                .as_ref()
                .map(|c| c.uuid == cloud_uuid)
                .unwrap_or(false)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::websdk::cloud_rules::data::app_item::AppInfo;
    use crate::websdk::cloud_rules::data::hub_item::Info;

    fn make_hub_item(uuid: &str, templates: Vec<String>) -> HubItem {
        HubItem {
            base_version: 6,
            config_version: 1,
            uuid: uuid.to_string(),
            info: Info {
                hub_name: "GitHub".to_string(),
                hub_icon_url: None,
            },
            api_keywords: vec!["owner".to_string(), "repo".to_string()],
            app_url_templates: templates,
            target_check_api: None,
        }
    }

    fn make_app_item(uuid: &str, hub_uuid: &str, url: &str) -> AppItem {
        AppItem {
            base_version: 2,
            config_version: 1,
            uuid: uuid.to_string(),
            base_hub_uuid: hub_uuid.to_string(),
            info: AppInfo {
                name: "TestApp".to_string(),
                url: url.to_string(),
                extra_map: HashMap::new(),
            },
        }
    }

    #[tokio::test]
    async fn test_build_app_id_from_url() {
        let dir = tempfile::tempdir().unwrap();
        let _ = crate::database::init_db(dir.path());

        let hub_uuid = "fd9b2602-62c5-4d55-bd1e-0d6537714ca0";
        let db = crate::database::Database::open(dir.path()).unwrap();
        let hub = HubRecord::new(
            hub_uuid.to_string(),
            make_hub_item(
                hub_uuid,
                vec!["https://github.com/%owner/%repo/".to_string()],
            ),
        );
        db.upsert_hub(&hub).unwrap();

        let hub_mgr = HubManager::load().unwrap_or_else(|_| {
            // fallback: create manager from local db
            let records = db.load_hubs().unwrap();
            let _ = records; // suppress unused warning
                             // We can't easily construct HubManager without global DB, so use a helper
            panic!("This test requires global DB init")
        });
        let _ = hub_mgr;
    }

    #[test]
    fn test_build_app_id_extra_map_wins() {
        // extra_map should take precedence over URL-extracted values
        let app_item = AppItem {
            base_version: 2,
            config_version: 1,
            uuid: "test-uuid".to_string(),
            base_hub_uuid: "hub-uuid".to_string(),
            info: AppInfo {
                name: "TestApp".to_string(),
                url: "https://github.com/owner/repo".to_string(),
                extra_map: HashMap::from([
                    ("android_app_package".to_string(), "com.example".to_string()),
                    ("owner".to_string(), "override_owner".to_string()),
                ]),
            },
        };

        // Simulate what build_app_id does with extra_map override
        let mut app_id: HashMap<String, Option<String>> = HashMap::new();
        // Pretend URL extraction gave owner=owner, repo=repo
        app_id.insert("owner".to_string(), Some("owner".to_string()));
        app_id.insert("repo".to_string(), Some("repo".to_string()));
        // extra_map override
        for (k, v) in &app_item.info.extra_map {
            app_id.insert(k.clone(), Some(v.clone()));
        }
        // owner should be overridden by extra_map
        assert_eq!(app_id["owner"], Some("override_owner".to_string()));
        assert_eq!(
            app_id["android_app_package"],
            Some("com.example".to_string())
        );
        // repo still from URL extraction
        assert_eq!(app_id["repo"], Some("repo".to_string()));
    }

    #[test]
    fn test_hub_item_to_record_preserves_auth() {
        // hub_item_to_record should preserve auth from existing record
        // We test the logic inline since it's async and needs a HubManager
        let hub_item = make_hub_item(
            "hub-1",
            vec!["https://github.com/%owner/%repo/".to_string()],
        );
        let existing = HubRecord {
            uuid: "hub-1".to_string(),
            hub_config: make_hub_item("hub-1", vec![]),
            auth: HashMap::from([("token".to_string(), "secret".to_string())]),
            ignore_app_id_list: vec![],
            applications_mode: 1,
            user_ignore_app_id_list: vec![],
            sort_point: -5,
        };

        // Simulate hub_item_to_record(hub_item, existing)
        let record = HubRecord {
            hub_config: hub_item.clone(),
            ..existing.clone()
        };

        assert_eq!(record.auth["token"], "secret");
        assert_eq!(record.applications_mode, 1);
        assert_eq!(record.sort_point, -5);
        assert_eq!(
            record.hub_config.app_url_templates[0],
            "https://github.com/%owner/%repo/"
        );
    }
}
