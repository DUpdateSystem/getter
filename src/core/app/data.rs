use std::collections::BTreeMap;

use crate::websdk::cloud_rules::data::app_item::AppItem;

pub struct AppDb {
    cloud_config: Option<AppItem>,
    pub name: String,  // name is id
    pub app_id_map: BTreeMap<String, Option<String>>,
    pub invalid_version_number_field_regex: Option<String>,
    pub include_version_number_field_regex: Option<String>,
    pub ignore_version_number: Option<String>,
    pub enable_hub_list: Option<Vec<String>>,
    pub star: Option<bool>,
}
