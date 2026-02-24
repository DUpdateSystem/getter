use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtraAppRecord {
    /// UUID v4 identifier
    pub id: String,
    pub app_id: HashMap<String, Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mark_version_number: Option<String>,
}

impl ExtraAppRecord {
    pub fn new(app_id: HashMap<String, Option<String>>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            app_id,
            mark_version_number: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialization_roundtrip() {
        let record = ExtraAppRecord {
            id: "test-uuid".to_string(),
            app_id: HashMap::from([(
                "android_app_package".to_string(),
                Some("com.foo".to_string()),
            )]),
            mark_version_number: Some("1.2.3".to_string()),
        };
        let json = serde_json::to_string(&record).unwrap();
        let decoded: ExtraAppRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, decoded);
    }

    #[test]
    fn test_none_fields_skipped() {
        let record = ExtraAppRecord::new(HashMap::new());
        let json = serde_json::to_string(&record).unwrap();
        assert!(!json.contains("mark_version_number"));
    }
}
