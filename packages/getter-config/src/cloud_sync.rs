use crate::app_registry::{AppConfig, HubConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::path::Path;

/// Cloud configuration format (compatible with existing cloud_config.json)
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CloudConfig {
    #[serde(rename = "app_config_list")]
    pub app_config_list: Vec<CloudAppItem>,

    #[serde(rename = "hub_config_list")]
    pub hub_config_list: Vec<CloudHubItem>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CloudAppItem {
    #[serde(rename = "base_version")]
    pub base_version: i32,

    #[serde(rename = "config_version", default)]
    pub config_version: i32,

    #[serde(rename = "uuid")]
    pub uuid: String,

    #[serde(rename = "base_hub_uuid")]
    pub base_hub_uuid: String,

    #[serde(rename = "info")]
    pub info: CloudAppInfo,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CloudAppInfo {
    #[serde(rename = "name")]
    pub name: String,

    #[serde(rename = "url")]
    pub url: String,

    #[serde(rename = "extra_map", default)]
    pub extra_map: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CloudHubItem {
    #[serde(rename = "base_version")]
    pub base_version: i32,

    #[serde(rename = "config_version", default)]
    pub config_version: i32,

    #[serde(rename = "uuid")]
    pub uuid: String,

    #[serde(rename = "info")]
    pub info: CloudHubInfo,

    #[serde(rename = "target_check_api", default)]
    pub target_check_api: String,

    #[serde(rename = "api_keywords", default)]
    pub api_keywords: Vec<String>,

    #[serde(rename = "app_url_templates", default)]
    pub app_url_templates: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CloudHubInfo {
    #[serde(rename = "hub_name")]
    pub hub_name: String,

    #[serde(rename = "hub_icon_url", default)]
    pub hub_icon_url: String,
}

/// Sync cloud configuration to local repo layer
pub struct CloudSync {
    cloud_url: Option<String>,
    pub uuid_to_name_map: HashMap<String, String>, // UUID to human-readable name mapping
}

impl CloudSync {
    pub fn new() -> Self {
        Self {
            cloud_url: None,
            uuid_to_name_map: HashMap::new(),
        }
    }

    pub fn with_url(cloud_url: String) -> Self {
        Self {
            cloud_url: Some(cloud_url),
            uuid_to_name_map: HashMap::new(),
        }
    }

    /// Load cloud configuration from URL or file
    pub async fn fetch_cloud_config(&self) -> Result<CloudConfig, Box<dyn Error + Send + Sync>> {
        if let Some(url) = &self.cloud_url {
            // Fetch from URL
            let response = reqwest::get(url).await?;
            let config: CloudConfig = response.json().await?;
            Ok(config)
        } else {
            Err("No cloud URL configured".into())
        }
    }

    /// Load cloud configuration from a local file (for testing)
    pub fn load_from_file(&self, path: &Path) -> Result<CloudConfig, Box<dyn Error + Send + Sync>> {
        let content = std::fs::read_to_string(path)?;
        let config: CloudConfig = serde_json::from_str(&content)?;
        Ok(config)
    }

    /// Convert cloud app item to new format
    pub fn convert_app_item(&self, cloud_app: &CloudAppItem) -> (String, AppConfig) {
        // Generate a human-readable ID from the app name
        let app_id = self
            .uuid_to_name_map
            .get(&cloud_app.uuid)
            .cloned()
            .unwrap_or_else(|| {
                // Convert name to lowercase and replace spaces with hyphens
                cloud_app.info.name.to_lowercase().replace(' ', "-")
            });

        let mut metadata = HashMap::new();
        metadata.insert("uuid".to_string(), serde_json::json!(cloud_app.uuid));
        metadata.insert(
            "base_hub_uuid".to_string(),
            serde_json::json!(cloud_app.base_hub_uuid),
        );
        metadata.insert("url".to_string(), serde_json::json!(cloud_app.info.url));
        metadata.insert(
            "base_version".to_string(),
            serde_json::json!(cloud_app.base_version),
        );
        metadata.insert(
            "config_version".to_string(),
            serde_json::json!(cloud_app.config_version),
        );

        // Add extra_map fields
        for (key, value) in &cloud_app.info.extra_map {
            metadata.insert(key.clone(), serde_json::json!(value));
        }

        let config = AppConfig {
            name: cloud_app.info.name.clone(),
            metadata,
        };

        (app_id, config)
    }

    /// Convert cloud hub item to new format
    pub fn convert_hub_item(&self, cloud_hub: &CloudHubItem) -> (String, HubConfig) {
        // Generate a human-readable ID from the hub name
        let hub_id = self
            .uuid_to_name_map
            .get(&cloud_hub.uuid)
            .cloned()
            .unwrap_or_else(|| {
                // Convert name to lowercase and replace spaces with hyphens
                cloud_hub.info.hub_name.to_lowercase().replace(' ', "-")
            });

        let mut config_map = HashMap::new();
        config_map.insert("uuid".to_string(), serde_json::json!(cloud_hub.uuid));
        config_map.insert(
            "base_version".to_string(),
            serde_json::json!(cloud_hub.base_version),
        );
        config_map.insert(
            "config_version".to_string(),
            serde_json::json!(cloud_hub.config_version),
        );
        config_map.insert(
            "hub_icon_url".to_string(),
            serde_json::json!(cloud_hub.info.hub_icon_url),
        );
        config_map.insert(
            "target_check_api".to_string(),
            serde_json::json!(cloud_hub.target_check_api),
        );
        config_map.insert(
            "api_keywords".to_string(),
            serde_json::json!(cloud_hub.api_keywords),
        );
        config_map.insert(
            "app_url_templates".to_string(),
            serde_json::json!(cloud_hub.app_url_templates),
        );

        let config = HubConfig {
            name: cloud_hub.info.hub_name.clone(),
            provider_type: hub_id.clone(), // Use hub_id as provider_type for now
            config: config_map,
        };

        (hub_id, config)
    }

    /// Sync cloud configuration to repo directory
    pub async fn sync_to_repo(
        &mut self,
        repo_path: &Path,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let cloud_config = self.fetch_cloud_config().await?;

        // Create directories
        std::fs::create_dir_all(repo_path.join("apps"))?;
        std::fs::create_dir_all(repo_path.join("hubs"))?;

        // Build UUID to name mappings first (for cross-references)
        for hub in &cloud_config.hub_config_list {
            let hub_id = hub.info.hub_name.to_lowercase().replace(' ', "-");
            self.uuid_to_name_map.insert(hub.uuid.clone(), hub_id);
        }

        // Sync hubs
        for hub in &cloud_config.hub_config_list {
            let (hub_id, hub_config) = self.convert_hub_item(hub);
            let hub_path = repo_path.join("hubs").join(format!("{}.json", hub_id));
            let json = serde_json::to_string_pretty(&hub_config)?;
            std::fs::write(hub_path, json)?;
        }

        // Sync apps
        for app in &cloud_config.app_config_list {
            let (app_id, app_config) = self.convert_app_item(app);
            let app_path = repo_path.join("apps").join(format!("{}.json", app_id));
            let json = serde_json::to_string_pretty(&app_config)?;
            std::fs::write(app_path, json)?;
        }

        // Create a mapping file for UUID to human-readable names
        let mapping_path = repo_path.join("uuid_mapping.json");
        let mapping_json = serde_json::to_string_pretty(&self.uuid_to_name_map)?;
        std::fs::write(mapping_path, mapping_json)?;

        Ok(())
    }

    /// Create app identifier from cloud app item
    pub fn create_app_identifier(&self, cloud_app: &CloudAppItem) -> String {
        let app_id = self
            .uuid_to_name_map
            .get(&cloud_app.uuid)
            .cloned()
            .unwrap_or_else(|| cloud_app.info.name.to_lowercase().replace(' ', "-"));

        let hub_id = self
            .uuid_to_name_map
            .get(&cloud_app.base_hub_uuid)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());

        format!("{}::{}", app_id, hub_id)
    }
}

/// Integration with AppRegistry
impl crate::app_registry::AppRegistry {
    /// Sync from cloud configuration
    pub async fn sync_from_cloud(
        &mut self,
        cloud_url: &str,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut cloud_sync = CloudSync::with_url(cloud_url.to_string());
        cloud_sync.sync_to_repo(&self.repo_path).await?;

        // Clear cache to reload configurations
        self.clear_cache();

        Ok(())
    }

    /// Import apps from cloud config and add to tracking
    pub async fn import_cloud_apps(
        &mut self,
        cloud_url: &str,
    ) -> Result<Vec<String>, Box<dyn Error + Send + Sync>> {
        let cloud_sync = CloudSync::with_url(cloud_url.to_string());
        let cloud_config = cloud_sync.fetch_cloud_config().await?;

        let mut imported = Vec::new();

        // First sync the configurations
        self.sync_from_cloud(cloud_url).await?;

        // Then add apps to tracking
        for app in &cloud_config.app_config_list {
            let identifier = cloud_sync.create_app_identifier(app);
            if self.add_app(&identifier).is_ok() {
                imported.push(identifier);
            }
        }

        Ok(imported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cloud_config_parsing() {
        let json = r#"{
            "app_config_list": [
                {
                    "base_version": 2,
                    "config_version": 1,
                    "uuid": "test-uuid",
                    "base_hub_uuid": "hub-uuid",
                    "info": {
                        "name": "Test App",
                        "url": "https://example.com",
                        "extra_map": {
                            "key": "value"
                        }
                    }
                }
            ],
            "hub_config_list": [
                {
                    "base_version": 6,
                    "config_version": 1,
                    "uuid": "hub-uuid",
                    "info": {
                        "hub_name": "Test Hub",
                        "hub_icon_url": ""
                    },
                    "target_check_api": "",
                    "api_keywords": ["owner", "repo"],
                    "app_url_templates": ["https://example.com/%owner/%repo/"]
                }
            ]
        }"#;

        let config: CloudConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.app_config_list.len(), 1);
        assert_eq!(config.hub_config_list.len(), 1);
        assert_eq!(config.app_config_list[0].info.name, "Test App");
        assert_eq!(config.hub_config_list[0].info.hub_name, "Test Hub");
    }

    #[test]
    fn test_cloud_sync_conversion() {
        let cloud_app = CloudAppItem {
            base_version: 2,
            config_version: 1,
            uuid: "test-uuid".to_string(),
            base_hub_uuid: "hub-uuid".to_string(),
            info: CloudAppInfo {
                name: "Test App".to_string(),
                url: "https://example.com".to_string(),
                extra_map: HashMap::from([(
                    "android_app_package".to_string(),
                    "com.example.app".to_string(),
                )]),
            },
        };

        let sync = CloudSync::new();
        let (app_id, app_config) = sync.convert_app_item(&cloud_app);

        assert_eq!(app_id, "test-app");
        assert_eq!(app_config.name, "Test App");
        assert_eq!(app_config.metadata.get("uuid").unwrap(), "test-uuid");
        assert_eq!(
            app_config.metadata.get("android_app_package").unwrap(),
            "com.example.app"
        );
    }
}
