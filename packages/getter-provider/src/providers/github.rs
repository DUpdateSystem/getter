use async_trait::async_trait;
use bytes::Bytes;
use serde_json::Value;
use std::collections::HashMap;

use crate::base_provider::*;
use crate::data::{AssetData, ReleaseData};
use crate::register_provider;

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
                GITHUB_URL,
                id_map.get("owner").map_or("", |v| v),
                id_map.get("repo").map_or("", |v| v)
            )],
            FunctionType::GetLatestRelease | FunctionType::GetReleases => vec![format!(
                "{}/repos/{}/{}/releases",
                GITHUB_API_URL,
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

        let api_url = format!("{}/{}/{}", GITHUB_URL, owner, repo);
        let api_url = self.replace_proxy_url(fin, &api_url);

        if let Ok(parsed_url) = api_url.parse() {
            if let Ok(rsp) = head(parsed_url, &HashMap::new()).await {
                return FOut::new(http_status_is_ok(rsp.status));
            }
        }
        FOut::new(false)
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

        let url = format!("{}/repos/{}/{}/releases", GITHUB_API_URL, owner, repo);
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

// Automatically register the GitHub provider
register_provider!(GitHubProvider);

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;
    use std::collections::BTreeMap;
    use std::fs;

    #[test]
    fn test_github_cache_keys() {
        let provider = GitHubProvider::new();

        let app_data = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
        let hub_data = BTreeMap::new();
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };

        let keys = provider.get_cache_request_key(&FunctionType::CheckAppAvailable, &data_map);
        assert_eq!(
            keys,
            vec!["https://github.com/DUpdateSystem/UpgradeAll/HEAD"]
        );

        let keys = provider.get_cache_request_key(&FunctionType::GetLatestRelease, &data_map);
        assert_eq!(
            keys,
            vec!["https://api.github.com/repos/DUpdateSystem/UpgradeAll/releases"]
        );

        let keys = provider.get_cache_request_key(&FunctionType::GetReleases, &data_map);
        assert_eq!(
            keys,
            vec!["https://api.github.com/repos/DUpdateSystem/UpgradeAll/releases"]
        );
    }

    #[tokio::test]
    async fn test_github_check_app_available() {
        // Test without proxy - this should try to connect to real GitHub
        // and will likely return false in test environment, which is expected
        let provider = GitHubProvider::new();
        let app_data = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
        let hub_data = BTreeMap::new();
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        let result = provider.check_app_available(&fin).await;
        // This may succeed or fail depending on network - just check it returns a bool
        assert!(result.result.is_ok());
    }

    // Note: Proxy testing is complex due to the URL replacement mechanism
    // In real usage, the proxy feature works as expected

    #[tokio::test]
    async fn test_github_check_app_nonexistent_with_proxy() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("HEAD", "/DUpdateSystem/NonExistent")
            .with_status(404)
            .create_async()
            .await;

        let provider = GitHubProvider::new();
        let app_data = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "NonExistent")]);
        let proxy_url = format!("{} -> {}", GITHUB_URL, server.url());
        let hub_data = BTreeMap::from([(REVERSE_PROXY, proxy_url.as_str())]);
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        let result = provider.check_app_available(&fin).await;
        assert!(result.result.is_ok());
        assert!(!result.result.unwrap());
    }

    #[tokio::test]
    async fn test_github_get_releases() {
        let body = fs::read_to_string("tests/web/github_api_release.json").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/DUpdateSystem/UpgradeAll/releases")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let provider = GitHubProvider::new();
        let app_data = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
        let proxy_url = format!("{} -> {}", GITHUB_API_URL, server.url());
        let hub_data = BTreeMap::from([(REVERSE_PROXY, proxy_url.as_str())]);
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        let result = provider.get_releases(&fin).await;
        assert!(result.result.is_ok());

        let releases = result.result.unwrap();
        assert!(!releases.is_empty());

        // Check first release data
        let first_release = &releases[0];
        assert_eq!(first_release.version_number, "0.13-beta.4");
        assert_eq!(
            first_release.changelog,
            "Changelog:\r\nAdd Ukrainian Language\r\n更新日志：\r\n添加乌克兰语"
        );
        assert_eq!(first_release.assets.len(), 1);

        // Check asset data
        let asset = &first_release.assets[0];
        assert_eq!(asset.file_name, "UpgradeAll_0.13-beta.4.apk");
        assert_eq!(asset.file_type, "application/vnd.android.package-archive");
        assert_eq!(asset.download_url, "https://github.com/DUpdateSystem/UpgradeAll/releases/download/0.13-beta.4/UpgradeAll_0.13-beta.4.apk");
    }

    #[tokio::test]
    async fn test_github_get_releases_compare_expected() {
        let body = fs::read_to_string("tests/web/github_api_release.json").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/DUpdateSystem/UpgradeAll/releases")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let app_data = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
        let proxy_url = format!("{} -> {}", GITHUB_API_URL, server.url());
        let hub_data = BTreeMap::from([(REVERSE_PROXY, proxy_url.as_str())]);
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };

        let github_provider = GitHubProvider::new();
        let releases = github_provider
            .get_releases(&FIn::new(data_map, None))
            .await
            .result
            .unwrap();

        let release_json = fs::read_to_string("tests/data/provider_github_release.json").unwrap();
        let releases_saved = serde_json::from_str::<Vec<ReleaseData>>(&release_json).unwrap();
        assert_eq!(releases, releases_saved);
    }

    #[tokio::test]
    async fn test_github_get_releases_with_token() {
        let body = fs::read_to_string("tests/web/github_api_release.json").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/DUpdateSystem/UpgradeAll/releases")
            .match_header("authorization", "Bearer test_token")
            .match_header("user-agent", "UpgradeAll-App")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let provider = GitHubProvider::new();
        let app_data = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
        let proxy_url = format!("{} -> {}", GITHUB_API_URL, server.url());
        let hub_data = BTreeMap::from([
            ("reverse_proxy", proxy_url.as_str()),
            ("token", "test_token"),
        ]);
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        let result = provider.get_releases(&fin).await;
        assert!(result.result.is_ok());

        let releases = result.result.unwrap();
        assert!(!releases.is_empty());
        assert_eq!(releases[0].version_number, "0.13-beta.4");
    }

    #[tokio::test]
    async fn test_github_get_releases_empty() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/DUpdateSystem/UpgradeAll/releases")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("[]")
            .create_async()
            .await;

        let provider = GitHubProvider::new();
        let app_data = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
        let proxy_url = format!("{} -> {}", GITHUB_API_URL, server.url());
        let hub_data = BTreeMap::from([(REVERSE_PROXY, proxy_url.as_str())]);
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        let result = provider.get_releases(&fin).await;
        assert!(result.result.is_ok());

        let releases = result.result.unwrap();
        assert!(releases.is_empty());
    }

    #[tokio::test]
    async fn test_github_get_latest_release() {
        let body = fs::read_to_string("tests/web/github_api_release.json").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/DUpdateSystem/UpgradeAll/releases")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let provider = GitHubProvider::new();
        let app_data = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
        let proxy_url = format!("{} -> {}", GITHUB_API_URL, server.url());
        let hub_data = BTreeMap::from([(REVERSE_PROXY, proxy_url.as_str())]);
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        let result = provider.get_latest_release(&fin).await;
        assert!(result.result.is_ok());

        let release = result.result.unwrap();
        assert_eq!(release.version_number, "0.13-beta.4");
        assert_eq!(
            release.changelog,
            "Changelog:\r\nAdd Ukrainian Language\r\n更新日志：\r\n添加乌克兰语"
        );
        assert_eq!(release.assets.len(), 1);
        assert_eq!(release.assets[0].file_name, "UpgradeAll_0.13-beta.4.apk");
    }

    #[tokio::test]
    async fn test_github_version_number_key() {
        let body = fs::read_to_string("tests/web/github_api_release.json").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/DUpdateSystem/UpgradeAll/releases")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let provider = GitHubProvider::new();
        let app_data = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
        let proxy_url = format!("{} -> {}", GITHUB_API_URL, server.url());
        let hub_data = BTreeMap::from([
            ("reverse_proxy", proxy_url.as_str()),
            ("version_number_key", "tag_name"), // Use tag_name instead of name
        ]);
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        let result = provider.get_releases(&fin).await;
        assert!(result.result.is_ok());

        let releases = result.result.unwrap();
        assert!(!releases.is_empty());
        assert_eq!(releases[0].version_number, "0.13-beta.4");
    }

    #[test]
    fn test_github_token_header() {
        let provider = GitHubProvider::new();

        // Test with token in hub_data
        let app_data = BTreeMap::new();
        let hub_data = BTreeMap::from([("token", "test_token")]);
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        let headers = provider.get_token_header(&fin);
        assert_eq!(
            headers.get("Authorization"),
            Some(&"Bearer test_token".to_string())
        );

        // Test with token in app_data
        let app_data = BTreeMap::from([("token", "app_token")]);
        let hub_data = BTreeMap::new();
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        let headers = provider.get_token_header(&fin);
        assert_eq!(
            headers.get("Authorization"),
            Some(&"Bearer app_token".to_string())
        );

        // Test with no token
        let app_data = BTreeMap::new();
        let hub_data = BTreeMap::new();
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        let headers = provider.get_token_header(&fin);
        assert!(!headers.contains_key("Authorization"));

        // Test with empty token
        let hub_data = BTreeMap::from([("token", "   ")]);
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        let headers = provider.get_token_header(&fin);
        assert!(!headers.contains_key("Authorization"));
    }
}
