pub mod models;
pub mod store;

use models::{
    app::AppRecord, extra_app::ExtraAppRecord, extra_hub::ExtraHubRecord, hub::HubRecord,
};
use once_cell::sync::OnceCell;
use std::path::Path;

use crate::error::Result;
use store::{HasId, JsonlStore};

impl HasId for AppRecord {
    fn id(&self) -> &str {
        &self.id
    }
}

impl HasId for HubRecord {
    fn id(&self) -> &str {
        &self.uuid
    }
}

impl HasId for ExtraAppRecord {
    fn id(&self) -> &str {
        &self.id
    }
}

impl HasId for ExtraHubRecord {
    fn id(&self) -> &str {
        &self.id
    }
}

pub struct Database {
    pub apps: JsonlStore,
    pub hubs: JsonlStore,
    pub extra_apps: JsonlStore,
    pub extra_hubs: JsonlStore,
}

impl Database {
    pub fn open(data_dir: &Path) -> Result<Self> {
        let db = Self {
            apps: JsonlStore::new(data_dir.join("apps.jsonl")),
            hubs: JsonlStore::new(data_dir.join("hubs.jsonl")),
            extra_apps: JsonlStore::new(data_dir.join("extra_apps.jsonl")),
            extra_hubs: JsonlStore::new(data_dir.join("extra_hubs.jsonl")),
        };
        db.apps.ensure_file()?;
        db.hubs.ensure_file()?;
        db.extra_apps.ensure_file()?;
        db.extra_hubs.ensure_file()?;
        Ok(db)
    }

    // --- App CRUD ---

    pub fn load_apps(&self) -> Result<Vec<AppRecord>> {
        self.apps.load_all()
    }

    pub fn upsert_app(&self, record: &AppRecord) -> Result<()> {
        self.apps.upsert(record)
    }

    pub fn delete_app(&self, id: &str) -> Result<bool> {
        self.apps.delete::<AppRecord>(id)
    }

    pub fn find_app(&self, id: &str) -> Result<Option<AppRecord>> {
        self.apps.find_by_id(id)
    }

    // --- Hub CRUD ---

    pub fn load_hubs(&self) -> Result<Vec<HubRecord>> {
        self.hubs.load_all()
    }

    pub fn upsert_hub(&self, record: &HubRecord) -> Result<()> {
        self.hubs.upsert(record)
    }

    pub fn delete_hub(&self, uuid: &str) -> Result<bool> {
        self.hubs.delete::<HubRecord>(uuid)
    }

    pub fn find_hub(&self, uuid: &str) -> Result<Option<HubRecord>> {
        self.hubs.find_by_id(uuid)
    }

    // --- ExtraApp CRUD ---

    pub fn load_extra_apps(&self) -> Result<Vec<ExtraAppRecord>> {
        self.extra_apps.load_all()
    }

    pub fn upsert_extra_app(&self, record: &ExtraAppRecord) -> Result<()> {
        self.extra_apps.upsert(record)
    }

    pub fn delete_extra_app(&self, id: &str) -> Result<bool> {
        self.extra_apps.delete::<ExtraAppRecord>(id)
    }

    /// Find an ExtraApp record by matching its `app_id` map.
    pub fn get_extra_app_by_app_id(
        &self,
        app_id: &std::collections::HashMap<String, Option<String>>,
    ) -> Result<Option<ExtraAppRecord>> {
        let all = self.extra_apps.load_all::<ExtraAppRecord>()?;
        Ok(all.into_iter().find(|r| &r.app_id == app_id))
    }

    // --- ExtraHub CRUD ---

    pub fn load_extra_hubs(&self) -> Result<Vec<ExtraHubRecord>> {
        self.extra_hubs.load_all()
    }

    pub fn upsert_extra_hub(&self, record: &ExtraHubRecord) -> Result<()> {
        self.extra_hubs.upsert(record)
    }

    pub fn delete_extra_hub(&self, id: &str) -> Result<bool> {
        self.extra_hubs.delete::<ExtraHubRecord>(id)
    }

    pub fn find_extra_hub(&self, id: &str) -> Result<Option<ExtraHubRecord>> {
        self.extra_hubs.find_by_id(id)
    }
}

static DB: OnceCell<Database> = OnceCell::new();

/// Initialize the global database. Must be called once before `get_db()`.
pub fn init_db(data_dir: &Path) -> Result<()> {
    let db = Database::open(data_dir)?;
    DB.set(db)
        .map_err(|_| crate::error::Error::Other("Database already initialized".to_string()))
}

/// Get the global database instance. Panics if `init_db` was not called.
pub fn get_db() -> &'static Database {
    DB.get()
        .expect("Database not initialized. Call init_db() first.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn open_test_db() -> (Database, TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).unwrap();
        (db, dir)
    }

    #[test]
    fn test_open_creates_files() {
        let dir = tempfile::tempdir().unwrap();
        Database::open(dir.path()).unwrap();
        assert!(dir.path().join("apps.jsonl").exists());
        assert!(dir.path().join("hubs.jsonl").exists());
        assert!(dir.path().join("extra_apps.jsonl").exists());
        assert!(dir.path().join("extra_hubs.jsonl").exists());
    }

    #[test]
    fn test_app_crud() {
        let (db, _dir) = open_test_db();
        let app = AppRecord::new(
            "TestApp".to_string(),
            std::collections::HashMap::from([("owner".to_string(), Some("alice".to_string()))]),
        );
        db.upsert_app(&app).unwrap();

        let apps = db.load_apps().unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "TestApp");

        let found = db.find_app(&app.id).unwrap();
        assert!(found.is_some());

        let deleted = db.delete_app(&app.id).unwrap();
        assert!(deleted);
        assert!(db.load_apps().unwrap().is_empty());
    }

    #[test]
    fn test_hub_crud() {
        use crate::websdk::cloud_rules::data::hub_item::{HubItem, Info};
        let (db, _dir) = open_test_db();
        let hub = HubRecord::new(
            "fd9b2602-62c5-4d55-bd1e-0d6537714ca0".to_string(),
            HubItem {
                base_version: 6,
                config_version: 1,
                uuid: "fd9b2602-62c5-4d55-bd1e-0d6537714ca0".to_string(),
                info: Info {
                    hub_name: "GitHub".to_string(),
                    hub_icon_url: None,
                },
                api_keywords: vec!["owner".to_string(), "repo".to_string()],
                app_url_templates: vec![],
                target_check_api: None,
            },
        );
        db.upsert_hub(&hub).unwrap();
        let hubs = db.load_hubs().unwrap();
        assert_eq!(hubs.len(), 1);
        assert_eq!(hubs[0].uuid, "fd9b2602-62c5-4d55-bd1e-0d6537714ca0");

        let deleted = db
            .delete_hub("fd9b2602-62c5-4d55-bd1e-0d6537714ca0")
            .unwrap();
        assert!(deleted);
        assert!(db.load_hubs().unwrap().is_empty());
    }
}
