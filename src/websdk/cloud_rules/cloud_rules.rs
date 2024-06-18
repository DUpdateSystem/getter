use hyper::Uri;
use std::collections::HashMap;

use super::data::config_list::{ConfigList, ConfigListViewer};
use crate::utils::http::get;

pub struct CloudRules {
    pub api_url: String,

    _config_list: Option<ConfigList>,
}

#[derive(Debug)]
pub struct DownloadError {
    pub url: Uri,
    pub error: Box<dyn std::error::Error + Send + Sync>,
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "DownloadError: {} ({})", self.error, self.url)
    }
}

impl CloudRules {
    pub fn new(api_url: &str) -> CloudRules {
        CloudRules {
            api_url: api_url.to_string(),
            _config_list: None,
        }
    }

    pub fn get_config_list(&self) -> ConfigListViewer {
        self._config_list.as_ref().map_or_else(
            || ConfigListViewer::default(),
            |config_list| config_list.viewer(),
        )
    }

    pub async fn renew(&mut self) -> Result<(), DownloadError> {
        let config_list = self.download_config_list(&self.api_url).await?;
        self._config_list = Some(config_list);
        Ok(())
    }

    async fn download_config_list(&self, url: &str) -> Result<ConfigList, DownloadError> {
        Self::_download_config_list_impl(url)
            .await
            .map_err(|e| DownloadError {
                url: url.parse().unwrap(),
                error: e,
            })
    }
    async fn _download_config_list_impl(
        url: &str,
    ) -> Result<ConfigList, Box<dyn std::error::Error + Send + Sync>> {
        let map = HashMap::new();
        let resp = get(url.parse()?, &map).await?;
        if let Some(body) = resp.body {
            let config_list: ConfigList = serde_json::from_slice(&body)?;
            Ok(config_list)
        } else {
            Err("No body".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;
    use std::fs;

    #[tokio::test]
    async fn test_download_config_list() {
        let json = fs::read_to_string("tests/files/data/UpgradeAll-rules_rules.json").unwrap();
        let path = "/DUpdateSystem/UpgradeAll-rules/master/rules.json";
        let mut server = Server::new_async().await;
        server.mock("GET", path).with_body(json).create();
        let url = server.url() + path;

        let mut cloud_rules = CloudRules::new(&url);
        cloud_rules.renew().await.unwrap();
        let config_list = cloud_rules.get_config_list();
        assert_eq!(config_list.app_config_list.len(), 219);
        assert_eq!(config_list.app_config_list[0].info.name, "UpgradeAll");
        assert_eq!(
            config_list.app_config_list.last().unwrap().info.name,
            "黑阈"
        );
        assert_eq!(config_list.hub_config_list.len(), 11);
        assert_eq!(config_list.hub_config_list[0].info.hub_name, "GitHub");
        assert_eq!(
            config_list.hub_config_list.last().unwrap().info.hub_name,
            "Xposed Module Repository"
        );
    }

    #[tokio::test]
    async fn test_download_config_list_invalid() {
        let path = "/DUpdateSystem/UpgradeAll-rules/master/rules.json";
        let mut server = Server::new_async().await;
        server.mock("GET", path).with_status(404).create();
        let url = server.url() + path;

        let mut cloud_rules = CloudRules::new(&url);
        let result = cloud_rules.renew().await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().url.to_string(), url);
    }
}
