use serde::{Deserialize, Serialize};

/// Configuration lists
///
/// JSON Schema:
/// ```json
/// {
///   "app_list": ["", ],
///   "hub_list": ["", ]
/// }
/// ```

#[derive(Serialize, Deserialize, Debug)]
pub struct RuleList {
    #[serde(rename = "app_list")]
    pub app_list: Vec<String>,

    #[serde(rename = "hub_list")]
    pub hub_list: Vec<String>,
}

impl RuleList {
    pub fn new() -> Self {
        RuleList {
            app_list: Vec::new(),
            hub_list: Vec::new(),
        }
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

        let config_list: RuleList = serde_json::from_str(&json).unwrap();

        // check app_config_list
        assert_eq!(config_list.app_list.len(), 2);
        assert_eq!(config_list.app_list[0], "UpgradeAll");
        // check hub_config_list
        assert_eq!(config_list.hub_list.len(), 1);
        assert_eq!(config_list.hub_list[0], "GitHub");
    }
}
