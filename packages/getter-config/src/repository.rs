use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    pub name: String,
    pub url: Option<String>,
    pub path: PathBuf,
    pub priority: i32,
    pub enabled: bool,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepositoryConfig {
    pub repositories: Vec<Repository>,
}

impl RepositoryConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(path: &Path) -> Result<Self, Box<dyn Error + Send + Sync>> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let content = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn save(&self, path: &Path) -> Result<(), Box<dyn Error + Send + Sync>> {
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)?;
        Ok(())
    }

    pub fn add_repository(&mut self, repo: Repository) {
        self.repositories.push(repo);
        self.sort_by_priority();
    }

    pub fn remove_repository(&mut self, name: &str) -> bool {
        if let Some(pos) = self.repositories.iter().position(|r| r.name == name) {
            self.repositories.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn get_repository(&self, name: &str) -> Option<&Repository> {
        self.repositories.iter().find(|r| r.name == name)
    }

    pub fn get_repository_mut(&mut self, name: &str) -> Option<&mut Repository> {
        self.repositories.iter_mut().find(|r| r.name == name)
    }

    pub fn sort_by_priority(&mut self) {
        self.repositories
            .sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    pub fn get_enabled_repositories(&self) -> Vec<&Repository> {
        self.repositories.iter().filter(|r| r.enabled).collect()
    }
}

pub struct RepositoryManager {
    config: RepositoryConfig,
    config_path: PathBuf,
    data_path: PathBuf,
}

impl RepositoryManager {
    pub fn new(data_path: &Path) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let config_path = data_path.join("repos.conf");
        let config = RepositoryConfig::load(&config_path)?;

        Ok(Self {
            config,
            config_path,
            data_path: data_path.to_path_buf(),
        })
    }

    pub fn init_default_repositories(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        if self.config.repositories.is_empty() {
            let main_repo = Repository {
                name: "getter-main".to_string(),
                url: Some("https://raw.githubusercontent.com/DUpdateSystem/getter/master/cloud_config.json".to_string()),
                path: self.data_path.join("repos/getter-main"),
                priority: 0,
                enabled: true,
                metadata: HashMap::new(),
            };
            self.config.add_repository(main_repo);

            let local_repo = Repository {
                name: "local".to_string(),
                url: None,
                path: self.data_path.join("repos/local"),
                priority: 100,
                enabled: true,
                metadata: HashMap::new(),
            };
            self.config.add_repository(local_repo);

            self.save_config()?;
        }
        Ok(())
    }

    pub fn add_repository(
        &mut self,
        name: String,
        url: Option<String>,
        priority: i32,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let path = self.data_path.join("repos").join(&name);

        let repo = Repository {
            name,
            url,
            path,
            priority,
            enabled: true,
            metadata: HashMap::new(),
        };

        self.config.add_repository(repo);
        self.save_config()?;
        Ok(())
    }

    pub fn remove_repository(&mut self, name: &str) -> Result<bool, Box<dyn Error + Send + Sync>> {
        let removed = self.config.remove_repository(name);
        if removed {
            self.save_config()?;

            let repo_path = self.data_path.join("repos").join(name);
            if repo_path.exists() {
                fs::remove_dir_all(repo_path)?;
            }
        }
        Ok(removed)
    }

    pub fn enable_repository(
        &mut self,
        name: &str,
        enabled: bool,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        if let Some(repo) = self.config.get_repository_mut(name) {
            repo.enabled = enabled;
            self.save_config()?;
        }
        Ok(())
    }

    pub fn set_repository_priority(
        &mut self,
        name: &str,
        priority: i32,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        if let Some(repo) = self.config.get_repository_mut(name) {
            repo.priority = priority;
            self.config.sort_by_priority();
            self.save_config()?;
        }
        Ok(())
    }

    pub async fn sync_repository(&self, name: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
        if let Some(repo) = self.config.get_repository(name) {
            if let Some(url) = &repo.url {
                fs::create_dir_all(&repo.path)?;

                let mut cloud_sync = crate::cloud_sync::CloudSync::with_url(url.clone());
                cloud_sync.sync_to_repo(&repo.path).await?;
            }
        }
        Ok(())
    }

    pub async fn sync_all_repositories(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        for repo in self.config.get_enabled_repositories() {
            if repo.url.is_some() {
                self.sync_repository(&repo.name).await?;
            }
        }
        Ok(())
    }

    pub fn get_repositories(&self) -> &[Repository] {
        &self.config.repositories
    }

    pub fn get_enabled_repositories(&self) -> Vec<&Repository> {
        self.config.get_enabled_repositories()
    }

    fn save_config(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.config.save(&self.config_path)
    }

    pub fn find_app_in_repositories(&self, app_id: &str) -> Vec<(String, PathBuf)> {
        let mut results = Vec::new();

        for repo in self.config.get_enabled_repositories() {
            let app_path = repo.path.join("apps").join(format!("{}.json", app_id));
            if app_path.exists() {
                results.push((repo.name.clone(), app_path));
            }
        }

        results
    }

    pub fn find_hub_in_repositories(&self, hub_id: &str) -> Vec<(String, PathBuf)> {
        let mut results = Vec::new();

        for repo in self.config.get_enabled_repositories() {
            let hub_path = repo.path.join("hubs").join(format!("{}.json", hub_id));
            if hub_path.exists() {
                results.push((repo.name.clone(), hub_path));
            }
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_repository_config() {
        let mut config = RepositoryConfig::new();

        let repo1 = Repository {
            name: "test1".to_string(),
            url: Some("https://example.com/repo1".to_string()),
            path: PathBuf::from("/tmp/repo1"),
            priority: 10,
            enabled: true,
            metadata: HashMap::new(),
        };

        let repo2 = Repository {
            name: "test2".to_string(),
            url: None,
            path: PathBuf::from("/tmp/repo2"),
            priority: 20,
            enabled: false,
            metadata: HashMap::new(),
        };

        config.add_repository(repo1);
        config.add_repository(repo2);

        assert_eq!(config.repositories.len(), 2);
        assert_eq!(config.repositories[0].name, "test2");
        assert_eq!(config.repositories[1].name, "test1");

        let enabled = config.get_enabled_repositories();
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].name, "test1");

        assert!(config.remove_repository("test1"));
        assert_eq!(config.repositories.len(), 1);
    }

    #[test]
    fn test_repository_manager() {
        let temp_dir = TempDir::new().unwrap();
        let mut manager = RepositoryManager::new(temp_dir.path()).unwrap();

        manager.init_default_repositories().unwrap();

        let repos = manager.get_repositories();
        assert_eq!(repos.len(), 2);

        manager
            .add_repository(
                "custom".to_string(),
                Some("https://example.com/custom".to_string()),
                50,
            )
            .unwrap();

        let repos = manager.get_repositories();
        assert_eq!(repos.len(), 3);
        assert_eq!(repos[0].name, "local");
        assert_eq!(repos[1].name, "custom");
        assert_eq!(repos[2].name, "getter-main");

        manager.enable_repository("custom", false).unwrap();
        let enabled = manager.get_enabled_repositories();
        assert_eq!(enabled.len(), 2);

        assert!(manager.remove_repository("custom").unwrap());
        assert_eq!(manager.get_repositories().len(), 2);
    }
}
