use crate::app_registry::{AppConfig, AppIdentifier, AppRegistry, HubConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// App tracking information stored alongside configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppTrackingInfo {
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub last_checked: Option<u64>,
    pub added_at: u64,
}

/// Layered configuration manager combining registry with tracking
pub struct LayeredConfig {
    registry: AppRegistry,
    tracking: HashMap<String, AppTrackingInfo>,
    data_path: PathBuf,
}

impl LayeredConfig {
    pub fn new(data_path: &Path) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let registry = AppRegistry::new(data_path)?;
        let tracking = Self::load_tracking(data_path)?;

        Ok(Self {
            registry,
            tracking,
            data_path: data_path.to_path_buf(),
        })
    }

    fn load_tracking(
        data_path: &Path,
    ) -> Result<HashMap<String, AppTrackingInfo>, Box<dyn Error + Send + Sync>> {
        let tracking_path = data_path.join("config").join("tracking.json");
        if tracking_path.exists() {
            let content = std::fs::read_to_string(&tracking_path)?;
            Ok(serde_json::from_str(&content)?)
        } else {
            Ok(HashMap::new())
        }
    }

    fn save_tracking(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let tracking_path = self.data_path.join("config").join("tracking.json");
        let json = serde_json::to_string_pretty(&self.tracking)?;
        std::fs::write(&tracking_path, json)?;
        Ok(())
    }

    pub fn add_tracked_app(
        &mut self,
        identifier: &str,
        app_config: Option<AppConfig>,
        hub_config: Option<HubConfig>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let app_id = AppIdentifier::parse(identifier)?;

        // Save configs if provided
        if let Some(config) = app_config {
            self.registry
                .save_app_config(&app_id.app_id, &config, false)?;
        }

        if let Some(config) = hub_config {
            self.registry
                .save_hub_config(&app_id.hub_id, &config, false)?;
        }

        // Add to registry
        self.registry.add_app(identifier)?;

        // Add tracking info
        if !self.tracking.contains_key(identifier) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();

            self.tracking.insert(
                identifier.to_string(),
                AppTrackingInfo {
                    current_version: None,
                    latest_version: None,
                    last_checked: None,
                    added_at: now,
                },
            );
            self.save_tracking()?;
        }

        Ok(())
    }

    pub fn remove_tracked_app(
        &mut self,
        identifier: &str,
    ) -> Result<bool, Box<dyn Error + Send + Sync>> {
        let removed = self.registry.remove_app(identifier)?;
        if removed {
            self.tracking.remove(identifier);
            self.save_tracking()?;
        }
        Ok(removed)
    }

    pub fn list_tracked_apps(&self) -> Vec<String> {
        self.registry.list_apps()
    }

    pub fn get_app_details(
        &mut self,
        identifier: &str,
    ) -> Result<(AppConfig, HubConfig), Box<dyn Error + Send + Sync>> {
        self.registry.get_app_details(identifier)
    }

    pub fn get_tracking_info(&self, identifier: &str) -> Option<&AppTrackingInfo> {
        self.tracking.get(identifier)
    }

    pub fn update_version(
        &mut self,
        identifier: &str,
        current: Option<String>,
        latest: Option<String>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        if let Some(info) = self.tracking.get_mut(identifier) {
            if current.is_some() {
                info.current_version = current;
            }
            if latest.is_some() {
                info.latest_version = latest;
            }
            info.last_checked = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            );
            self.save_tracking()?;
        }
        Ok(())
    }

    pub fn get_outdated_apps(&self) -> Vec<(String, AppTrackingInfo)> {
        self.tracking
            .iter()
            .filter(|(_, info)| {
                matches!((&info.current_version, &info.latest_version), (Some(current), Some(latest)) if current != latest)
            })
            .map(|(id, info)| (id.clone(), info.clone()))
            .collect()
    }

    pub fn clear_cache(&mut self) {
        self.registry.clear_cache();
    }
}

/// Global instance management
static INSTANCE: once_cell::sync::OnceCell<Arc<Mutex<LayeredConfig>>> =
    once_cell::sync::OnceCell::new();

pub async fn init_layered_config(data_path: &Path) -> Result<(), Box<dyn Error + Send + Sync>> {
    let config = LayeredConfig::new(data_path)?;
    INSTANCE
        .set(Arc::new(Mutex::new(config)))
        .map_err(|_| "LayeredConfig already initialized")?;
    Ok(())
}

pub async fn get_layered_config() -> Arc<Mutex<LayeredConfig>> {
    INSTANCE
        .get()
        .expect("LayeredConfig not initialized")
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_layered_config() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = LayeredConfig::new(temp_dir.path()).unwrap();

        // Create test configs
        let app_config = AppConfig {
            name: "firefox".to_string(),
            metadata: HashMap::from([("repo".to_string(), serde_json::json!("mozilla/firefox"))]),
        };

        let hub_config = HubConfig {
            name: "mozilla".to_string(),
            provider_type: "github".to_string(),
            config: HashMap::new(),
        };

        // Add tracked app
        config
            .add_tracked_app("firefox::mozilla", Some(app_config), Some(hub_config))
            .unwrap();

        // Check app is tracked
        assert_eq!(config.list_tracked_apps(), vec!["firefox::mozilla"]);

        // Update version
        config
            .update_version(
                "firefox::mozilla",
                Some("100.0".to_string()),
                Some("101.0".to_string()),
            )
            .unwrap();

        // Check outdated apps
        let outdated = config.get_outdated_apps();
        assert_eq!(outdated.len(), 1);
        assert_eq!(outdated[0].0, "firefox::mozilla");

        // Remove app
        assert!(config.remove_tracked_app("firefox::mozilla").unwrap());
        assert!(config.list_tracked_apps().is_empty());
    }
}
