use crate::data::release::*;
use crate::provider::base_provider::{BaseProvider, IdMap};
use crate::utils::http::{get, head, http_status_is_ok};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;

pub struct GithubProvider {
    url_proxy_map: HashMap<String, String>,
}

impl GithubProvider {
    fn replace_proxy_url(&self, url: &str) -> String {
        let mut url = url.to_string();
        for (url_prefix, proxy_url) in &self.url_proxy_map {
            url = url.replace(url_prefix, proxy_url);
        }
        url
    }
}

#[async_trait]
impl BaseProvider for GithubProvider {
    async fn check_app_available(&self, id_map: &IdMap) -> Option<bool> {
        let api_url = format!("https://github.com/{}/{}", id_map["owner"], id_map["repo"]);
        let api_url = self.replace_proxy_url(&api_url);

        if let Ok(parsed_url) = api_url.parse() {
            if let Ok(rsp) = head(parsed_url, &HashMap::new()).await {
                return Some(http_status_is_ok(rsp.status));
            }
        }
        None
    }

    async fn get_releases(&self, id_map: &IdMap) -> Option<Vec<ReleaseData>> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/releases",
            id_map["owner"], id_map["repo"]
        );
        let url = self.replace_proxy_url(&url);
        if let Ok(parsed_uri) = url.parse() {
            let header_map = {
                let mut map = HashMap::new();
                map.insert("User-Agent".to_string(), "Awesome-Octocat-App".to_string());
                map
            };
            if let Ok(rsp) = get(parsed_uri, &header_map).await {
                if let Ok(data) = serde_json::from_slice::<Vec<Value>>(&rsp.body.unwrap()) {
                    return Some(
                        data.iter()
                            .filter_map(|json| {
                                let assets_data = match json.get("assets") {
                                    Some(assets) => assets
                                        .as_array()?
                                        .iter()
                                        .filter_map(|asset| {
                                            let file_name =
                                                asset.get("name")?.as_str()?.to_string();
                                            let file_type =
                                                asset.get("content_type")?.as_str()?.to_string();
                                            let download_url = asset
                                                .get("browser_download_url")?
                                                .as_str()?
                                                .to_string();
                                            Some(AssetData {
                                                file_name,
                                                file_type,
                                                download_url,
                                            })
                                        })
                                        .collect(),
                                    None => vec![],
                                };
                                let version_number = json.get("name")?.as_str()?.to_string();
                                let changelog = json.get("body")?.as_str()?.to_string();
                                Some(ReleaseData {
                                    version_number,
                                    changelog,
                                    assets: assets_data,
                                    extra: None,
                                })
                            })
                            .collect(),
                    );
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::base_provider::BaseProvider;
    use mockito::Server;
    use std::collections::HashMap;
    use std::fs;

    #[tokio::test]
    async fn test_check_app_available() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/DUpdateSystem/UpgradeAll")
            .with_status(200)
            .create_async()
            .await;

        let id_map = {
            let mut map = HashMap::new();
            map.insert("owner".to_string(), "DUpdateSystem".to_string());
            map.insert("repo".to_string(), "UpgradeAll".to_string());
            map
        };

        let url_proxy_map = {
            let mut map = HashMap::new();
            map.insert("https://github.com".to_string(), server.url());
            map
        };

        let github_provider = GithubProvider { url_proxy_map };

        assert!(github_provider.check_app_available(&id_map).await.unwrap());
    }

    #[tokio::test]
    async fn test_get_releases() {
        let body = fs::read_to_string("tests/files/web/github_api_release.json").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/DUpdateSystem/UpgradeAll/releases")
            .with_status(200)
            .with_body(body)
            .create();

        let id_map = {
            let mut map = HashMap::new();
            map.insert("owner".to_string(), "DUpdateSystem".to_string());
            map.insert("repo".to_string(), "UpgradeAll".to_string());
            map
        };

        let url_proxy_map = {
            let mut map = HashMap::new();
            map.insert("https://api.github.com".to_string(), server.url());
            map
        };

        let github_provider = GithubProvider { url_proxy_map };

        let releases = github_provider.get_releases(&id_map).await.unwrap();

        let release_json =
            fs::read_to_string("tests/files/data/provider_github_release.json").unwrap();
        let releases_saved = serde_json::from_str::<Vec<ReleaseData>>(&release_json).unwrap();
        assert_eq!(releases, releases_saved)
    }
}
