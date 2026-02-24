use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::database::get_db;
use crate::database::models::hub::HubRecord;
use crate::error::Result;

/// In-memory hub registry backed by the JSONL database.
///
/// Mirrors Kotlin's `HubManager`.
pub struct HubManager {
    hubs: Arc<RwLock<HashMap<String, HubRecord>>>,
}

impl HubManager {
    /// Load all hubs from the database.
    pub fn load() -> Result<Self> {
        let records = get_db().load_hubs()?;
        let map = records.into_iter().map(|h| (h.uuid.clone(), h)).collect();
        Ok(Self {
            hubs: Arc::new(RwLock::new(map)),
        })
    }

    pub async fn get_hub_list(&self) -> Vec<HubRecord> {
        self.hubs.read().await.values().cloned().collect()
    }

    pub async fn get_hub(&self, uuid: &str) -> Option<HubRecord> {
        self.hubs.read().await.get(uuid).cloned()
    }

    /// Insert or update a hub (persists to database).
    pub async fn upsert_hub(&self, record: HubRecord) -> Result<()> {
        get_db().upsert_hub(&record)?;
        self.hubs.write().await.insert(record.uuid.clone(), record);
        Ok(())
    }

    /// Remove a hub by UUID (persists deletion to database).
    pub async fn remove_hub(&self, uuid: &str) -> Result<bool> {
        let deleted = get_db().delete_hub(uuid)?;
        self.hubs.write().await.remove(uuid);
        Ok(deleted)
    }

    pub async fn is_applications_mode_enabled(&self) -> bool {
        self.hubs
            .read()
            .await
            .values()
            .any(|h| h.applications_mode_enabled())
    }

    /// Update the auth map for a hub identified by UUID and persist the change.
    ///
    /// Returns `false` if no hub with the given UUID exists.
    pub async fn update_auth(&self, uuid: &str, auth: HashMap<String, String>) -> Result<bool> {
        let mut hubs = self.hubs.write().await;
        let hub = match hubs.get_mut(uuid) {
            Some(h) => h,
            None => return Ok(false),
        };
        hub.auth = auth;
        get_db().upsert_hub(hub)?;
        Ok(true)
    }

    /// Return hubs whose api_keywords contain any of the given app_id keys.
    pub async fn hubs_for_app(&self, app_id: &HashMap<String, Option<String>>) -> Vec<HubRecord> {
        let app_keys: Vec<&str> = app_id.keys().map(String::as_str).collect();
        self.hubs
            .read()
            .await
            .values()
            .filter(|h| {
                h.hub_config
                    .api_keywords
                    .iter()
                    .any(|kw| app_keys.contains(&kw.as_str()))
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database;
    use crate::websdk::cloud_rules::data::hub_item::{HubItem, Info};
    use tempfile::TempDir;

    fn setup_db() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        database::init_db(dir.path()).ok(); // may already be init in other tests
        dir
    }

    fn make_hub(uuid: &str) -> HubRecord {
        HubRecord::new(
            uuid.to_string(),
            HubItem {
                base_version: 6,
                config_version: 1,
                uuid: uuid.to_string(),
                info: Info {
                    hub_name: "TestHub".to_string(),
                    hub_icon_url: None,
                },
                api_keywords: vec!["owner".to_string(), "repo".to_string()],
                auth_keywords: vec![],
                app_url_templates: vec![],
                target_check_api: None,
            },
        )
    }

    // These tests use a fresh TempDir + DB each time via open() directly,
    // bypassing the global singleton to allow parallel test runs.
    #[test]
    fn test_upsert_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::database::Database::open(dir.path()).unwrap();
        let hub = make_hub("uuid-1");
        db.upsert_hub(&hub).unwrap();
        let hubs = db.load_hubs().unwrap();
        assert_eq!(hubs.len(), 1);
        assert_eq!(hubs[0].uuid, "uuid-1");
    }

    #[test]
    fn test_delete_hub() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::database::Database::open(dir.path()).unwrap();
        let hub = make_hub("uuid-2");
        db.upsert_hub(&hub).unwrap();
        let deleted = db.delete_hub("uuid-2").unwrap();
        assert!(deleted);
        assert!(db.load_hubs().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_update_auth() {
        let dir = tempfile::tempdir().unwrap();
        crate::database::init_db(dir.path()).ok();

        // Insert the hub via HubManager so it is in both the global DB and in-memory state.
        let mgr = HubManager::load().unwrap();
        let hub = make_hub("uuid-auth");
        mgr.upsert_hub(hub).await.unwrap();

        let new_auth: HashMap<String, String> =
            [("token".to_string(), "ghp_test123".to_string())].into();
        let ok = mgr
            .update_auth("uuid-auth", new_auth.clone())
            .await
            .unwrap();
        assert!(ok);

        // Verify in-memory state updated.
        let updated = mgr.get_hub("uuid-auth").await.unwrap();
        assert_eq!(updated.auth, new_auth);

        // Returns false for unknown UUID.
        let not_found = mgr
            .update_auth("no-such-uuid", HashMap::new())
            .await
            .unwrap();
        assert!(!not_found);
    }
}
