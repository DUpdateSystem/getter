use async_trait::async_trait;
use bytes::Bytes;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;
use std::error::Error;

use getter_utils::http::{get, head};

use crate::base_provider::*;
use crate::data::{AssetData, ReleaseData};
use crate::register_provider;

const FDROID_URL: &str = "https://f-droid.org";

pub struct FDroidProvider;

impl Default for FDroidProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl FDroidProvider {
    pub fn new() -> FDroidProvider {
        FDroidProvider {}
    }

    pub fn get_api_url(url: &str) -> String {
        format!("{}/repo/index.xml", url)
    }

    fn get_urls(data_map: &DataMap) -> (String, String) {
        let url = data_map.hub_data.get(KEY_REPO_URL).unwrap_or(&FDROID_URL);
        let api_url = if let Some(api_url) = data_map.hub_data.get(KEY_REPO_API_URL) {
            api_url.to_string()
        } else {
            FDroidProvider::get_api_url(url)
        };
        (url.to_string(), api_url)
    }
}

impl BaseProviderExt for FDroidProvider {}

#[async_trait]
impl BaseProvider for FDroidProvider {
    fn get_uuid(&self) -> &'static str {
        "fd9b2602-62c5-4d55-bd1e-0d6537714ca1"
    }

    fn get_friendly_name(&self) -> &'static str {
        "fdroid"
    }

    fn get_cache_request_key(
        &self,
        function_type: &FunctionType,
        data_map: &DataMap,
    ) -> Vec<String> {
        let (url, api_url) = FDroidProvider::get_urls(data_map);
        let id_map = data_map.app_data;
        match function_type {
            FunctionType::CheckAppAvailable => {
                let package_id = id_map.get(ANDROID_APP_TYPE).map_or("", |v| v);
                vec![format!("{}/packages/{}", url, package_id)]
            }
            FunctionType::GetLatestRelease | FunctionType::GetReleases => vec![api_url.to_string()],
        }
    }

    async fn check_app_available(&self, fin: &FIn) -> FOut<bool> {
        let (url, _) = FDroidProvider::get_urls(&fin.data_map);
        let id_map = fin.data_map.app_data;
        let package_id = match id_map.get(ANDROID_APP_TYPE) {
            Some(id) => id,
            None => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(
                    "Missing android_app_package in app_data",
                )))
            }
        };
        let api_url = format!("{}/packages/{}", url, package_id);
        let api_url = self.replace_proxy_url(fin, &api_url);

        if let Ok(parsed_url) = api_url.parse() {
            if let Ok(rsp) = head(parsed_url, &HashMap::new()).await {
                return FOut::new(rsp.status >= 200 && rsp.status < 300);
            }
        }
        FOut::new(false)
    }

    async fn get_releases(&self, fin: &FIn) -> FOut<Vec<ReleaseData>> {
        let (url, api_url) = FDroidProvider::get_urls(&fin.data_map);
        let id_map = fin.data_map.app_data;
        let package_id = match id_map.get(ANDROID_APP_TYPE) {
            Some(id) => id,
            None => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(
                    "Missing android_app_package in app_data",
                )))
            }
        };
        let api_url = self.replace_proxy_url(fin, &api_url);
        let cache_key = self
            .get_cache_request_key(&FunctionType::GetReleases, &fin.data_map)
            .first()
            .unwrap()
            .clone();
        let mut cache_map_fout = HashMap::new();
        let index_cache = fin.get_cache(&cache_key);
        let mut index: Option<Bytes> = None;
        if let Some(i) = index_cache {
            index = Some(i.clone());
        } else if let Ok(parsed_url) = api_url.parse() {
            if let Ok(rsp) = get(parsed_url, &HashMap::new()).await {
                index = rsp.body;
                if let Some(ref content) = index {
                    cache_map_fout.insert(cache_key.to_string(), content.clone());
                }
            }
        };
        if index.is_none() {
            return FOut::new_empty();
        }
        let mut releases_fout = Vec::new();
        if let Ok(content) = std::str::from_utf8(&index.unwrap()) {
            let mut reader = Reader::from_str(content.trim());
            loop {
                let (xml_package_id, releases) =
                    FDroidProvider::get_releases_from_xml(&mut reader, &url)
                        .await
                        .unwrap_or((String::new(), Vec::new()));
                if xml_package_id == *package_id {
                    releases_fout = releases;
                }
                if xml_package_id.is_empty() {
                    break;
                }
            }
        }
        let mut fout = FOut::new(releases_fout);
        if !cache_map_fout.is_empty() {
            fout = fout.set_cached_map(cache_map_fout);
        }
        fout
    }
}

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

impl FDroidProvider {
    async fn decode_package_xml(reader: &mut Reader<&[u8]>, url: &str) -> Result<ReleaseData> {
        let xml_key = b"package";
        let mut version_number = String::new();
        let mut changelog = String::new();
        let mut file_name = String::new();
        let mut extra = HashMap::new();

        let mut current_tag = String::new();
        loop {
            match reader.read_event() {
                Err(e) => return Err(Box::new(e)),
                Ok(Event::Eof) => break,
                Ok(Event::Start(e)) => {
                    let name = e.name();
                    current_tag = String::from_utf8_lossy(name.as_ref()).to_string();
                }
                Ok(Event::End(e)) => {
                    if e.name().as_ref() == xml_key {
                        break;
                    }
                }
                Ok(Event::Text(e)) => {
                    let text = String::from_utf8_lossy(&e).to_string();
                    match current_tag.as_str() {
                        "version" => version_number += &text,
                        "changelog" => changelog += &text,
                        "versionCode" | "nativecode" => {
                            extra.insert(current_tag.clone(), text.to_string());
                        }
                        "apkname" => file_name += &text,
                        _ => (),
                    }
                }
                _ => (),
            }
        }
        let download_url = format!("{}/{}", url, file_name);
        let file_type = file_name.split('.').next_back().unwrap_or("").to_string();

        let extra = if extra.is_empty() { None } else { Some(extra) };
        Ok(ReleaseData {
            version_number,
            changelog,
            assets: vec![AssetData {
                file_name,
                file_type,
                download_url,
            }],
            extra,
        })
    }

    async fn decode_release_xml(reader: &mut Reader<&[u8]>, url: &str) -> Result<Vec<ReleaseData>> {
        let mut releases = Vec::new();
        let mut changelog = String::new();

        let mut current_tag = String::new();
        loop {
            match reader.read_event() {
                Err(e) => return Err(Box::new(e)),
                Ok(Event::Eof) => break,
                Ok(Event::Start(e)) => {
                    let name = e.name();
                    match name.as_ref() {
                        b"package" => {
                            releases.push(FDroidProvider::decode_package_xml(reader, url).await?);
                        }
                        _ => {
                            current_tag = String::from_utf8_lossy(name.as_ref()).to_string();
                        }
                    }
                }
                Ok(Event::End(e)) => {
                    if e.name().as_ref() == b"application" {
                        break;
                    }
                }
                Ok(Event::Text(e)) => {
                    let text = String::from_utf8_lossy(&e).to_string();
                    if current_tag.as_str() == "changelog" {
                        changelog += &text;
                    }
                }
                _ => (),
            }
        }
        if !changelog.is_empty() {
            if let Some(release) = releases.first_mut() {
                release.changelog = changelog.clone();
            }
        }
        Ok(releases)
    }

    async fn get_releases_from_xml(
        reader: &mut Reader<&[u8]>,
        url: &str,
    ) -> Result<(String, Vec<ReleaseData>)> {
        let mut package_id = String::new();
        loop {
            match reader.read_event() {
                Err(e) => return Err(Box::new(e)),
                Ok(Event::Eof) => return Ok((package_id, Vec::new())),
                Ok(Event::Start(e)) => {
                    if e.name().as_ref() == b"application" {
                        for attr in e.attributes().filter_map(|id| id.ok()) {
                            if attr.key.as_ref() == b"id" {
                                package_id =
                                    String::from_utf8_lossy(attr.value.as_ref()).to_string();
                                let releases =
                                    FDroidProvider::decode_release_xml(reader, url).await?;
                                return Ok((package_id, releases));
                            }
                        }
                    }
                }
                _ => (),
            }
        }
    }
}

// Automatically register the F-Droid provider
register_provider!(FDroidProvider);
