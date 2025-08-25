use async_trait::async_trait;
use bytes::Bytes;
use serde_json::Value;
use std::collections::HashMap;

use crate::base_provider::*;
use crate::data::{AssetData, ReleaseData};

use getter_utils::{
    http::{get, head, http_status_is_ok},
    versioning::Version,
};

pub const GITHUB_API_URL: &str = "https://api.github.com";
const GITHUB_URL: &str = "https://github.com";

const VERSION_NUMBER_KEY: &str = "version_number_key";
const VERSION_CODE_KEY: &str = "version_code_key";

pub struct GitHubProvider;

impl Default for GitHubProvider {
    fn default() -> Self {
        Self::new()
    }
}

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
    fn get_uuid(&self) -> &'static str {
        "fd9b2602-62c5-4d55-bd1e-0d6537714ca0"
    }

    fn get_friendly_name(&self) -> &'static str {
        "github"
    }

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
