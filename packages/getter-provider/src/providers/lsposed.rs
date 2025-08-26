use std::collections::HashMap;

use async_trait::async_trait;
use bytes::Bytes;
use serde_json::Value;

use getter_utils::{http::get, versioning::Version};

use crate::base_provider::*;
use crate::data::{AssetData, ReleaseData};
use crate::register_provider;

const LSPOSED_REPO_API_URL: &str = "https://modules.lsposed.org/modules.json";

pub struct LsposedRepoProvider;

impl Default for LsposedRepoProvider {
    fn default() -> Self {
        Self::new()
    }
}

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
    fn get_uuid(&self) -> &'static str {
        "8e7f9a2d-5c41-4b68-9f31-2e8d7c6a4b9f"
    }

    fn get_friendly_name(&self) -> &'static str {
        "lsposed"
    }

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
        let package_id = match id_map.get(ANDROID_APP_TYPE) {
            Some(id) => id,
            None => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(
                    "Missing android_app_package in app_data",
                )))
            }
        };
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
        let package_id = match id_map.get(ANDROID_APP_TYPE) {
            Some(id) => id,
            None => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(
                    "Missing android_app_package in app_data",
                )))
            }
        };
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

                                    for key in &["name", "tagName"] {
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

// Automatically register the LSPosed provider
register_provider!(LsposedRepoProvider);
