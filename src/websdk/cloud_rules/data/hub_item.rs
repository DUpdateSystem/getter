use serde::{Serialize, Deserialize};

/// HubConfig
///
/// JSON Schema:
/// ```json
/// {
///   base_version: 6
///   config_version: 1
///   uuid: ""
///   info: {
///     "hub_name": "",
///     "hub_icon_url": ""
///   }
///   target_check_api: ""
///   api_keywords: []
///   app_url_templates": []
/// }
/// ```

#[derive(Serialize, Deserialize, Debug)]
pub struct HubItem{
    #[serde(rename = "base_version", default)]
    pub base_version: i32,

    #[serde(rename = "config_version", default)]
    pub config_version: i32,

    #[serde(rename = "uuid", default)]
    pub uuid: String,

    #[serde(rename = "info")]
    pub info: Info,

    #[serde(rename = "api_keywords", default)]
    pub api_keywords: Vec<String>,

    #[serde(rename = "app_url_templates", default)]
    pub app_url_templates: Vec<String>,

    #[serde(rename = "target_check_api")]
    pub target_check_api: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Info {
    #[serde(rename = "hub_name", default)]
    pub hub_name: String,

    #[serde(rename = "hub_icon_url", default)]
    pub hub_icon_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hub_item() {
        let json = r#"
{
  "base_version": 6,
  "config_version": 3,
  "uuid": "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
  "info": {
    "hub_name": "GitHub",
    "hub_icon_url": ""
  },
  "target_check_api": "",
  "api_keywords": [
    "owner",
    "repo"
  ],
  "app_url_templates": [
    "https://github.com/%owner/%repo/"
  ]
}
        "#;

        let hub_item: HubItem = serde_json::from_str(json).unwrap();
        assert_eq!(hub_item.base_version, 6);
        assert_eq!(hub_item.config_version, 3);
        assert_eq!(hub_item.uuid, "fd9b2602-62c5-4d55-bd1e-0d6537714ca0");
        assert_eq!(hub_item.info.hub_name, "GitHub");
        assert_eq!(hub_item.info.hub_icon_url, Some("".to_string()));
        assert_eq!(hub_item.target_check_api, Some("".to_string()));
        assert_eq!(hub_item.api_keywords, ["owner", "repo"]);
        assert_eq!(hub_item.app_url_templates[0], "https://github.com/%owner/%repo/");
    }
}
