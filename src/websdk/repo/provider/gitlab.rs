use async_trait::async_trait;
use bytes::Bytes;
use serde_json::Value;
use std::collections::HashMap;

use super::super::data::release::*;
use super::base_provider::*;
use markdown::{mdast::Node, to_mdast, ParseOptions};

use crate::utils::{
    http::{get, head, http_status_is_ok},
    versioning::Version,
};

const GITLAB_URL: &str = "https://gitlab.com";
const GITLAB_API_URL: &str = "https://gitlab.com/api/v4/projects";

const VERSION_NUMBER_KEY: &str = "version_number_key";

pub struct GitLabProvider;

impl GitLabProvider {
    pub fn new() -> GitLabProvider {
        GitLabProvider {}
    }
}

impl BaseProviderExt for GitLabProvider {}

impl GitLabProvider {
    async fn get_project_id(&self, fin: &FIn<'_>) -> Option<String> {
        let id_map = fin.data_map.app_data;
        let api_url = format!(
            "{}/{}%2F{}",
            GITLAB_API_URL, id_map["owner"], id_map["repo"]
        );
        let api_url = self.replace_proxy_url(fin, &api_url);

        if let Ok(parsed_url) = api_url.parse() {
            if let Ok(rsp) = get(parsed_url, &HashMap::new()).await {
                if let Some(body) = rsp.body {
                    if let Ok(data) = serde_json::from_slice::<HashMap<String, Value>>(&body) {
                        return Some(data.get("id")?.as_number()?.to_string());
                    }
                }
            }
        }
        None
    }

    fn try_get_download_url_from_changelog(&self, changelog: &str) -> Vec<(String, String)> {
        let changelog_ast = to_mdast(changelog, &ParseOptions::default());
        if changelog_ast.is_err() {
            return vec![];
        }
        let changelog_ast = changelog_ast.unwrap();

        fn get_link_text(nodes: &Vec<Node>) -> Vec<(String, String)> {
            let mut url_map = vec![];
            for node in nodes {
                let mut name = String::new();
                if let Node::Link(link) = node {
                    for c in &link.children {
                        if let Node::Text(text) = c {
                            name.push_str(&text.value);
                        }
                    }
                    url_map.push((name, link.url.to_string()));
                }
                if let Some(child) = node.children() {
                    url_map.extend(get_link_text(child));
                }
            }
            url_map
        }
        if let Some(children) = changelog_ast.children() {
            return get_link_text(children);
        }
        vec![]
    }

    fn fix_download_url(&self, download_url: &str, project_id: &str) -> String {
        if download_url.starts_with("/uploads/") {
            return format!("{}/-/project/{}{}", GITLAB_URL, project_id, download_url);
        }
        download_url.to_string()
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
            "{}/{}%2F{}/releases",
            GITLAB_API_URL, id_map["owner"], id_map["repo"]
        );
        let url = self.replace_proxy_url(fin, &url);
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
                    let changelog = json.get("description")?.as_str()?.to_string();
                    let extra_download_url = self.try_get_download_url_from_changelog(&changelog);
                    let assets_data = assets_data
                        .into_iter()
                        .chain(extra_download_url.into_iter().map(|(k, v)| AssetData {
                            file_name: k,
                            file_type: "".to_string(),
                            download_url: v,
                        }))
                        .collect();
                    Some(ReleaseData {
                        version_number: version_number?.to_string(),
                        changelog,
                        assets: assets_data,
                        extra: None,
                    })
                })
                .collect::<Vec<ReleaseData>>();
            let mut project_id = None;
            for release in release_list.iter_mut() {
                for asset in release.assets.iter_mut() {
                    if asset.download_url.starts_with("/uploads/") {
                        if project_id.is_none() {
                            project_id = self.get_project_id(fin).await;
                        }
                        if let Some(project_id) = &project_id {
                            asset.download_url =
                                self.fix_download_url(&asset.download_url, project_id);
                        }
                    }
                }
            }
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
            .mock("GET", "/fdroid/fdroidclient")
            .with_status(200)
            .create_async()
            .await;

        let id_map = AppDataMap::from([("owner", "fdroid"), ("repo", "fdroidclient")]);
        let proxy_url = format!("{} -> {}", GITLAB_URL, server.url());
        let hub_data = HubDataMap::from([(REVERSE_PROXY, proxy_url.as_str())]);

        let gitlab_provider = GitLabProvider::new();
        assert!(gitlab_provider
            .check_app_available(&FIn::new_with_frag(&id_map, &hub_data, None))
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
        let proxy_url = format!("{} -> {}", GITLAB_API_URL, server.url());
        let hub_data = HubDataMap::from([(REVERSE_PROXY, proxy_url.as_str())]);

        let gitlab_provider = GitLabProvider::new();
        let releases = gitlab_provider
            .get_releases(&FIn::new_with_frag(&id_map, &hub_data, None))
            .await
            .result
            .unwrap();

        let release_json =
            fs::read_to_string("tests/files/data/provider_gitlab_release.json").unwrap();
        let releases_saved = serde_json::from_str::<Vec<ReleaseData>>(&release_json).unwrap();
        assert_eq!(releases, releases_saved)
    }

    #[tokio::test]
    async fn test_try_get_download_url_from_changelog_in_release() {
        let body =
            fs::read_to_string("tests/files/web/gitlab_api_release_AuroraStore.json").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/AuroraOSS%2FAuroraStore/releases")
            .with_status(200)
            .with_body(body)
            .create();
        let project_body =
            fs::read_to_string("tests/files/web/gitlab_api_project_AuroraStore.json").unwrap();
        let _m = server
            .mock("GET", "/AuroraOSS%2FAuroraStore")
            .with_status(200)
            .with_body(project_body)
            .create_async()
            .await;

        let id_map = AppDataMap::from([("owner", "AuroraOSS"), ("repo", "AuroraStore")]);
        let proxy_url = format!("{} -> {}", GITLAB_API_URL, server.url());
        let hub_data = HubDataMap::from([(REVERSE_PROXY, proxy_url.as_str())]);

        let gitlab_provider = GitLabProvider::new();
        let releases = gitlab_provider
            .get_releases(&FIn::new_with_frag(&id_map, &hub_data, None))
            .await
            .result
            .unwrap();

        let release_json =
            fs::read_to_string("tests/files/data/provider_gitlab_release_AuroraStore.json")
                .unwrap();
        let releases_saved = serde_json::from_str::<Vec<ReleaseData>>(&release_json).unwrap();

        assert_eq!(releases, releases_saved)
    }
}
