use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// AppConfig
///
/// JSON Schema:
/// ```json
/// {
///   "base_version": 2,
///   "config_version": 1,
///   "uuid": "",
///   "base_hub_uuid": "",
///   "info": {
///     "name": "",
///     "url": "",
///     "extra_map": {
///       "android_app_package": ""
///     }
///   }
/// }
/// ```

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppItem {
    #[serde(rename = "base_version")]
    pub base_version: i32,

    #[serde(rename = "config_version")]
    pub config_version: i32,

    #[serde(rename = "uuid")]
    pub uuid: String,

    #[serde(rename = "base_hub_uuid")]
    pub base_hub_uuid: String,

    #[serde(rename = "info")]
    pub info: AppInfo,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppInfo {
    #[serde(rename = "name")]
    pub name: String,

    #[serde(rename = "url")]
    pub url: String,

    #[serde(rename = "extra_map")]
    pub extra_map: HashMap<String, String>, // Use HashMap to store arbitrary key/value pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_app_config() {
        let json = r#"
{
  "base_version": 2,
  "config_version": 1,
  "uuid": "f27f71e1-d7a1-4fd1-bbcc-9744380611a1",
  "base_hub_uuid": "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
  "info": {
    "name": "UpgradeAll",
    "url": "https://github.com/xz-dev/UpgradeAll",
    "extra_map": {
      "android_app_package": "net.xzos.upgradeall"
    }
  }
}
"#;

        let app_item: AppItem = serde_json::from_str(json).unwrap();
        assert_eq!(app_item.base_version, 2);
        assert_eq!(app_item.config_version, 1);
        assert_eq!(app_item.uuid, "f27f71e1-d7a1-4fd1-bbcc-9744380611a1");
        assert_eq!(
            app_item.base_hub_uuid,
            "fd9b2602-62c5-4d55-bd1e-0d6537714ca0"
        );
        assert_eq!(app_item.info.name, "UpgradeAll");
        assert_eq!(app_item.info.url, "https://github.com/xz-dev/UpgradeAll");
        assert_eq!(
            app_item.info.extra_map,
            HashMap::from([(
                "android_app_package".to_string(),
                "net.xzos.upgradeall".to_string()
            )])
        );
    }
}
