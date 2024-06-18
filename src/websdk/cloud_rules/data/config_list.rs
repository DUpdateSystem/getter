use serde::{Deserialize, Serialize};

use super::app_item::AppItem;
use super::hub_item::HubItem;

/// Configuration lists
///
/// JSON Schema:
/// ```json
/// {
///   "app_config_list": [<AppConfig>],
///   "hub_config_list": [<HubConfig>]
/// }
/// ```

#[derive(Default, Serialize, Debug, Clone)]
pub struct ConfigListViewer<'a> {
    #[serde(rename = "app_config_list")]
    pub app_config_list: Vec<&'a AppItem>,

    #[serde(rename = "hub_config_list")]
    pub hub_config_list: Vec<&'a HubItem>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConfigList {
    #[serde(rename = "app_config_list")]
    pub app_config_list: Vec<AppItem>,

    #[serde(rename = "hub_config_list")]
    pub hub_config_list: Vec<HubItem>,
}

impl ConfigList {
    pub fn viewer(&self) -> ConfigListViewer {
        ConfigListViewer {
            app_config_list: self.app_config_list.iter().collect(),
            hub_config_list: self.hub_config_list.iter().collect(),
        }
    }
}

impl ConfigListViewer<'_> {
    pub fn to_owned(&self) -> ConfigList {
        ConfigList {
            app_config_list: self.app_config_list.iter().map(|&x| x.clone()).collect(),
            hub_config_list: self.hub_config_list.iter().map(|&x| x.clone()).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;
    use std::fs;

    #[test]
    fn test_config_list() {
        // read json from file
        let json = fs::read_to_string("tests/files/data/UpgradeAll-rules_rules.json").unwrap();

        let config_list: ConfigList = serde_json::from_str(&json).unwrap();

        // check app_config_list
        assert_eq!(config_list.app_config_list.len(), 219);
        assert_eq!(config_list.app_config_list[0].info.name, "UpgradeAll");
        assert_eq!(
            config_list.app_config_list.last().unwrap().info.name,
            "黑阈"
        );
        // check hub_config_list
        assert_eq!(config_list.hub_config_list.len(), 11);
        assert_eq!(config_list.hub_config_list[0].info.hub_name, "GitHub");
        assert_eq!(
            config_list.hub_config_list.last().unwrap().info.hub_name,
            "Xposed Module Repository"
        );
    }
}
