use async_trait::async_trait;
use bytes::Bytes;
use serde_json::Value;
use std::collections::HashMap;
use urlencoding::encode;

use crate::base_provider::*;
use crate::data::{AssetData, ReleaseData};
use crate::register_provider;

use getter_utils::http::get;
use hyper::Uri;

const GITLAB_URL: &str = "https://gitlab.com";
const GITLAB_API_URL: &str = "https://gitlab.com/api/v4/projects";

pub struct GitLabProvider;

impl Default for GitLabProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl GitLabProvider {
    pub fn new() -> Self {
        GitLabProvider
    }

    async fn get_project_id(&self, fin: &FIn<'_>) -> Option<String> {
        let id_map = fin.data_map.app_data;
        let api_url = format!(
            "{}/{}%2F{}",
            GITLAB_API_URL,
            id_map.get("owner")?,
            id_map.get("repo")?
        );
        let api_url = self.replace_proxy_url(fin, &api_url);

        if let Ok(parsed_url) = api_url.parse() {
            let headers = self.build_headers();
            if let Ok(rsp) = get(parsed_url, &headers).await {
                if let Some(body) = rsp.body {
                    if let Ok(data) = serde_json::from_slice::<HashMap<String, Value>>(&body) {
                        return Some(data.get("id")?.as_number()?.to_string());
                    }
                }
            }
        }
        None
    }

    fn build_headers(&self) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        headers.insert("User-Agent".to_string(), "getter-provider".to_string());
        headers
    }

    pub fn fix_download_url(&self, download_url: &str, project_id: &str) -> String {
        if download_url.starts_with("/uploads/") {
            return format!("{}/-/project/{}{}", GITLAB_URL, project_id, download_url);
        }
        download_url.to_string()
    }

    pub fn parse_release_data(&self, release_data: &Value) -> Option<ReleaseData> {
        // Try different keys for version number
        let keys_to_try = ["tag_name", "name"];
        let mut version_number: Option<String> = None;

        for key in keys_to_try.iter() {
            if let Some(value) = release_data.get(key).and_then(|v| v.as_str()) {
                // Simple version validation - just check if it's not empty
                if !value.is_empty() {
                    version_number = Some(value.to_string());
                    break;
                }
            }
        }

        let changelog = release_data
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Parse assets from GitLab API response
        let mut assets = Vec::new();
        if let Some(links) = release_data
            .get("assets")
            .and_then(|a| a.get("links"))
            .and_then(|l| l.as_array())
        {
            for link in links {
                if let (Some(name), Some(url)) = (
                    link.get("name").and_then(|v| v.as_str()),
                    link.get("url").and_then(|v| v.as_str()),
                ) {
                    let link_type = link
                        .get("link_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("other");
                    assets.push(AssetData {
                        file_name: name.to_string(),
                        file_type: link_type.to_string(),
                        download_url: url.to_string(),
                    });
                }
            }
        }

        Some(ReleaseData {
            version_number: version_number?,
            changelog,
            assets,
            extra: None,
        })
    }
}

impl BaseProviderExt for GitLabProvider {}

#[async_trait]
impl BaseProvider for GitLabProvider {
    fn get_uuid(&self) -> &'static str {
        "2f8e3d4c-1a9b-4e7f-8c2d-5a3b9e6f1c4d"
    }

    fn get_friendly_name(&self) -> &'static str {
        "gitlab"
    }

    fn get_cache_request_key(
        &self,
        function_type: &FunctionType,
        data_map: &DataMap,
    ) -> Vec<String> {
        let id_map = data_map.app_data;
        match function_type {
            FunctionType::CheckAppAvailable => vec![format!(
                "{}/{}/{}",
                GITLAB_URL,
                id_map.get("owner").map_or("", |v| v),
                id_map.get("repo").map_or("", |v| v)
            )],
            FunctionType::GetLatestRelease | FunctionType::GetReleases => vec![format!(
                "{}/{}%2F{}/releases",
                GITLAB_API_URL,
                id_map.get("owner").map_or("", |v| v),
                id_map.get("repo").map_or("", |v| v)
            )],
        }
    }

    async fn check_app_available(&self, fin: &FIn) -> FOut<bool> {
        let id_map = fin.data_map.app_data;
        let owner = match id_map.get("owner") {
            Some(owner) => owner,
            None => {
                return FOut::new_empty()
                    .set_error(Box::new(std::io::Error::other("Missing owner in app_data")))
            }
        };
        let repo = match id_map.get("repo") {
            Some(repo) => repo,
            None => {
                return FOut::new_empty()
                    .set_error(Box::new(std::io::Error::other("Missing repo in app_data")))
            }
        };

        let api_url = format!("{}/{}/{}", GITLAB_URL, owner, repo);
        let api_url = self.replace_proxy_url(fin, &api_url);
        let headers = self.build_headers();

        let uri: Uri = match api_url.parse() {
            Ok(uri) => uri,
            Err(e) => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(format!(
                    "Invalid URL: {}",
                    e
                ))))
            }
        };

        match get(uri, &headers).await {
            Ok(response) => {
                if response.status == 200 {
                    FOut::new(true)
                } else if response.status == 404 {
                    FOut::new(false)
                } else {
                    FOut::new_empty().set_error(Box::new(std::io::Error::other(format!(
                        "HTTP error: {}",
                        response.status
                    ))))
                }
            }
            Err(e) => FOut::new_empty().set_error(Box::new(std::io::Error::other(format!(
                "Request failed: {}",
                e
            )))),
        }
    }

    async fn get_releases(&self, fin: &FIn) -> FOut<Vec<ReleaseData>> {
        let id_map = fin.data_map.app_data;
        let owner = match id_map.get("owner") {
            Some(owner) => owner,
            None => {
                return FOut::new_empty()
                    .set_error(Box::new(std::io::Error::other("Missing owner in app_data")))
            }
        };
        let repo = match id_map.get("repo") {
            Some(repo) => repo,
            None => {
                return FOut::new_empty()
                    .set_error(Box::new(std::io::Error::other("Missing repo in app_data")))
            }
        };

        let url = format!(
            "{}/{}%2F{}/releases",
            GITLAB_API_URL,
            encode(owner),
            encode(repo)
        );
        let url = self.replace_proxy_url(fin, &url);
        let headers = self.build_headers();
        let mut fout = FOut::new_empty();

        // Check cache first
        let cache_body = fin.get_cache(&url);
        let mut rsp_body = None;

        if cache_body.is_none() {
            let uri: Uri = match url.parse() {
                Ok(uri) => uri,
                Err(e) => {
                    return FOut::new_empty().set_error(Box::new(std::io::Error::other(format!(
                        "Invalid URL: {}",
                        e
                    ))))
                }
            };

            match get(uri, &headers).await {
                Ok(response) => {
                    if response.status != 200 {
                        return FOut::new_empty().set_error(Box::new(std::io::Error::other(
                            format!("HTTP error: {}", response.status),
                        )));
                    }
                    if let Some(content) = response.body {
                        rsp_body = Some(content);
                    }
                }
                Err(e) => {
                    return FOut::new_empty().set_error(Box::new(std::io::Error::other(format!(
                        "Request failed: {}",
                        e
                    ))))
                }
            }
        }

        let body: &Bytes = if let Some(ref content) = rsp_body {
            content
        } else if let Some(content) = cache_body {
            content
        } else {
            return fout;
        };

        if let Ok(data) = serde_json::from_slice::<Vec<Value>>(body) {
            let mut release_list: Vec<ReleaseData> = data
                .iter()
                .filter_map(|json| self.parse_release_data(json))
                .collect();

            // Fix download URLs for relative paths
            let mut project_id = None;
            for release in release_list.iter_mut() {
                for asset in release.assets.iter_mut() {
                    if asset.download_url.starts_with("/uploads/") {
                        if project_id.is_none() {
                            project_id = self.get_project_id(fin).await;
                        }
                        if let Some(ref project_id) = project_id {
                            asset.download_url =
                                self.fix_download_url(&asset.download_url, project_id);
                        }
                    }
                }
            }

            fout = fout.set_data(release_list);
        }

        if let Some(content) = rsp_body {
            fout.set_cached_map(HashMap::from([(url, content)]))
        } else {
            fout
        }
    }
}

// Automatically register the GitLab provider
register_provider!(GitLabProvider);
