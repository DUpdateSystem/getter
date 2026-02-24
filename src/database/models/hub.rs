use crate::websdk::cloud_rules::data::hub_item::HubItem;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HubRecord {
    /// Hub UUID — primary key
    pub uuid: String,
    pub hub_config: HubItem,
    pub auth: HashMap<String, String>,
    #[serde(default)]
    pub ignore_app_id_list: Vec<HashMap<String, Option<String>>>,
    /// 0 = disabled, 1 = enabled
    #[serde(default)]
    pub applications_mode: i32,
    #[serde(default)]
    pub user_ignore_app_id_list: Vec<HashMap<String, Option<String>>>,
    /// Lower = higher priority. Default is -(hub list size).
    #[serde(default)]
    pub sort_point: i32,
}

impl HubRecord {
    pub fn new(uuid: String, hub_config: HubItem) -> Self {
        Self {
            uuid,
            hub_config,
            auth: HashMap::new(),
            ignore_app_id_list: vec![],
            applications_mode: 0,
            user_ignore_app_id_list: vec![],
            sort_point: 0,
        }
    }

    pub fn applications_mode_enabled(&self) -> bool {
        self.applications_mode == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::websdk::cloud_rules::data::hub_item::Info;

    fn sample_hub() -> HubRecord {
        HubRecord::new(
            "fd9b2602-62c5-4d55-bd1e-0d6537714ca0".to_string(),
            HubItem {
                base_version: 6,
                config_version: 3,
                uuid: "fd9b2602-62c5-4d55-bd1e-0d6537714ca0".to_string(),
                info: Info {
                    hub_name: "GitHub".to_string(),
                    hub_icon_url: None,
                },
                api_keywords: vec!["owner".to_string(), "repo".to_string()],
                app_url_templates: vec!["https://github.com/%owner/%repo/".to_string()],
                target_check_api: None,
            },
        )
    }

    #[test]
    fn test_serialization_roundtrip() {
        let hub = sample_hub();
        let json = serde_json::to_string(&hub).unwrap();
        let decoded: HubRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(hub, decoded);
    }

    #[test]
    fn test_applications_mode() {
        let mut hub = sample_hub();
        assert!(!hub.applications_mode_enabled());
        hub.applications_mode = 1;
        assert!(hub.applications_mode_enabled());
    }
}
