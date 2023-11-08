use crate::data::release::*;
use crate::provider::base_provider::*;
use crate::utils::http::{get, head, http_status_is_ok};
use async_trait::async_trait;
use bytes::Bytes;
use serde_json::Value;
use std::collections::HashMap;

pub struct GithubProvider {
    url_proxy_map: HashMap<String, String>,
}

impl GithubProvider {
    pub fn new(url_proxy_map: HashMap<String, String>) -> GithubProvider {
        GithubProvider { url_proxy_map }
    }

    fn replace_proxy_url(&self, url: &str) -> String {
        let mut url = url.to_string();
        for (url_prefix, proxy_url) in &self.url_proxy_map {
            url = url.replace(url_prefix, proxy_url);
        }
        url
    }
}

static GITHUB_API_URL: &str = "https://api.github.com";

#[async_trait]
impl BaseProvider for GithubProvider {
    fn get_cache_request_key(&self, function_type: FunctionType, id_map: IdMap) -> Vec<String> {
        match function_type {
            FunctionType::CheckAppAvailable => vec![],
            FunctionType::GetLatestRelease | FunctionType::GetReleases => vec![format!(
                "{}/repos/{}/{}/releases",
                GITHUB_API_URL, id_map["owner"], id_map["repo"]
            )],
        }
    }

    async fn check_app_available(&self, fin: &FIn) -> FOut<bool> {
        let id_map = fin.id_map;
        let api_url = format!("https://github.com/{}/{}", id_map["owner"], id_map["repo"]);
        let api_url = self.replace_proxy_url(&api_url);

        if let Ok(parsed_url) = api_url.parse() {
            if let Ok(rsp) = head(parsed_url, &HashMap::new()).await {
                return FOut::new(http_status_is_ok(rsp.status));
            }
        }
        FOut::new_empty()
    }

    async fn get_releases(&self, fin: &FIn) -> FOut<Vec<ReleaseData>> {
        let id_map = fin.id_map;
        let cache_map = fin.cache_map;
        let url = format!(
            "https://api.github.com/repos/{}/{}/releases",
            id_map["owner"], id_map["repo"]
        );
        let url = self.replace_proxy_url(&url);
        let mut fout = FOut::new_empty();
        let cache_body = cache_map.get(&url);
        let mut rsp_body = None;
        if cache_body.is_none() {
            if let Ok(parsed_url) = url.parse() {
                let header_map = {
                    let mut map = HashMap::new();
                    map.insert("User-Agent".to_string(), "Awesome-Octocat-App".to_string());
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
            let mut release_list = data
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
                    let version_number = json.get("name")?.as_str()?.to_string();
                    let changelog = json.get("body")?.as_str()?.to_string();
                    Some(ReleaseData {
                        version_number,
                        changelog,
                        assets: assets_data,
                        extra: None,
                    })
                })
                .collect::<Vec<ReleaseData>>();
            release_list.reverse();
            fout = fout.set_data(release_list);
        };

        if let Some(content) = rsp_body {
            fout.set_cached_map(CacheMap::new().set(url, content))
        } else {
            fout
        }
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

        assert!(github_provider
            .check_app_available(FIn::new(&id_map))
            .await
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

        let releases = github_provider
            .get_releases(FIn::new(&id_map))
            .await
            .unwrap();

        let release_json =
            fs::read_to_string("tests/files/data/provider_github_release.json").unwrap();
        let releases_saved = serde_json::from_str::<Vec<ReleaseData>>(&release_json).unwrap();
        assert_eq!(releases, releases_saved)
    }
}
