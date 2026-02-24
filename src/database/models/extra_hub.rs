use serde::{Deserialize, Serialize};

pub const GLOBAL_HUB_ID: &str = "GLOBAL";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtraHubRecord {
    /// Hub UUID or GLOBAL_HUB_ID
    pub id: String,
    #[serde(default)]
    pub enable_global: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_replace_search: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_replace_string: Option<String>,
}

impl ExtraHubRecord {
    pub fn new(id: String) -> Self {
        Self {
            id,
            enable_global: false,
            url_replace_search: None,
            url_replace_string: None,
        }
    }

    pub fn global() -> Self {
        Self::new(GLOBAL_HUB_ID.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialization_roundtrip() {
        let record = ExtraHubRecord {
            id: "some-hub-uuid".to_string(),
            enable_global: true,
            url_replace_search: Some("github.com".to_string()),
            url_replace_string: Some("mirror.example.com".to_string()),
        };
        let json = serde_json::to_string(&record).unwrap();
        let decoded: ExtraHubRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, decoded);
    }

    #[test]
    fn test_none_fields_skipped() {
        let record = ExtraHubRecord::global();
        let json = serde_json::to_string(&record).unwrap();
        assert!(!json.contains("url_replace_search"));
        assert!(!json.contains("url_replace_string"));
    }
}
