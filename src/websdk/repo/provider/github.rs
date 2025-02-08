use async_trait::async_trait;
use bytes::Bytes;
use serde_json::Value;
use std::collections::HashMap;

use super::super::data::release::*;
use super::base_provider::*;

use crate::utils::{
    http::{get, head, http_status_is_ok},
    versioning::Version,
};

pub const GITHUB_API_URL: &str = "https://api.github.com";
const GITHUB_URL: &str = "https://github.com";

const VERSION_NUMBER_KEY: &str = "version_number_key";
const VERSION_CODE_KEY: &str = "version_code_key";

pub struct GitHubProvider;

impl GitHubProvider {
    pub fn new() -> Self {
        GitHubProvider {}
    }

    fn get_token_header(&self, fin: &FIn) -> HashMap<String, String> {
        let mut map = HashMap::new();
        // check if token is empty or blank
        let token = fin
            .data_map
            .hub_data
            .get("token")
            .filter(|t| !t.trim().is_empty())
            .or_else(|| fin.data_map.app_data.get("token"));
        if let Some(token) = token {
            map.insert("Authorization".to_string(), format!("Bearer {}", token));
        }
        map
    }
}

impl BaseProviderExt for GitHubProvider {}

#[async_trait]
impl BaseProvider for GitHubProvider {
    fn get_cache_request_key(
        &self,
        function_type: &FunctionType,
        data_map: &DataMap,
    ) -> Vec<String> {
        let id_map = data_map.app_data;
        match function_type {
            FunctionType::CheckAppAvailable => vec![format!(
                "{}/{}/{}/HEAD",
                GITHUB_URL, id_map["owner"], id_map["repo"]
            )],
            FunctionType::GetLatestRelease | FunctionType::GetReleases => vec![format!(
                "{}/repos/{}/{}/releases",
                GITHUB_API_URL, id_map["owner"], id_map["repo"]
            )],
        }
    }

    async fn check_app_available(&self, fin: &FIn) -> FOut<bool> {
        let id_map = fin.data_map.app_data;
        let api_url = format!("{}/{}/{}", GITHUB_URL, id_map["owner"], id_map["repo"]);
        let api_url = self.replace_proxy_url(fin, &api_url);

        if let Ok(parsed_url) = api_url.parse() {
            if let Ok(rsp) = head(parsed_url, &HashMap::new()).await {
                return FOut::new(http_status_is_ok(rsp.status));
            }
        }
        FOut::new_empty()
    }

    async fn get_releases(&self, fin: &FIn) -> FOut<Vec<ReleaseData>> {
        let id_map = fin.data_map.app_data;
        let url = format!(
            "{}/repos/{}/{}/releases",
            GITHUB_API_URL, id_map["owner"], id_map["repo"]
        );
        let url = self.replace_proxy_url(fin, &url);
        let mut fout = FOut::new_empty();
        let cache_body = fin.get_cache(&url);
        let mut rsp_body = None;
        if cache_body.is_none() {
            if let Ok(parsed_url) = url.parse() {
                let header_map = {
                    let mut map = HashMap::new();
                    map.insert("User-Agent".to_string(), "UpgradeAll-App".to_string());
                    let token_map = self.get_token_header(fin);
                    map.extend(token_map);
                    map
                };
                if let Ok(rsp) = get(parsed_url, &header_map).await {
                    if let Some(content) = rsp.body {
                        rsp_body = Some(content);
                    }
                }
            }
        }

        let body: &Bytes;
        if let Some(ref content) = rsp_body {
            body = content;
        } else if let Some(content) = cache_body {
            body = content;
        } else {
            return fout;
        }

        if let Ok(data) = serde_json::from_slice::<Vec<Value>>(body) {
            let release_list = data
                .iter()
                .filter_map(|json| {
                    let assets_data = match json.get("assets") {
                        Some(assets) => assets
                            .as_array()?
                            .iter()
                            .filter_map(|asset| {
                                let file_name = asset.get("name")?.as_str()?.to_string();
                                let file_type = asset.get("content_type")?.as_str()?.to_string();
                                let download_url =
                                    asset.get("browser_download_url")?.as_str()?.to_string();
                                Some(AssetData {
                                    file_name,
                                    file_type,
                                    download_url,
                                })
                            })
                            .collect(),
                        None => vec![],
                    };
                    let mut keys_to_try = vec!["name", "tag_name"];
                    if let Some(tag) = fin.data_map.hub_data.get(VERSION_NUMBER_KEY) {
                        keys_to_try.insert(0, tag);
                    }
                    let mut version_number: Option<String> = None;

                    for key in keys_to_try.iter() {
                        if let Some(value) = json.get(key).and_then(|v| v.as_str()) {
                            if Version::new(value.to_string()).is_valid() {
                                version_number = Some(value.to_string());
                                break;
                            }
                        }
                    }
                    let changelog = json.get("body")?.as_str()?.to_string();

                    let mut extra = None;
                    if let Some(tag) = fin.data_map.hub_data.get(VERSION_CODE_KEY) {
                        if let Some(value) = json.get(tag) {
                            extra = Some(HashMap::from([(tag.to_string(), value.to_string())]));
                        }
                    }
                    Some(ReleaseData {
                        version_number: version_number?.to_string(),
                        changelog,
                        assets: assets_data,
                        extra,
                    })
                })
                .collect::<Vec<ReleaseData>>();
            fout = fout.set_data(release_list);
        };

        if let Some(content) = rsp_body {
            fout.set_cached_map(HashMap::from([(url, content)]))
        } else {
            fout
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;
    use std::fs;

    #[tokio::test]
    async fn test_check_app_available() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/DUpdateSystem/UpgradeAll")
            .with_status(200)
            .create_async()
            .await;

        let id_map = AppDataMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
        let proxy_url = format!("{} -> {}", GITHUB_URL, server.url());
        let hub_data = HubDataMap::from([(REVERSE_PROXY, proxy_url.as_str())]);

        let github_provider = GitHubProvider::new();
        assert!(github_provider
            .check_app_available(&FIn::new_with_frag(&id_map, &hub_data, None))
            .await
            .result
            .unwrap());
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

        let id_map = AppDataMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
        let proxy_url = format!("{} -> {}", GITHUB_API_URL, server.url());
        let hub_data = HubDataMap::from([(REVERSE_PROXY, proxy_url.as_str())]);

        let github_provider = GitHubProvider::new();
        let releases = github_provider
            .get_releases(&FIn::new_with_frag(&id_map, &hub_data, None))
            .await
            .result
            .unwrap();

        let release_json =
            fs::read_to_string("tests/files/data/provider_github_release.json").unwrap();
        let releases_saved = serde_json::from_str::<Vec<ReleaseData>>(&release_json).unwrap();
        assert_eq!(releases, releases_saved)
    }

    #[tokio::test]
    async fn test_get_releases_token() {
        let mut id_map = AppDataMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
        let test_token = std::env::var("GITHUB_TOKEN");
        if test_token.is_err() {
            return;
        }
        let test_token = test_token.unwrap();
        let hub_data = HubDataMap::from([("token", test_token.as_str())]);

        let github_provider = GitHubProvider::new();
        let releases = github_provider
            .get_releases(&FIn::new_with_frag(&id_map, &hub_data, None))
            .await
            .result
            .unwrap();

        assert!(!releases.is_empty());

        let hub_data = HubDataMap::from([("token", " ")]);
        id_map.insert("token", &test_token);

        let releases = github_provider
            .get_releases(&FIn::new_with_frag(&id_map, &hub_data, None))
            .await
            .result
            .unwrap();

        assert!(!releases.is_empty());
    }
}
