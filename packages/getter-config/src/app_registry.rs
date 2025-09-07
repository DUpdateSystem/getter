use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub name: String,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubConfig {
    pub name: String,
    pub provider_type: String,
    #[serde(default)]
    pub config: HashMap<String, Value>,
}

#[derive(Debug, Clone)]
pub struct AppIdentifier {
    pub app_id: String,
    pub hub_id: String,
}

impl AppIdentifier {
    pub fn parse(identifier: &str) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let parts: Vec<&str> = identifier.split("::").collect();
        if parts.len() != 2 {
            return Err(format!("Invalid identifier format: {}", identifier).into());
        }
        Ok(Self {
            app_id: parts[0].to_string(),
            hub_id: parts[1].to_string(),
        })
    }

    pub fn to_string(&self) -> String {
        format!("{}::{}", self.app_id, self.hub_id)
    }
}

/// Registry for managing apps and hubs with layered configuration
pub struct AppRegistry {
    pub(crate) repo_path: PathBuf,
    pub(crate) config_path: PathBuf,
    app_list: Vec<String>,
    apps_cache: HashMap<String, AppConfig>,
    hubs_cache: HashMap<String, HubConfig>,
    repository_manager: Option<crate::repository::RepositoryManager>,
}

impl AppRegistry {
    pub fn new(data_path: &Path) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let repo_path = data_path.join("repo");
        let config_path = data_path.join("config");

        // Create directory structure if not exists
        fs::create_dir_all(repo_path.join("apps"))?;
        fs::create_dir_all(repo_path.join("hubs"))?;
        fs::create_dir_all(config_path.join("apps"))?;
        fs::create_dir_all(config_path.join("hubs"))?;

        let repository_manager = crate::repository::RepositoryManager::new(data_path).ok();
        
        let mut registry = Self {
            repo_path,
            config_path,
            app_list: Vec::new(),
            apps_cache: HashMap::new(),
            hubs_cache: HashMap::new(),
            repository_manager,
        };

        registry.load_app_list()?;
        Ok(registry)
    }

    fn load_app_list(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let app_list_path = self.config_path.join("app_list");
        if app_list_path.exists() {
            let content = fs::read_to_string(&app_list_path)?;
            self.app_list = content
                .lines()
                .filter(|line| !line.trim().is_empty() && !line.trim().starts_with('#'))
                .map(|line| line.trim().to_string())
                .collect();
        }
        Ok(())
    }

    pub fn save_app_list(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let app_list_path = self.config_path.join("app_list");
        let content = self.app_list.join("\n");
        fs::write(&app_list_path, content)?;
        Ok(())
    }

    pub fn add_app(&mut self, identifier: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
        let app_id = AppIdentifier::parse(identifier)?;
        
        // Check if app and hub configs exist
        self.get_app_config(&app_id.app_id)?;
        self.get_hub_config(&app_id.hub_id)?;
        
        if !self.app_list.contains(&identifier.to_string()) {
            self.app_list.push(identifier.to_string());
            self.save_app_list()?;
        }
        Ok(())
    }

    pub fn remove_app(&mut self, identifier: &str) -> Result<bool, Box<dyn Error + Send + Sync>> {
        if let Some(pos) = self.app_list.iter().position(|x| x == identifier) {
            self.app_list.remove(pos);
            self.save_app_list()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn list_apps(&self) -> Vec<String> {
        self.app_list.clone()
    }

    pub fn get_app_config(&mut self, app_id: &str) -> Result<AppConfig, Box<dyn Error + Send + Sync>> {
        if let Some(config) = self.apps_cache.get(app_id) {
            return Ok(config.clone());
        }

        let config = self.load_merged_app_config(app_id)?;
        self.apps_cache.insert(app_id.to_string(), config.clone());
        Ok(config)
    }

    pub fn get_hub_config(&mut self, hub_id: &str) -> Result<HubConfig, Box<dyn Error + Send + Sync>> {
        if let Some(config) = self.hubs_cache.get(hub_id) {
            return Ok(config.clone());
        }

        let config = self.load_merged_hub_config(hub_id)?;
        self.hubs_cache.insert(hub_id.to_string(), config.clone());
        Ok(config)
    }

    fn load_merged_app_config(&self, app_id: &str) -> Result<AppConfig, Box<dyn Error + Send + Sync>> {
        let mut base_config: Option<Value> = None;

        // First check if we have repository manager for multi-repo support
        if let Some(ref repo_manager) = self.repository_manager {
            // Load from repositories in priority order
            let repo_configs = repo_manager.find_app_in_repositories(app_id);
            for (_repo_name, config_path) in repo_configs.iter().rev() {
                let content = fs::read_to_string(config_path)?;
                let config: Value = serde_json::from_str(&content)?;
                
                if let Some(base) = base_config {
                    base_config = Some(apply_merge_patch(&base, &config)?);
                } else {
                    base_config = Some(config);
                }
            }
        }
        
        // If no config found via repository manager, try the default repo path
        if base_config.is_none() {
            let repo_path = self.repo_path.join("apps").join(format!("{}.json", app_id));
            if repo_path.exists() {
                let content = fs::read_to_string(&repo_path)?;
                base_config = Some(serde_json::from_str(&content)?);
            }
        }

        // Load and merge local config if exists
        let config_path = self.config_path.join("apps").join(format!("{}.json", app_id));
        if config_path.exists() {
            let content = fs::read_to_string(&config_path)?;
            let local_config: Value = serde_json::from_str(&content)?;
            
            if let Some(base) = base_config {
                base_config = Some(apply_merge_patch(&base, &local_config)?);
            } else {
                base_config = Some(local_config);
            }
        }

        match base_config {
            Some(config) => Ok(serde_json::from_value(config)?),
            None => Err(format!("App configuration not found: {}", app_id).into()),
        }
    }

    fn load_merged_hub_config(&self, hub_id: &str) -> Result<HubConfig, Box<dyn Error + Send + Sync>> {
        let mut base_config: Option<Value> = None;

        // First check if we have repository manager for multi-repo support
        if let Some(ref repo_manager) = self.repository_manager {
            // Load from repositories in priority order
            let repo_configs = repo_manager.find_hub_in_repositories(hub_id);
            for (_repo_name, config_path) in repo_configs.iter().rev() {
                let content = fs::read_to_string(config_path)?;
                let config: Value = serde_json::from_str(&content)?;
                
                if let Some(base) = base_config {
                    base_config = Some(apply_merge_patch(&base, &config)?);
                } else {
                    base_config = Some(config);
                }
            }
        }
        
        // If no config found via repository manager, try the default repo path
        if base_config.is_none() {
            let repo_path = self.repo_path.join("hubs").join(format!("{}.json", hub_id));
            if repo_path.exists() {
                let content = fs::read_to_string(&repo_path)?;
                base_config = Some(serde_json::from_str(&content)?);
            }
        }

        // Load and merge local config if exists
        let config_path = self.config_path.join("hubs").join(format!("{}.json", hub_id));
        if config_path.exists() {
            let content = fs::read_to_string(&config_path)?;
            let local_config: Value = serde_json::from_str(&content)?;
            
            if let Some(base) = base_config {
                base_config = Some(apply_merge_patch(&base, &local_config)?);
            } else {
                base_config = Some(local_config);
            }
        }

        match base_config {
            Some(config) => Ok(serde_json::from_value(config)?),
            None => Err(format!("Hub configuration not found: {}", hub_id).into()),
        }
    }

    pub fn save_app_config(&self, app_id: &str, config: &AppConfig, to_repo: bool) -> Result<(), Box<dyn Error + Send + Sync>> {
        let path = if to_repo {
            self.repo_path.join("apps").join(format!("{}.json", app_id))
        } else {
            self.config_path.join("apps").join(format!("{}.json", app_id))
        };

        let json = serde_json::to_string_pretty(config)?;
        fs::write(&path, json)?;
        Ok(())
    }

    pub fn save_hub_config(&self, hub_id: &str, config: &HubConfig, to_repo: bool) -> Result<(), Box<dyn Error + Send + Sync>> {
        let path = if to_repo {
            self.repo_path.join("hubs").join(format!("{}.json", hub_id))
        } else {
            self.config_path.join("hubs").join(format!("{}.json", hub_id))
        };

        let json = serde_json::to_string_pretty(config)?;
        fs::write(&path, json)?;
        Ok(())
    }

    pub fn get_app_details(&mut self, identifier: &str) -> Result<(AppConfig, HubConfig), Box<dyn Error + Send + Sync>> {
        let app_id = AppIdentifier::parse(identifier)?;
        let app_config = self.get_app_config(&app_id.app_id)?;
        let hub_config = self.get_hub_config(&app_id.hub_id)?;
        Ok((app_config, hub_config))
    }

    pub fn clear_cache(&mut self) {
        self.apps_cache.clear();
        self.hubs_cache.clear();
    }

    pub fn get_repository_manager(&mut self) -> Option<&mut crate::repository::RepositoryManager> {
        self.repository_manager.as_mut()
    }

    pub async fn sync_all_repositories(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        if let Some(ref repo_manager) = self.repository_manager {
            repo_manager.sync_all_repositories().await?;
            self.clear_cache();
        }
        Ok(())
    }

    pub async fn sync_repository(&mut self, name: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
        if let Some(ref repo_manager) = self.repository_manager {
            repo_manager.sync_repository(name).await?;
            self.clear_cache();
        }
        Ok(())
    }

    pub fn list_available_apps(&self) -> Result<Vec<String>, Box<dyn Error + Send + Sync>> {
        let mut apps = HashMap::new();
        
        if let Some(ref repo_manager) = self.repository_manager {
            for repo in repo_manager.get_enabled_repositories() {
                let apps_dir = repo.path.join("apps");
                if apps_dir.exists() {
                    for entry in fs::read_dir(apps_dir)? {
                        let entry = entry?;
                        if let Some(name) = entry.file_name().to_str() {
                            if name.ends_with(".json") {
                                let app_id = name.trim_end_matches(".json");
                                apps.insert(app_id.to_string(), repo.priority);
                            }
                        }
                    }
                }
            }
        } else {
            let apps_dir = self.repo_path.join("apps");
            if apps_dir.exists() {
                for entry in fs::read_dir(apps_dir)? {
                    let entry = entry?;
                    if let Some(name) = entry.file_name().to_str() {
                        if name.ends_with(".json") {
                            let app_id = name.trim_end_matches(".json");
                            apps.insert(app_id.to_string(), 0);
                        }
                    }
                }
            }
        }
        
        let mut app_list: Vec<_> = apps.keys().cloned().collect();
        app_list.sort();
        Ok(app_list)
    }

    pub fn list_available_hubs(&self) -> Result<Vec<String>, Box<dyn Error + Send + Sync>> {
        let mut hubs = HashMap::new();
        
        if let Some(ref repo_manager) = self.repository_manager {
            for repo in repo_manager.get_enabled_repositories() {
                let hubs_dir = repo.path.join("hubs");
                if hubs_dir.exists() {
                    for entry in fs::read_dir(hubs_dir)? {
                        let entry = entry?;
                        if let Some(name) = entry.file_name().to_str() {
                            if name.ends_with(".json") {
                                let hub_id = name.trim_end_matches(".json");
                                hubs.insert(hub_id.to_string(), repo.priority);
                            }
                        }
                    }
                }
            }
        } else {
            let hubs_dir = self.repo_path.join("hubs");
            if hubs_dir.exists() {
                for entry in fs::read_dir(hubs_dir)? {
                    let entry = entry?;
                    if let Some(name) = entry.file_name().to_str() {
                        if name.ends_with(".json") {
                            let hub_id = name.trim_end_matches(".json");
                            hubs.insert(hub_id.to_string(), 0);
                        }
                    }
                }
            }
        }
        
        let mut hub_list: Vec<_> = hubs.keys().cloned().collect();
        hub_list.sort();
        Ok(hub_list)
    }
}

/// Apply JSON Merge Patch (RFC 7386) using json-patch library
fn apply_merge_patch(base: &Value, patch: &Value) -> Result<Value, Box<dyn Error + Send + Sync>> {
    // json-patch::merge modifies the base in place, so we need to clone
    let mut result = base.clone();
    json_patch::merge(&mut result, patch);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_app_identifier_parse() {
        let id = AppIdentifier::parse("rust::github").unwrap();
        assert_eq!(id.app_id, "rust");
        assert_eq!(id.hub_id, "github");
        assert_eq!(id.to_string(), "rust::github");

        assert!(AppIdentifier::parse("invalid").is_err());
        assert!(AppIdentifier::parse("too::many::parts").is_err());
    }

    #[test]
    fn test_merge_patch() {
        let base = serde_json::json!({
            "name": "test",
            "version": "1.0",
            "config": {
                "timeout": 30,
                "retry": 3
            }
        });

        let patch = serde_json::json!({
            "version": "2.0",
            "config": {
                "timeout": 60,
                "new_field": true
            },
            "extra": "data"
        });

        let result = apply_merge_patch(&base, &patch).unwrap();
        assert_eq!(result["name"], "test");
        assert_eq!(result["version"], "2.0");
        assert_eq!(result["config"]["timeout"], 60);
        assert_eq!(result["config"]["retry"], 3);
        assert_eq!(result["config"]["new_field"], true);
        assert_eq!(result["extra"], "data");
        
        // Test null removal
        let patch_with_null = serde_json::json!({
            "name": null,
            "version": "3.0"
        });
        
        let result2 = apply_merge_patch(&result, &patch_with_null).unwrap();
        assert_eq!(result2.get("name"), None); // name should be removed
        assert_eq!(result2["version"], "3.0");
    }

    #[test]
    fn test_app_registry() {
        let temp_dir = TempDir::new().unwrap();
        let data_path = temp_dir.path();
        
        // Create necessary directories
        fs::create_dir_all(data_path.join("repo/apps")).unwrap();
        fs::create_dir_all(data_path.join("repo/hubs")).unwrap();
        fs::create_dir_all(data_path.join("config/apps")).unwrap();
        fs::create_dir_all(data_path.join("config/hubs")).unwrap();
        
        let mut registry = AppRegistry::new(data_path).unwrap();

        // Create test app config
        let app_config = AppConfig {
            name: "rust".to_string(),
            metadata: HashMap::from([
                ("repo".to_string(), serde_json::json!("rust-lang/rust")),
            ]),
        };

        // Create test hub config
        let hub_config = HubConfig {
            name: "github".to_string(),
            provider_type: "github".to_string(),
            config: HashMap::new(),
        };

        // Save configs to repo (true means save to repo)
        registry.save_app_config("rust", &app_config, true).unwrap();
        registry.save_hub_config("github", &hub_config, true).unwrap();

        // Clear cache to ensure configs are reloaded
        registry.clear_cache();

        // Add app
        registry.add_app("rust::github").unwrap();
        assert_eq!(registry.list_apps(), vec!["rust::github"]);

        // Get app details
        let (app, hub) = registry.get_app_details("rust::github").unwrap();
        assert_eq!(app.name, "rust");
        assert_eq!(hub.name, "github");

        // Remove app
        assert!(registry.remove_app("rust::github").unwrap());
        assert!(registry.list_apps().is_empty());
    }
}