use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseData {
    pub version_number: String,
    pub changelog: String,
    pub assets: Vec<AssetData>,
    pub extra: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetData {
    pub file_name: String,
    pub file_type: String,
    pub download_url: String,
}
