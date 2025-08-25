use std::error::Error;
use std::fs::{create_dir_all, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use crate::rule_list::RuleList;

pub const WORLD_CONFIG_LIST_NAME: &str = "world_config_list.json";

pub struct WorldList {
    config_path: Option<PathBuf>,
    pub rule_list: RuleList,
}

impl Default for WorldList {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldList {
    pub fn new() -> Self {
        Self {
            config_path: None,
            rule_list: RuleList::new(),
        }
    }

    pub fn load(&mut self, config_path: &Path) -> Result<&mut Self, Box<dyn Error + Send + Sync>> {
        let rule_list = if let Ok(file) = File::open(config_path) {
            let reader = BufReader::new(file);
            serde_json::from_reader(reader)?
        } else {
            RuleList::new()
        };
        self.config_path = Some(config_path.to_path_buf());
        self.rule_list = rule_list;
        Ok(self)
    }

    pub fn save(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let path = self
            .config_path
            .as_deref()
            .ok_or("WorldList save: path not set")?;
        let parent = path
            .parent()
            .ok_or("WorldList save: get parent dir failed")?;
        let _ = create_dir_all(parent);
        let file = File::create(path)?;
        serde_json::to_writer(file, &self.rule_list)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_world_list() {
        let path_base = "/tmp/getter_test_world_list";
        let _ = fs::remove_dir_all(path_base);
        let config_path = PathBuf::from(path_base).join(WORLD_CONFIG_LIST_NAME);

        let mut world_list = WorldList::new();
        world_list.load(&config_path).unwrap();
        assert!(world_list.rule_list.app_list.is_empty());
        assert!(world_list.rule_list.hub_list.is_empty());
        let value = world_list.save();
        assert!(value.is_ok());
        let content = fs::read_to_string(&config_path).expect("test_world_list: read file failed");
        assert!(!content.is_empty());

        let mut world_list = WorldList::new();
        world_list.load(&config_path).unwrap();
        world_list.rule_list.app_list.push("UpgradeAll".to_string());
        world_list.rule_list.hub_list.push("GitHub".to_string());
        let value = world_list.save();
        assert!(value.is_ok());

        let mut world_list = WorldList::new();
        world_list.load(&config_path).unwrap();
        assert_eq!(world_list.rule_list.app_list.len(), 1);
        assert_eq!(world_list.rule_list.app_list[0], "UpgradeAll");
        assert_eq!(world_list.rule_list.hub_list.len(), 1);
        assert_eq!(world_list.rule_list.hub_list[0], "GitHub");

        fs::remove_dir_all(path_base).expect("test_world_list: clean failed");
    }

    #[test]
    fn test_world_list_only_load() {
        let path_base = "/tmp/getter_test_world_list_only_load";
        let _ = fs::remove_dir(path_base);
        let path = PathBuf::from(path_base).join(WORLD_CONFIG_LIST_NAME);

        let _ = WorldList::new().load(&path);
        assert!(path.try_exists().is_ok_and(|x| !x));
    }
}
