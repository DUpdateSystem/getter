use std::fs::{create_dir_all, File};
use std::io::BufReader;
use std::path::PathBuf;

use crate::error::{Result, GetterError};

use super::super::data::rule_list::RuleList;

pub const WORLD_CONFIG_LIST_NAME: &str = "world_config_list.json";

pub struct WorldList {
    path: PathBuf,
    pub rule_list: RuleList,
}

impl WorldList {
    pub fn load(config_path: &str) -> Result<Self>{
        let path = PathBuf::from(config_path);
        let rule_list = if let Ok(file) = File::open(&path) {
            let reader = BufReader::new(file);
            let rule_list =
                serde_json::from_reader(reader).map_err(|e| GetterError::new("WorldList", "load", Box::new(e)))?;
            rule_list
        } else {
            RuleList::new()
        };
        Ok(Self { path, rule_list })
    }

    pub fn save(&self) -> Result<()> {
        let parent = &self
            .path
            .parent()
            .ok_or_else(|| GetterError::new_nobase("WorldList", "save: get parent dir failed"))?;
        let _ = create_dir_all(parent);
        let file = File::create(&self.path).map_err(|e| GetterError::new("WorldList", "save", Box::new(e)))?;
        serde_json::to_writer(file, &self.rule_list).map_err(|e| GetterError::new("WorldList", "save", Box::new(e)))?;
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
        let path = PathBuf::from(path_base).join(WORLD_CONFIG_LIST_NAME);
        let path_str = path.to_str().unwrap();

        let world_list = WorldList::load(path_str);
        let world_list = world_list.unwrap();
        assert!(world_list.rule_list.app_list.is_empty());
        assert!(world_list.rule_list.hub_list.is_empty());
        let value = world_list.save();
        assert!(value.is_ok());
        let content = fs::read_to_string(&path).expect("test_world_list: read file failed");
        assert!(!content.is_empty());

        let mut world_list = WorldList::load(path_str).unwrap();
        world_list.rule_list.app_list.push("UpgradeAll".to_string());
        world_list.rule_list.hub_list.push("GitHub".to_string());
        let value = world_list.save();
        assert!(value.is_ok());

        let world_list = WorldList::load(path_str).unwrap();
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
        let path_str = path.to_str().unwrap();

        let _ = WorldList::load(path_str);
        assert_eq!(path.try_exists().is_ok_and(|x| x == false), true);
    }
}
