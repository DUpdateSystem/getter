use std::collections::HashMap;

use async_trait::async_trait;
use bytes::Bytes;
use serde_json::Value;

use crate::utils::http::get;
use crate::utils::versioning::Version;

use super::super::data::release::*;
use super::base_provider::*;

const LSPOSED_REPO_API_URL: &str = "https://modules.lsposed.org/modules.json";

pub struct LsposedRepoProvider;

impl LsposedRepoProvider {
    pub fn new() -> Self {
        LsposedRepoProvider {}
    }

    fn get_app_json(package_name: &str, body: &Bytes) -> Option<Value> {
        if let Ok(json) = serde_json::from_slice::<Vec<Value>>(body) {
            for i in json {
                if let Some(name) = i.get("name") {
                    if let Some(name_str) = name.as_str() {
                        if name_str == package_name {
                            return Some(i);
                        }
                    }
                }
            }
        }
        None
    }
}

impl BaseProviderExt for LsposedRepoProvider {}

#[async_trait]
impl BaseProvider for LsposedRepoProvider {
    fn get_cache_request_key(
        &self,
        _function_type: &FunctionType,
        _data_map: &DataMap,
    ) -> Vec<String> {
        vec![LSPOSED_REPO_API_URL.to_string()]
    }

    async fn check_app_available(&self, fin: &FIn) -> FOut<bool> {
        let url = self.replace_proxy_url(fin, LSPOSED_REPO_API_URL);
        let mut fout = FOut::new_empty();
        let cache_body = fin.get_cache(&url);
        let mut rsp_body = None;
        if cache_body.is_none() {
            if let Ok(parsed_url) = url.parse() {
                let map = HashMap::new();
                if let Ok(rsp) = get(parsed_url, &map).await {
                    if let Some(content) = rsp.body {
                        fout = fout.set_cache(&url, content.clone());
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
        let id_map = fin.data_map.app_data;
        let package_id = id_map[ANDROID_APP_TYPE];
        let json = LsposedRepoProvider::get_app_json(package_id, body);
        fout.set_data(json.is_some())
    }

    async fn get_releases(&self, fin: &FIn) -> FOut<Vec<ReleaseData>> {
        let url = self.replace_proxy_url(fin, LSPOSED_REPO_API_URL);
        let mut fout = FOut::new_empty();
        let cache_body = fin.get_cache(&url);
        let mut rsp_body = None;
        if cache_body.is_none() {
            if let Ok(parsed_url) = url.parse() {
                let map = HashMap::new();
                if let Ok(rsp) = get(parsed_url, &map).await {
                    if let Some(content) = rsp.body {
                        fout = fout.set_cache(&url, content.clone());
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
        let id_map = fin.data_map.app_data;
        let package_id = id_map[ANDROID_APP_TYPE];
        let json = LsposedRepoProvider::get_app_json(package_id, body);
        if let Some(json) = json {
            if let Some(releases_block) = json.get("releases") {
                if let Some(release_json) = releases_block.as_array() {
                    let release_list = release_json
                        .iter()
                        .filter_map(|json| {
                            if let Some(assets_block) = json.get("releaseAssets") {
                                if let Some(assets) = assets_block.as_array() {
                                    let assets_data =
                                        assets
                                            .iter()
                                            .filter_map(|asset| {
                                                Some(AssetData {
                                        file_name: asset.get("name")?.as_str()?.to_string(),
                                        file_type: asset
                                            .get("contentType")?
                                            .as_str()
                                            .unwrap_or("application/vnd.android.package-archive")
                                            .to_string(),
                                        download_url: asset
                                            .get("downloadUrl")?
                                            .as_str()?
                                            .to_string(),
                                    })
                                            })
                                            .collect();
                                    let mut version_number: Option<String> = None;

                                    for key in vec!["name", "tagName"].iter() {
                                        if let Some(value) = json.get(key).and_then(|v| v.as_str())
                                        {
                                            if Version::new(value.to_string()).is_valid() {
                                                version_number = Some(value.to_string());
                                                break;
                                            }
                                        }
                                    }
                                    let changelog = json
                                        .get("descriptionHTML")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    return Some(ReleaseData {
                                        version_number: version_number?.to_string(),
                                        changelog,
                                        assets: assets_data,
                                        extra: None,
                                    });
                                }
                            }
                            None
                        })
                        .collect();
                    fout = fout.set_data(release_list);
                }
            }
        }
        fout
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;
    use std::fs;

    #[tokio::test]
    async fn test_check_app_available() {
        let body = fs::read_to_string("tests/files/web/lsposed_modules.json").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/modules.json")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let id_map = AppDataMap::from([(ANDROID_APP_TYPE, "com.agoines.relaxhelp")]);
        let proxy_url = format!("{} -> {}", LSPOSED_REPO_API_URL, server.url());
        let hub_data = HubDataMap::from([("proxy_url", proxy_url.as_str())]);

        let lsposed_provider = LsposedRepoProvider::new();
        assert!(lsposed_provider
            .check_app_available(&FIn::new_with_frag(&id_map, &hub_data, None))
            .await
            .result
            .unwrap());
    }

    #[tokio::test]
    async fn test_get_releases() {
        let body = fs::read_to_string("tests/files/web/lsposed_modules.json").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/modules.json")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let id_map = AppDataMap::from([(ANDROID_APP_TYPE, "com.agoines.relaxhelp")]);
        let proxy_url = format!("{} -> {}", LSPOSED_REPO_API_URL, server.url());
        let hub_data = HubDataMap::from([("proxy_url", proxy_url.as_str())]);

        let lsposed_provider = LsposedRepoProvider::new();
        let releases = lsposed_provider
            .get_releases(&FIn::new_with_frag(&id_map, &hub_data, None))
            .await
            .result
            .unwrap();

        let release_json =
            fs::read_to_string("tests/files/data/provider_lsposed_releases.json").unwrap();
        let releases_saved = serde_json::from_str::<Vec<ReleaseData>>(&release_json).unwrap();
        assert_eq!(releases, releases_saved)
    }
}
