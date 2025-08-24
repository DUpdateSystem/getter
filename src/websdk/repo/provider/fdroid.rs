use async_trait::async_trait;
use bytes::Bytes;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;
use std::error::Error;

use crate::utils::http::{get, head, http_status_is_ok};

use super::super::data::release::*;
use super::base_provider::*;

const FDROID_URL: &str = "https://f-droid.org";

pub struct FDroidProvider;

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
        "6a6d590b-1809-41bf-8ce3-7e3f6c8da945"
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
            FunctionType::CheckAppAvailable => vec![format!(
                "{}/packages/{}/HEAD",
                url, id_map[ANDROID_APP_TYPE]
            )],
            FunctionType::GetLatestRelease | FunctionType::GetReleases => vec![api_url.to_string()],
        }
    }

    async fn check_app_available(&self, fin: &FIn) -> FOut<bool> {
        let (url, _) = FDroidProvider::get_urls(&fin.data_map);
        let id_map = fin.data_map.app_data;
        let package_id = id_map[ANDROID_APP_TYPE];
        let api_url = format!("{}/packages/{}", url, package_id);
        let api_url = self.replace_proxy_url(fin, &api_url);

        if let Ok(parsed_url) = api_url.parse() {
            if let Ok(rsp) = head(parsed_url, &HashMap::new()).await {
                return FOut::new(http_status_is_ok(rsp.status));
            }
        }
        FOut::new_empty()
    }

    async fn get_releases(&self, fin: &FIn) -> FOut<Vec<ReleaseData>> {
        let (url, api_url) = FDroidProvider::get_urls(&fin.data_map);
        let id_map = fin.data_map.app_data;
        let package_id = id_map[ANDROID_APP_TYPE];
        let api_url = self.replace_proxy_url(fin, &api_url);
        let cache_key = self
            .get_cache_request_key(&FunctionType::GetReleases, &fin.data_map)
            .first()
            .unwrap()
            .clone();
        let mut cache_map_fout = CacheMap::new();
        let index_cache = fin.get_cache(&cache_key);
        let mut index: Option<Bytes> = None;
        if let Some(i) = index_cache {
            index = Some(i.clone());
        } else if let Ok(parsed_url) = api_url.parse() {
            if let Ok(rsp) = get(parsed_url, &HashMap::new()).await {
                index = rsp.body;
                cache_map_fout.insert(cache_key.to_string(), index.clone().unwrap());
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
                        .unwrap();
                if xml_package_id == package_id {
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
                    if let Ok(e) = e.unescape() {
                        let text = e.into_owned();
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
                }
                _ => (),
            };
        }
        let download_url = format!("{}/{}", url, file_name);
        let file_type = file_name.split('.').last().unwrap_or("").to_string();

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
                    if let Ok(e) = e.unescape() {
                        let text = e.into_owned();
                        if current_tag.as_str() == "changelog" {
                            changelog += &text
                        }
                    }
                }
                _ => (),
            };
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

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;
    use std::fs;

    #[tokio::test]
    async fn test_check_app_available() {
        let package_id = "com.termux";
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", format!("/packages/{}", package_id).as_str())
            .with_status(200)
            .create_async()
            .await;

        let provider = FDroidProvider::new();
        let app_data = AppDataMap::from([(ANDROID_APP_TYPE, package_id)]);
        let proxy_url = format!("{} -> {}", FDROID_URL, server.url());
        let hub_data = HubDataMap::from([(REVERSE_PROXY, proxy_url.as_str())]);
        let fin = FIn::new_with_frag(&app_data, &hub_data, None);
        let fout = provider.check_app_available(&fin).await;
        assert!(fout.result.unwrap());
    }

    #[tokio::test]
    async fn test_check_app_available_nonexist() {
        let package_id = "com.termux";
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", format!("/packages/{}", package_id).as_str())
            .with_status(200)
            .create_async()
            .await;

        let provider = FDroidProvider::new();
        let nonexist_package_id = "nonexist";
        let app_data = AppDataMap::from([(ANDROID_APP_TYPE, nonexist_package_id)]);
        let proxy_url = format!("{} -> {}", FDROID_URL, server.url());
        let hub_data = HubDataMap::from([(REVERSE_PROXY, proxy_url.as_str())]);
        let fin = FIn::new_with_frag(&app_data, &hub_data, None);
        let fout = provider.check_app_available(&fin).await;
        assert!(!fout.result.unwrap());
    }

    #[tokio::test]
    async fn test_get_releases() {
        let body = fs::read_to_string("tests/files/web/f-droid.xml").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/repo/index.xml")
            .with_status(200)
            .with_body(body)
            .create();

        let package_id = "org.fdroid.fdroid.privileged";
        let provider = FDroidProvider::new();
        let app_data = AppDataMap::from([(ANDROID_APP_TYPE, package_id)]);
        let proxy_url = format!("{} -> {}", FDROID_URL, server.url());
        let hub_data = HubDataMap::from([(REVERSE_PROXY, proxy_url.as_str())]);
        let fin = FIn::new_with_frag(&app_data, &hub_data, None);
        let fout = provider.get_releases(&fin).await;
        let releases = fout.result.unwrap();
        assert!(!releases.is_empty());
        assert_eq!(releases[0].assets[0].file_type, "apk");
    }

    #[tokio::test]
    async fn test_get_releases_nonexist() {
        let body = fs::read_to_string("tests/files/web/f-droid.xml").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/repo/index.xml")
            .with_status(200)
            .with_body(body)
            .create();

        let package_id = "nonexist";
        let provider = FDroidProvider::new();
        let app_data = AppDataMap::from([(ANDROID_APP_TYPE, package_id)]);
        let proxy_url = format!("{} -> {}", FDROID_URL, server.url());
        let hub_data = HubDataMap::from([(REVERSE_PROXY, proxy_url.as_str())]);
        let fin = FIn::new_with_frag(&app_data, &hub_data, None);
        let fout = provider.get_releases(&fin).await;
        let releases = fout.result.unwrap();
        assert!(releases.is_empty());
    }

    #[tokio::test]
    async fn test_get_releases_assets_type() {
        let body = fs::read_to_string("tests/files/web/f-droid.xml").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/repo/index.xml")
            .with_status(200)
            .with_body(body)
            .create();

        let package_id = "org.fdroid.fdroid.privileged.ota";
        let provider = FDroidProvider::new();
        let app_data = AppDataMap::from([(ANDROID_APP_TYPE, package_id)]);
        let proxy_url = format!("{} -> {}", FDROID_URL, server.url());
        let hub_data = HubDataMap::from([(REVERSE_PROXY, proxy_url.as_str())]);
        let fin = FIn::new_with_frag(&app_data, &hub_data, None);
        let fout = provider.get_releases(&fin).await;
        let releases = fout.result.unwrap();
        assert!(!releases.is_empty());
        assert_eq!(releases[0].assets[0].file_type, "zip");
    }
}
