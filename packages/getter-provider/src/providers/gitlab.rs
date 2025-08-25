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

const GITLAB_API_BASE: &str = "https://gitlab.com/api/v4";

/// GitLab repository provider
///
/// Supports fetching releases from GitLab repositories.
/// App data expected keys:
/// - project_id: GitLab project ID (numeric) or project path (e.g. "group/project")
///
/// Hub data keys (optional):
/// - gitlab_host: Custom GitLab host (default: gitlab.com)
/// - access_token: GitLab access token for private repositories
/// - include_prereleases: Include pre-release versions (default: false)
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

    fn get_api_base(&self, gitlab_host: Option<&str>) -> String {
        gitlab_host
            .map(|host| format!("https://{}/api/v4", host))
            .unwrap_or_else(|| GITLAB_API_BASE.to_string())
    }

    fn get_project_url(&self, project_id: &str, gitlab_host: Option<&str>) -> String {
        let api_base = self.get_api_base(gitlab_host);
        let encoded_project = encode(project_id);
        format!("{}/projects/{}", api_base, encoded_project)
    }

    fn get_releases_url(&self, project_id: &str, gitlab_host: Option<&str>) -> String {
        let api_base = self.get_api_base(gitlab_host);
        let encoded_project = encode(project_id);
        format!("{}/projects/{}/releases", api_base, encoded_project)
    }

    fn build_headers(&self, access_token: Option<&str>) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        headers.insert("User-Agent".to_string(), "getter-provider".to_string());

        if let Some(token) = access_token {
            headers.insert("Authorization".to_string(), format!("Bearer {}", token));
        }

        headers
    }

    fn parse_release_data(&self, release_data: &Value) -> Option<ReleaseData> {
        let tag_name = release_data["tag_name"].as_str()?.to_string();
        let _name = release_data["name"].as_str().unwrap_or(&tag_name);
        let description = release_data["description"].as_str().map(|s| s.to_string());
        let _released_at = release_data["released_at"].as_str().map(|s| s.to_string());

        // Parse assets/links from GitLab release
        let mut assets = Vec::new();

        // GitLab releases can have links
        if let Some(links) = release_data["assets"]["links"].as_array() {
            for link in links {
                let name = link["name"].as_str()?;
                let url = link["url"].as_str()?;
                let link_type = link["link_type"].as_str();

                assets.push(AssetData {
                    file_name: name.to_string(),
                    file_type: link_type.unwrap_or("unknown").to_string(),
                    download_url: url.to_string(),
                });
            }
        }

        // GitLab releases can have source archives
        if let Some(sources) = release_data["assets"]["sources"].as_array() {
            for source in sources {
                let format = source["format"].as_str()?;
                let url = source["url"].as_str()?;

                assets.push(AssetData {
                    file_name: format!("{}.{}", tag_name, format),
                    file_type: match format {
                        "zip" => "application/zip".to_string(),
                        "tar.gz" => "application/gzip".to_string(),
                        "tar.bz2" => "application/x-bzip2".to_string(),
                        "tar" => "application/x-tar".to_string(),
                        _ => "application/octet-stream".to_string(),
                    },
                    download_url: url.to_string(),
                });
            }
        }

        // Use the first asset as the primary download URL, or construct a default
        let _download_url = assets
            .first()
            .map(|a| a.download_url.clone())
            .unwrap_or_else(|| {
                format!(
                    "https://gitlab.com/{}/-/archive/{}/{}-{}.zip",
                    release_data["_project_path"]
                        .as_str()
                        .unwrap_or("unknown/unknown"),
                    tag_name,
                    release_data["_project_name"].as_str().unwrap_or("project"),
                    tag_name
                )
            });

        Some(ReleaseData {
            version_number: tag_name,
            changelog: description.unwrap_or_default(),
            assets,
            extra: None,
        })
    }

    fn filter_prereleases(
        &self,
        releases: &[ReleaseData],
        include_prereleases: bool,
    ) -> Vec<ReleaseData> {
        if include_prereleases {
            releases.to_vec()
        } else {
            releases
                .iter()
                .filter(|release| {
                    let version = &release.version_number;
                    // Filter out versions containing common prerelease indicators
                    !version.contains("-alpha")
                        && !version.contains("-beta")
                        && !version.contains("-rc")
                        && !version.contains("-pre")
                        && !version.contains("-dev")
                })
                .cloned()
                .collect()
        }
    }
}

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
        data_map: &DataMap<'_>,
    ) -> Vec<String> {
        let project_id = data_map
            .app_data
            .get("project_id")
            .copied()
            .unwrap_or("unknown");
        let gitlab_host = data_map
            .hub_data
            .get("gitlab_host")
            .copied()
            .unwrap_or("gitlab.com");
        let include_prereleases = data_map
            .hub_data
            .get("include_prereleases")
            .map(|s| *s == "true")
            .unwrap_or(false);

        match function_type {
            FunctionType::CheckAppAvailable => {
                vec![format!("gitlab:check:{}:{}", gitlab_host, project_id)]
            }
            FunctionType::GetLatestRelease => {
                vec![format!(
                    "gitlab:latest:{}:{}:{}",
                    gitlab_host, project_id, include_prereleases
                )]
            }
            FunctionType::GetReleases => {
                vec![format!(
                    "gitlab:releases:{}:{}:{}",
                    gitlab_host, project_id, include_prereleases
                )]
            }
        }
    }

    async fn check_app_available(&self, fin: &FIn) -> FOut<bool> {
        let project_id = match fin.data_map.app_data.get("project_id") {
            Some(id) => id,
            None => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(
                    "Missing project_id in app_data",
                )))
            }
        };

        let gitlab_host = fin.data_map.hub_data.get("gitlab_host").copied();
        let access_token = fin.data_map.hub_data.get("access_token").copied();

        let url = self.get_project_url(project_id, gitlab_host);
        let headers = self.build_headers(access_token);

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

    async fn get_latest_release(&self, fin: &FIn) -> FOut<ReleaseData> {
        let project_id = match fin.data_map.app_data.get("project_id") {
            Some(id) => id,
            None => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(
                    "Missing project_id in app_data",
                )))
            }
        };

        let gitlab_host = fin.data_map.hub_data.get("gitlab_host").copied();
        let access_token = fin.data_map.hub_data.get("access_token").copied();
        let include_prereleases = fin
            .data_map
            .hub_data
            .get("include_prereleases")
            .map(|s| *s == "true")
            .unwrap_or(false);

        let url = self.get_releases_url(project_id, gitlab_host);
        let headers = self.build_headers(access_token);

        let uri: Uri = match url.parse() {
            Ok(uri) => uri,
            Err(e) => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(format!(
                    "Invalid URL: {}",
                    e
                ))))
            }
        };

        let response = match get(uri, &headers).await {
            Ok(resp) => resp,
            Err(e) => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(format!(
                    "Request failed: {}",
                    e
                ))))
            }
        };

        if response.status != 200 {
            return FOut::new_empty().set_error(Box::new(std::io::Error::other(format!(
                "HTTP error: {}",
                response.status
            ))));
        }

        let body_bytes = response.body.unwrap_or_default();
        let body = match String::from_utf8(body_bytes.to_vec()) {
            Ok(text) => text,
            Err(e) => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(format!(
                    "Failed to read response: {}",
                    e
                ))))
            }
        };

        let releases_data: Value = match serde_json::from_str(&body) {
            Ok(data) => data,
            Err(e) => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(format!(
                    "Failed to parse JSON: {}",
                    e
                ))))
            }
        };

        let releases = releases_data
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|r| self.parse_release_data(r))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let filtered_releases = self.filter_prereleases(&releases, include_prereleases);

        let mut fout = match filtered_releases.first() {
            Some(release) => FOut::new(release.clone()),
            None => {
                return FOut::new_empty()
                    .set_error(Box::new(std::io::Error::other("No releases found")))
            }
        };

        let cache_bytes: Bytes = body_bytes.clone();
        fout = fout.set_cache(&url, cache_bytes);
        fout
    }

    async fn get_releases(&self, fin: &FIn) -> FOut<Vec<ReleaseData>> {
        let project_id = match fin.data_map.app_data.get("project_id") {
            Some(id) => id,
            None => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(
                    "Missing project_id in app_data",
                )))
            }
        };

        let gitlab_host = fin.data_map.hub_data.get("gitlab_host").copied();
        let access_token = fin.data_map.hub_data.get("access_token").copied();
        let include_prereleases = fin
            .data_map
            .hub_data
            .get("include_prereleases")
            .map(|s| *s == "true")
            .unwrap_or(false);

        let url = format!(
            "{}?per_page=50",
            self.get_releases_url(project_id, gitlab_host)
        );
        let headers = self.build_headers(access_token);

        let uri: Uri = match url.parse() {
            Ok(uri) => uri,
            Err(e) => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(format!(
                    "Invalid URL: {}",
                    e
                ))))
            }
        };

        let response = match get(uri, &headers).await {
            Ok(resp) => resp,
            Err(e) => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(format!(
                    "Request failed: {}",
                    e
                ))))
            }
        };

        if response.status != 200 {
            return FOut::new_empty().set_error(Box::new(std::io::Error::other(format!(
                "HTTP error: {}",
                response.status
            ))));
        }

        let body_bytes = response.body.unwrap_or_default();
        let body = match String::from_utf8(body_bytes.to_vec()) {
            Ok(text) => text,
            Err(e) => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(format!(
                    "Failed to read response: {}",
                    e
                ))))
            }
        };

        let releases_data: Value = match serde_json::from_str(&body) {
            Ok(data) => data,
            Err(e) => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(format!(
                    "Failed to parse JSON: {}",
                    e
                ))))
            }
        };

        let releases = releases_data
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|r| self.parse_release_data(r))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let filtered_releases = self.filter_prereleases(&releases, include_prereleases);

        let mut fout = FOut::new(filtered_releases);
        let cache_bytes: Bytes = body_bytes.clone();
        fout = fout.set_cache(&url, cache_bytes);
        fout
    }
}

// Automatically register the GitLab provider
register_provider!(GitLabProvider);
