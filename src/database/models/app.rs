use crate::websdk::cloud_rules::data::app_item::AppItem;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppRecord {
    /// UUID v4 identifier (replaces Room's auto-increment Long id)
    pub id: String,
    pub name: String,
    pub app_id: HashMap<String, Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalid_version_number_field_regex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_version_number_field_regex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore_version_number: Option<String>,
    /// Cloud config (AppItem), optional
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloud_config: Option<AppItem>,
    /// Space-separated hub UUIDs in priority order
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_hub_list: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub star: Option<bool>,
}

impl AppRecord {
    pub fn new(name: String, app_id: HashMap<String, Option<String>>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            app_id,
            invalid_version_number_field_regex: None,
            include_version_number_field_regex: None,
            ignore_version_number: None,
            cloud_config: None,
            enable_hub_list: None,
            star: None,
        }
    }

    pub fn get_sorted_hub_uuids(&self) -> Vec<String> {
        match &self.enable_hub_list {
            Some(s) if !s.is_empty() => s.split(' ').map(String::from).collect(),
            _ => vec![],
        }
    }

    pub fn set_sorted_hub_uuids(&mut self, uuids: &[String]) {
        let s = uuids.join(" ");
        self.enable_hub_list = if s.is_empty() { None } else { Some(s) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_app() -> AppRecord {
        AppRecord {
            id: "test-uuid".to_string(),
            name: "TestApp".to_string(),
            app_id: HashMap::from([("owner".to_string(), Some("alice".to_string()))]),
            invalid_version_number_field_regex: None,
            include_version_number_field_regex: None,
            ignore_version_number: None,
            cloud_config: None,
            enable_hub_list: Some("hub1 hub2".to_string()),
            star: Some(true),
        }
    }

    #[test]
    fn test_serialization_roundtrip() {
        let app = sample_app();
        let json = serde_json::to_string(&app).unwrap();
        let decoded: AppRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(app, decoded);
    }

    #[test]
    fn test_get_sorted_hub_uuids() {
        let app = sample_app();
        let uuids = app.get_sorted_hub_uuids();
        assert_eq!(uuids, vec!["hub1", "hub2"]);
    }

    #[test]
    fn test_set_sorted_hub_uuids() {
        let mut app = sample_app();
        app.set_sorted_hub_uuids(&["a".to_string(), "b".to_string(), "c".to_string()]);
        assert_eq!(app.enable_hub_list, Some("a b c".to_string()));
    }

    #[test]
    fn test_empty_hub_list_is_none() {
        let mut app = sample_app();
        app.set_sorted_hub_uuids(&[]);
        assert_eq!(app.enable_hub_list, None);
    }
}
