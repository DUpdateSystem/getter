use crate::error::{Result, GetterError};
use crate::utils::json::{string_to_json, json_to_string};
use crate::websdk::cloud_rules::data::app_item::AppItem;
use crate::websdk::cloud_rules::data::hub_item::HubItem;
use crate::websdk::cloud_rules::cloud_rules_wrapper::CloudRules;

use super::world_list::WorldList;
use super::local_repo::LocalRepo;

pub struct WorldConfigWrapper {
    pub world_list: WorldList,
    pub local_repo: LocalRepo,
}

impl WorldConfigWrapper {
    pub fn new(world_config_path: &str, local_repo_path: &str) -> Result<Self> {
        let world_list = WorldList::load(world_config_path)?;
        let local_repo = LocalRepo::new(local_repo_path);
        Ok(WorldConfigWrapper {
            world_list,
            local_repo,
        })
    }

    pub fn get_app_rule(&self, app_name: &str) -> Option<AppItem> {
        if let Ok(content) = self.local_repo.load(app_name) {
            if let Ok(app_item) = string_to_json(&content) {
                return Some(app_item);
            }
        }
        None
    }

    pub fn get_hub_rule(&self, hub_name: &str) -> Option<HubItem> {
        if let Ok(content) = self.local_repo.load(hub_name) {
            if let Ok(hub_item) = string_to_json(&content) {
                return Some(hub_item);
            }
        }
        None
    }

    pub fn download_app_rule(&self, app_name: &str, cloud_rules: &mut CloudRules) -> Result<()> {
        let app_item = cloud_rules.get_cloud_app_rules(|x| x.info.name == app_name);
        let content = json_to_string(&app_item).map_err(|e| GetterError::new("WorldConfigWrapper", "download_app_rule", Box::new(e)))?;
        self.local_repo.save(app_name, &content)?;
        Ok(())
    }

    pub fn download_hub_rule(&self, hub_name: &str, cloud_rules: &mut CloudRules) -> Result<()> {
        let hub_item = cloud_rules.get_cloud_hub_rules(|x| x.info.hub_name == hub_name);
        let content = json_to_string(&hub_item).map_err(|e| GetterError::new("WorldConfigWrapper", "download_hub_rule", Box::new(e)))?;
        self.local_repo.save(hub_name, &content)?;
        Ok(())
    }

    pub fn get_app_rule_list(&self) -> &Vec<String> {
        &self.world_list.rule_list.app_list
    }

    pub fn get_hub_rule_list(&self) -> &Vec<String> {
        &self.world_list.rule_list.hub_list
    }
}
