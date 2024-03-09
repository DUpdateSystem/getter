use async_trait::async_trait;
use bytes::Bytes;
use serde_json::Value;
use std::collections::HashMap;

use super::super::data::release::*;
use super::base_provider::*;

use crate::utils::http::{get, head, http_status_is_ok};

static GITLAB_URL: &str = "https://gitlab.com";
static GITLAB_API_URL: &str = "https://gitlab.com/api/v4/projects";

pub struct GitLabProvider {
    url_proxy_map: HashMap<String, String>,
}

impl GitLabProvider {
    pub fn new(url_proxy_map: HashMap<String, String>) -> GitLabProvider {
        GitLabProvider { url_proxy_map }
    }
}

impl BaseProviderExt for GitLabProvider {
    fn url_proxy_map(&self) -> &HashMap<String, String> {
        &self.url_proxy_map
    }
}

#[async_trait]
impl BaseProvider for GitLabProvider {
    fn get_cache_request_key(
        &self,
        function_type: &FunctionType,
        data_map: &DataMap,
    ) -> Vec<String> {
        let id_map = data_map.app_data;
        match function_type {
            FunctionType::CheckAppAvailable => vec![format!(
                "{}/{}/{}/HEAD",
                GITLAB_URL, id_map["owner"], id_map["repo"]
            )],
            FunctionType::GetLatestRelease | FunctionType::GetReleases => vec![format!(
                "{}/{}/{}/releases",
                GITLAB_API_URL, id_map["owner"], id_map["repo"]
            )],
        }
    }

    async fn check_app_available(&self, fin: &FIn) -> FOut<bool> {
        let id_map = fin.data_map.app_data;
        let api_url = format!("{}/{}/{}", GITLAB_URL, id_map["owner"], id_map["repo"]);
        let api_url = self.replace_proxy_url(&api_url);

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
            "{}/{}%2F{}/releases",
            GITLAB_API_URL, id_map["owner"], id_map["repo"]
        );
        let url = self.replace_proxy_url(&url);
        let mut fout = FOut::new_empty();
        let cache_body = fin.get_cache(&url);
        let mut rsp_body = None;
        if cache_body.is_none() {
            if let Ok(parsed_url) = url.parse() {
                let header_map = {
                    let mut map = HashMap::new();
                    map.insert("User-Agent".to_string(), "Awesome-Octocat-App".to_string());
                    map
                };
                if let Ok(rsp) = get(parsed_url, &header_map).await {
                    println!("{:?}", rsp);
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
            println!("{:?}", data);
            let release_list = data
                .iter()
                .filter_map(|json| {
                    let assets_data = match json.get("assets")?.get("links") {
                        Some(links) => links
                            .as_array()?
                            .iter()
                            .filter_map(|asset| {
                                let file_name = asset.get("name")?.as_str()?.to_string();
                                let file_type = asset.get("link_type")?.as_str()?.to_string();
                                let download_url = asset.get("url")?.as_str()?.to_string();
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
                    let changelog = json.get("description")?.as_str()?.to_string();
                    Some(ReleaseData {
                        version_number,
                        changelog,
                        assets: assets_data,
                        extra: None,
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
    use std::collections::HashMap;
    use std::fs;

    #[tokio::test]
    async fn test_check_app_available() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/fdroid/fdroidclient")
            .with_status(200)
            .create_async()
            .await;

        let id_map = AppDataMap::from([("owner", "fdroid"), ("repo", "fdroidclient")]);

        let github_provider =
            GitLabProvider::new(HashMap::from([(GITLAB_URL.to_string(), server.url())]));

        assert!(github_provider
            .check_app_available(&FIn::new_with_frag(&id_map, &HubDataMap::new(), None))
            .await
            .result
            .unwrap());
    }

    #[tokio::test]
    async fn test_get_releases() {
        let body = fs::read_to_string("tests/files/web/gitlab_api_release.json").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/fdroid%2Ffdroidclient/releases")
            .with_status(200)
            .with_body(body)
            .create();

        let id_map = AppDataMap::from([("owner", "fdroid"), ("repo", "fdroidclient")]);

        let github_provider =
            GitLabProvider::new(HashMap::from([(GITLAB_API_URL.to_string(), server.url())]));

        let releases = github_provider
            .get_releases(&FIn::new_with_frag(&id_map, &HubDataMap::new(), None))
            .await
            .result
            .unwrap();

        let release_json =
            fs::read_to_string("tests/files/data/provider_gitlab_release.json").unwrap();
        let releases_saved = serde_json::from_str::<Vec<ReleaseData>>(&release_json).unwrap();
        assert_eq!(releases, releases_saved)
    }
}
