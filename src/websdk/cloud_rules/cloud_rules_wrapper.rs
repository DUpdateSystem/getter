pub use super::cloud_rules::CloudRules;
use super::data::app_item::AppItem;
use super::data::hub_item::HubItem;

impl CloudRules {
    pub fn get_cloud_app_rules<F>(&self, filter: F) -> Option<AppItem>
    where
        F: Fn(&AppItem) -> bool,
    {
        self._get_cloud_app_rules(filter, true)
            .first()
            .map(|x| x.to_owned())
    }

    pub fn get_cloud_app_rules_list<F>(&self, filter: F) -> Vec<AppItem>
    where
        F: Fn(&AppItem) -> bool,
    {
        self._get_cloud_app_rules(filter, false)
    }

    pub fn _get_cloud_app_rules<F>(&self, filter: F, only_first: bool) -> Vec<AppItem>
    where
        F: Fn(&AppItem) -> bool,
    {
        let mut list = Vec::new();
        for config in self.get_config_list().app_config_list {
            if filter(&config) {
                list.push(config.to_owned());
                if only_first {
                    break;
                }
            }
        }
        list
    }

    pub fn get_cloud_hub_rules<F>(&self, filter: F) -> Option<HubItem>
    where
        F: Fn(&HubItem) -> bool,
    {
        for config in self.get_config_list().hub_config_list {
            if filter(&config) {
                return Some(config.to_owned());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;
    use std::fs;

    #[tokio::test]
    async fn test_get_cloud_app_rules() {
        let json = fs::read_to_string("tests/files/data/UpgradeAll-rules_rules.json").unwrap();
        let path = "/DUpdateSystem/UpgradeAll-rules/master/rules.json";
        let mut server = Server::new_async().await;
        server.mock("GET", path).with_body(json).create();
        let url = server.url() + path;

        let mut cloud_rules = CloudRules::new(&url);
        cloud_rules.renew().await.unwrap();
        let list = cloud_rules.get_cloud_app_rules(|x| x.info.name == "UpgradeAll");
        assert!(list.is_some());
        let list = cloud_rules.get_cloud_app_rules(|x| x.info.name == "");
        assert!(list.is_none());
    }

    #[tokio::test]
    async fn test_get_cloud_app_rules_list() {
        let json = fs::read_to_string("tests/files/data/UpgradeAll-rules_rules.json").unwrap();
        let path = "/DUpdateSystem/UpgradeAll-rules/master/rules.json";
        let mut server = Server::new_async().await;
        server.mock("GET", path).with_body(json).create();
        let url = server.url() + path;

        let mut cloud_rules = CloudRules::new(&url);
        cloud_rules.renew().await.unwrap();
        let list = cloud_rules
            .get_cloud_app_rules_list(|x| x.info.name == "UpgradeAll");
        assert_eq!(list.len(), 1);
        let list = cloud_rules
            .get_cloud_app_rules_list(|x| x.info.name == "");
        assert_eq!(list.len(), 0);
        let list = cloud_rules
            .get_cloud_app_rules_list(|x| x.info.name != "");
        assert_eq!(
            list.len(),
            cloud_rules
                .get_config_list()
                .app_config_list
                .len()
        );
    }

    #[tokio::test]
    async fn test_get_cloud_hub_rules() {
        let json = fs::read_to_string("tests/files/data/UpgradeAll-rules_rules.json").unwrap();
        let path = "/DUpdateSystem/UpgradeAll-rules/master/rules.json";
        let mut server = Server::new_async().await;
        server.mock("GET", path).with_body(json).create();
        let url = server.url() + path;

        let mut cloud_rules = CloudRules::new(&url);
        cloud_rules.renew().await.unwrap();
        let list = cloud_rules
            .get_cloud_hub_rules(|x| x.info.hub_name == "GitHub");
        assert!(list.is_some());
        let list = cloud_rules
            .get_cloud_hub_rules(|x| x.info.hub_name == "");
        assert!(list.is_none());
    }
}
