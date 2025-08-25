use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration lists
///
/// JSON Schema:
/// ```json
/// {
///   "app_list": ["", ],
///   "hub_list": ["", ],
///   "tracked_apps": {
///     "app_id": {
///       "hub_uuid": "github",
///       "app_data": {"owner": "rust-lang", "repo": "rust"},
///       "hub_data": {},
///       "current_version": "1.70.0",
///       "added_at": 1234567890,
///       "last_checked": 1234567890
///     }
///   }
/// }
/// ```

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TrackedApp {
    pub hub_uuid: String,
    pub app_data: HashMap<String, String>,
    pub hub_data: HashMap<String, String>,
    pub current_version: Option<String>,
    pub added_at: u64,
    pub last_checked: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RuleList {
    #[serde(rename = "app_list")]
    pub app_list: Vec<String>,

    #[serde(rename = "hub_list")]
    pub hub_list: Vec<String>,

    #[serde(rename = "tracked_apps", default)]
    pub tracked_apps: HashMap<String, TrackedApp>,
}

impl Default for RuleList {
    fn default() -> Self {
        Self::new()
    }
}

impl RuleList {
    pub fn new() -> Self {
        RuleList {
            app_list: Vec::new(),
            hub_list: Vec::new(),
            tracked_apps: HashMap::new(),
        }
    }

    pub fn add_tracked_app(
        &mut self,
        app_id: String,
        hub_uuid: String,
        app_data: HashMap<String, String>,
        hub_data: HashMap<String, String>,
    ) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let tracked_app = TrackedApp {
            hub_uuid,
            app_data,
            hub_data,
            current_version: None,
            added_at: now,
            last_checked: None,
        };

        self.tracked_apps.insert(app_id, tracked_app).is_none()
    }

    pub fn update_tracked_app_version(&mut self, app_id: &str, version: String) -> bool {
        if let Some(app) = self.tracked_apps.get_mut(app_id) {
            app.current_version = Some(version);
            app.last_checked = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            );
            true
        } else {
            false
        }
    }

    pub fn remove_tracked_app(&mut self, app_id: &str) -> bool {
        self.tracked_apps.remove(app_id).is_some()
    }

    pub fn get_tracked_app(&self, app_id: &str) -> Option<&TrackedApp> {
        self.tracked_apps.get(app_id)
    }

    pub fn list_tracked_apps(&self) -> Vec<(&String, &TrackedApp)> {
        self.tracked_apps.iter().collect()
    }

    pub fn push_app(&mut self, app_name: &str) -> bool {
        if self.app_list.contains(&app_name.to_string()) {
            false
        } else {
            self.app_list.push(app_name.to_string());
            true
        }
    }

    pub fn remove_app(&mut self, app_name: &str) -> bool {
        if let Some(index) = self.app_list.iter().position(|x| x == app_name) {
            self.app_list.remove(index);
            true
        } else {
            false
        }
    }

    pub fn push_hub(&mut self, hub_name: &str) -> bool {
        if self.hub_list.contains(&hub_name.to_string()) {
            false
        } else {
            self.hub_list.push(hub_name.to_string());
            true
        }
    }

    pub fn remove_hub(&mut self, hub_name: &str) -> bool {
        if let Some(index) = self.hub_list.iter().position(|x| x == hub_name) {
            self.hub_list.remove(index);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_config_list() {
        let json = r#"
{
  "app_list": ["UpgradeAll", ""],
  "hub_list": ["GitHub"]
}"#;

        let config_list: RuleList = serde_json::from_str(json).unwrap();

        // check app_config_list
        assert_eq!(config_list.app_list.len(), 2);
        assert_eq!(config_list.app_list[0], "UpgradeAll");
        // check hub_config_list
        assert_eq!(config_list.hub_list.len(), 1);
        assert_eq!(config_list.hub_list[0], "GitHub");
    }

    #[test]
    fn test_tracked_apps() {
        let mut rule_list = RuleList::new();
        assert!(rule_list.tracked_apps.is_empty());

        // Test adding tracked app
        let mut app_data = HashMap::new();
        app_data.insert("owner".to_string(), "rust-lang".to_string());
        app_data.insert("repo".to_string(), "rust".to_string());

        let hub_data = HashMap::new();

        let added = rule_list.add_tracked_app(
            "rust_lang_rust".to_string(),
            "fd9b2602-62c5-4d55-bd1e-0d6537714ca0".to_string(),
            app_data.clone(),
            hub_data.clone(),
        );

        assert!(added);
        assert_eq!(rule_list.tracked_apps.len(), 1);

        let tracked = rule_list.get_tracked_app("rust_lang_rust").unwrap();
        assert_eq!(tracked.hub_uuid, "fd9b2602-62c5-4d55-bd1e-0d6537714ca0");
        assert_eq!(tracked.app_data.get("owner").unwrap(), "rust-lang");
        assert_eq!(tracked.app_data.get("repo").unwrap(), "rust");
        assert!(tracked.current_version.is_none());
        assert!(tracked.last_checked.is_none());

        // Test adding duplicate app (should return false)
        let duplicate_added = rule_list.add_tracked_app(
            "rust_lang_rust".to_string(),
            "fd9b2602-62c5-4d55-bd1e-0d6537714ca0".to_string(),
            app_data,
            hub_data,
        );
        assert!(!duplicate_added);
        assert_eq!(rule_list.tracked_apps.len(), 1);
    }

    #[test]
    fn test_update_tracked_app_version() {
        let mut rule_list = RuleList::new();

        // Add app first
        let app_data = HashMap::from([
            ("owner".to_string(), "rust-lang".to_string()),
            ("repo".to_string(), "rust".to_string()),
        ]);
        let hub_data = HashMap::new();

        rule_list.add_tracked_app(
            "rust_lang_rust".to_string(),
            "fd9b2602-62c5-4d55-bd1e-0d6537714ca0".to_string(),
            app_data,
            hub_data,
        );

        // Update version
        let updated = rule_list.update_tracked_app_version("rust_lang_rust", "1.70.0".to_string());
        assert!(updated);

        let tracked = rule_list.get_tracked_app("rust_lang_rust").unwrap();
        assert_eq!(tracked.current_version.as_ref().unwrap(), "1.70.0");
        assert!(tracked.last_checked.is_some());

        // Update non-existent app
        let not_updated = rule_list.update_tracked_app_version("nonexistent", "1.0.0".to_string());
        assert!(!not_updated);
    }

    #[test]
    fn test_remove_tracked_app() {
        let mut rule_list = RuleList::new();

        // Add app first
        let app_data = HashMap::from([
            ("owner".to_string(), "rust-lang".to_string()),
            ("repo".to_string(), "rust".to_string()),
        ]);
        let hub_data = HashMap::new();

        rule_list.add_tracked_app(
            "rust_lang_rust".to_string(),
            "fd9b2602-62c5-4d55-bd1e-0d6537714ca0".to_string(),
            app_data,
            hub_data,
        );

        assert_eq!(rule_list.tracked_apps.len(), 1);

        // Remove existing app
        let removed = rule_list.remove_tracked_app("rust_lang_rust");
        assert!(removed);
        assert_eq!(rule_list.tracked_apps.len(), 0);

        // Remove non-existent app
        let not_removed = rule_list.remove_tracked_app("nonexistent");
        assert!(!not_removed);
    }

    #[test]
    fn test_list_tracked_apps() {
        let mut rule_list = RuleList::new();

        // Empty list
        assert!(rule_list.list_tracked_apps().is_empty());

        // Add multiple apps
        let app_data1 = HashMap::from([("repo".to_string(), "rust".to_string())]);
        let app_data2 = HashMap::from([("repo".to_string(), "cargo".to_string())]);
        let hub_data = HashMap::new();

        rule_list.add_tracked_app(
            "app1".to_string(),
            "uuid1".to_string(),
            app_data1,
            hub_data.clone(),
        );
        rule_list.add_tracked_app("app2".to_string(), "uuid2".to_string(), app_data2, hub_data);

        let apps = rule_list.list_tracked_apps();
        assert_eq!(apps.len(), 2);

        let app_ids: Vec<&str> = apps.iter().map(|(id, _)| id.as_str()).collect();
        assert!(app_ids.contains(&"app1"));
        assert!(app_ids.contains(&"app2"));
    }

    #[test]
    fn test_tracked_apps_json_serialization() {
        let mut rule_list = RuleList::new();

        let app_data = HashMap::from([
            ("owner".to_string(), "rust-lang".to_string()),
            ("repo".to_string(), "rust".to_string()),
        ]);
        let hub_data = HashMap::new();

        rule_list.add_tracked_app(
            "rust_lang_rust".to_string(),
            "fd9b2602-62c5-4d55-bd1e-0d6537714ca0".to_string(),
            app_data,
            hub_data,
        );

        rule_list.update_tracked_app_version("rust_lang_rust", "1.70.0".to_string());

        // Serialize to JSON
        let json = serde_json::to_string_pretty(&rule_list).unwrap();
        assert!(json.contains("tracked_apps"));
        assert!(json.contains("rust_lang_rust"));
        assert!(json.contains("fd9b2602-62c5-4d55-bd1e-0d6537714ca0"));
        assert!(json.contains("1.70.0"));

        // Deserialize back
        let deserialized: RuleList = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.tracked_apps.len(), 1);

        let tracked = deserialized.get_tracked_app("rust_lang_rust").unwrap();
        assert_eq!(tracked.hub_uuid, "fd9b2602-62c5-4d55-bd1e-0d6537714ca0");
        assert_eq!(tracked.current_version.as_ref().unwrap(), "1.70.0");
    }
}
