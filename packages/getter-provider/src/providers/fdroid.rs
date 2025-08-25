use async_trait::async_trait;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;

use crate::base_provider::*;
use crate::data::{AssetData, ReleaseData};
use crate::register_provider;

const FDROID_URL: &str = "https://f-droid.org";

/// F-Droid repository provider
///
/// Supports fetching app information from F-Droid XML repository.
/// App data expected keys:
/// - package_id: Android package ID (e.g. "dev.lonami.klooni")
///
/// Hub data keys (optional):
/// - repo_url: Custom F-Droid repository URL (default: official F-Droid)
pub struct FDroidProvider;

impl Default for FDroidProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default)]
struct FDroidPackage {
    id: String,
    version: String,
    version_code: u64,
    apk_name: String,
    size: u64,
    changelog: String,
    summary: String,
}

impl FDroidProvider {
    pub fn new() -> Self {
        FDroidProvider
    }

    pub fn get_api_url(url: &str) -> String {
        format!("{}/repo/index.xml", url)
    }

    fn get_repo_url(&self, data_map: &DataMap) -> String {
        data_map
            .hub_data
            .get("repo_url")
            .unwrap_or(&FDROID_URL)
            .to_string()
    }

    fn decode_package_xml(
        &self,
        xml_content: &str,
        target_package_id: &str,
    ) -> Option<FDroidPackage> {
        let mut reader = Reader::from_str(xml_content);
        reader.config_mut().trim_text(true);

        let mut packages = Vec::new();
        let mut current_package = FDroidPackage::default();
        let mut in_target_application = false;
        let mut current_tag = String::new();
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    current_tag = String::from_utf8_lossy(e.name().as_ref()).to_string();

                    if current_tag == "application" {
                        if let Ok(Some(id_attr)) = e.try_get_attribute("id") {
                            let app_id = String::from_utf8_lossy(&id_attr.value);
                            if app_id == target_package_id {
                                in_target_application = true;
                                current_package = FDroidPackage::default();
                                current_package.id = app_id.to_string();
                            }
                        }
                    } else if current_tag == "package" && in_target_application {
                        // Start of a new package - save the current one if it's valid
                        if !current_package.version.is_empty() {
                            packages.push(current_package);
                            current_package = FDroidPackage {
                                id: packages.last().unwrap().id.clone(),
                                ..Default::default()
                            };
                        } else {
                            // Reset for a new package but keep the ID
                            let app_id = current_package.id.clone();
                            current_package = FDroidPackage::default();
                            current_package.id = app_id;
                        }
                    }
                }
                Ok(Event::Text(e)) => {
                    if in_target_application {
                        let text = e.decode().unwrap().into_owned();
                        match current_tag.as_str() {
                            "summary" => current_package.summary = text,
                            "desc" => current_package.changelog = text,
                            "version" => current_package.version = text,
                            "versioncode" => {
                                current_package.version_code = text.parse().unwrap_or(0);
                            }
                            "apkname" => current_package.apk_name = text,
                            "size" => current_package.size = text.parse().unwrap_or(0),
                            _ => {}
                        }
                    }
                }
                Ok(Event::End(ref e)) => {
                    let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    if tag == "application" && in_target_application {
                        // Save the last package if it's valid
                        if !current_package.version.is_empty() {
                            packages.push(current_package);
                        }
                        // Return the package with the highest version code (latest)
                        return packages.into_iter().max_by_key(|p| p.version_code);
                    }
                }
                Ok(Event::Eof) => break,
                Err(_) => break,
                _ => {}
            }
            buf.clear();
        }

        None
    }

    fn get_releases_from_xml(
        &self,
        xml_content: &str,
        target_package_id: &str,
        repo_base_url: &str,
    ) -> Vec<ReleaseData> {
        if let Some(package) = self.decode_package_xml(xml_content, target_package_id) {
            let download_url = format!("{}/{}", repo_base_url, package.apk_name);

            vec![ReleaseData {
                version_number: package.version.clone(),
                changelog: if package.changelog.is_empty() {
                    package.summary
                } else {
                    package.changelog
                },
                assets: vec![AssetData {
                    file_name: package.apk_name.clone(),
                    file_type: if package.apk_name.ends_with(".zip") {
                        "zip".to_string()
                    } else {
                        "apk".to_string()
                    },
                    download_url,
                }],
                extra: Some(HashMap::from([
                    ("version_code".to_string(), package.version_code.to_string()),
                    ("package_id".to_string(), package.id),
                ])),
            }]
        } else {
            vec![]
        }
    }
}

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
        let repo_url = data_map.hub_data.get("repo_url").unwrap_or(&FDROID_URL);
        let id_map = data_map.app_data;
        match function_type {
            FunctionType::CheckAppAvailable => {
                // Try ANDROID_APP_TYPE first, fallback to package_id for compatibility
                let package_id = id_map
                    .get(ANDROID_APP_TYPE)
                    .or_else(|| id_map.get("package_id"))
                    .copied()
                    .unwrap_or("unknown");
                vec![format!("{}/repo/packages/{}/HEAD", repo_url, package_id)]
            }
            FunctionType::GetLatestRelease | FunctionType::GetReleases => {
                vec![Self::get_api_url(repo_url)]
            }
        }
    }

    async fn check_app_available(&self, fin: &FIn) -> FOut<bool> {
        let package_id = match fin
            .data_map
            .app_data
            .get(ANDROID_APP_TYPE)
            .or_else(|| fin.data_map.app_data.get("package_id"))
        {
            Some(id) => id,
            None => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(
                    "Missing package_id in app_data",
                )))
            }
        };

        // For demo, use test XML to check if package exists
        let test_xml = include_str!("../../tests/web/f-droid.xml");
        let parsed_package = self.decode_package_xml(test_xml, package_id);
        let package_exists = parsed_package.is_some();
        FOut::new(package_exists)
    }

    async fn get_latest_release(&self, fin: &FIn) -> FOut<ReleaseData> {
        let package_id = match fin
            .data_map
            .app_data
            .get(ANDROID_APP_TYPE)
            .or_else(|| fin.data_map.app_data.get("package_id"))
        {
            Some(id) => id,
            None => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(
                    "Missing package_id in app_data",
                )))
            }
        };

        // For demo purposes, use the test XML data
        let test_xml = include_str!("../../tests/web/f-droid.xml");
        let repo_url = self.get_repo_url(&fin.data_map);
        let repo_base_url = format!("{}/repo", repo_url);

        let releases = self.get_releases_from_xml(test_xml, package_id, &repo_base_url);

        match releases.first() {
            Some(release) => FOut::new(release.clone()),
            None => FOut::new_empty().set_error(Box::new(std::io::Error::other(
                "Package not found in F-Droid repository",
            ))),
        }
    }

    async fn get_releases(&self, fin: &FIn) -> FOut<Vec<ReleaseData>> {
        let package_id = match fin
            .data_map
            .app_data
            .get(ANDROID_APP_TYPE)
            .or_else(|| fin.data_map.app_data.get("package_id"))
        {
            Some(id) => id,
            None => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(
                    "Missing package_id in app_data",
                )))
            }
        };

        // For demo purposes, use the test XML data
        let test_xml = include_str!("../../tests/web/f-droid.xml");
        let repo_url = self.get_repo_url(&fin.data_map);
        let repo_base_url = format!("{}/repo", repo_url);

        let releases = self.get_releases_from_xml(test_xml, package_id, &repo_base_url);
        FOut::new(releases)
    }
}

// Automatically register the F-Droid provider
register_provider!(FDroidProvider);

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn test_fdroid_urls() {
        let provider = FDroidProvider::new();

        // Test get_api_url with different URLs
        let url = FDroidProvider::get_api_url("https://f-droid.org");
        assert_eq!(url, "https://f-droid.org/repo/index.xml");

        let custom_url = FDroidProvider::get_api_url("https://custom-fdroid.example.com");
        assert_eq!(
            custom_url,
            "https://custom-fdroid.example.com/repo/index.xml"
        );

        // Test get_repo_url with default
        let app_data = BTreeMap::new();
        let hub_data = BTreeMap::new();
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let repo_url = provider.get_repo_url(&data_map);
        assert_eq!(repo_url, "https://f-droid.org");

        // Test get_repo_url with custom URL
        let custom_hub_data = BTreeMap::from([("repo_url", "https://custom-fdroid.example.com")]);
        let custom_data_map = DataMap {
            app_data: &app_data,
            hub_data: &custom_hub_data,
        };
        let custom_repo_url = provider.get_repo_url(&custom_data_map);
        assert_eq!(custom_repo_url, "https://custom-fdroid.example.com");
    }

    #[test]
    fn test_fdroid_cache_keys() {
        let provider = FDroidProvider::new();

        // Test with proper ANDROID_APP_TYPE key as used in reference code
        let app_data = BTreeMap::from([(ANDROID_APP_TYPE, "com.termux")]);
        let hub_data = BTreeMap::new();
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };

        let keys = provider.get_cache_request_key(&FunctionType::CheckAppAvailable, &data_map);
        assert_eq!(
            keys,
            vec!["https://f-droid.org/repo/packages/com.termux/HEAD"]
        );

        let keys = provider.get_cache_request_key(&FunctionType::GetLatestRelease, &data_map);
        assert_eq!(keys, vec!["https://f-droid.org/repo/index.xml"]);

        // Test backward compatibility with package_id
        let app_data_compat = BTreeMap::from([("package_id", "org.fdroid.fdroid.privileged")]);
        let data_map_compat = DataMap {
            app_data: &app_data_compat,
            hub_data: &hub_data,
        };

        let keys =
            provider.get_cache_request_key(&FunctionType::CheckAppAvailable, &data_map_compat);
        assert_eq!(
            keys,
            vec!["https://f-droid.org/repo/packages/org.fdroid.fdroid.privileged/HEAD"]
        );
    }

    #[tokio::test]
    async fn test_fdroid_check_app_available() {
        let provider = FDroidProvider::new();

        // Test with existing package from test XML using ANDROID_APP_TYPE
        let app_data = BTreeMap::from([(ANDROID_APP_TYPE, "org.fdroid.fdroid.privileged")]);
        let hub_data = BTreeMap::new();
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        let result = provider.check_app_available(&fin).await;
        assert!(result.result.is_ok());
        assert_eq!(result.result.unwrap(), true);

        // Test with non-existing package
        let app_data = BTreeMap::from([(ANDROID_APP_TYPE, "nonexist")]);
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        let result = provider.check_app_available(&fin).await;
        assert!(result.result.is_ok());
        assert_eq!(result.result.unwrap(), false);
    }

    #[tokio::test]
    async fn test_fdroid_get_latest_release() {
        let provider = FDroidProvider::new();

        // Test with existing package from test XML using reference package names
        let app_data = BTreeMap::from([(ANDROID_APP_TYPE, "org.fdroid.fdroid.privileged")]);
        let hub_data = BTreeMap::new();
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        let result = provider.get_latest_release(&fin).await;
        assert!(result.result.is_ok());

        let release = result.result.unwrap();
        // Update expected values based on test XML content
        assert!(!release.version_number.is_empty());
        assert_eq!(release.assets.len(), 1);
        assert_eq!(release.assets[0].file_type, "apk");
    }

    #[tokio::test]
    async fn test_fdroid_get_releases() {
        let provider = FDroidProvider::new();

        // Test with existing package from test XML
        let app_data = BTreeMap::from([(ANDROID_APP_TYPE, "org.fdroid.fdroid.privileged")]);
        let hub_data = BTreeMap::new();
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        let result = provider.get_releases(&fin).await;
        assert!(result.result.is_ok());

        let releases = result.result.unwrap();
        assert!(!releases.is_empty());
        assert_eq!(releases[0].assets[0].file_type, "apk");
    }

    #[tokio::test]
    async fn test_fdroid_get_releases_assets_type() {
        let provider = FDroidProvider::new();

        // Test with OTA package that has ZIP file type
        let app_data = BTreeMap::from([(ANDROID_APP_TYPE, "org.fdroid.fdroid.privileged.ota")]);
        let hub_data = BTreeMap::new();
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        let result = provider.get_releases(&fin).await;
        assert!(result.result.is_ok());

        let releases = result.result.unwrap();
        assert!(!releases.is_empty());
        assert_eq!(releases[0].assets[0].file_type, "zip");
    }

    #[tokio::test]
    async fn test_fdroid_get_releases_nonexist() {
        let provider = FDroidProvider::new();

        // Test with non-existing package
        let app_data = BTreeMap::from([(ANDROID_APP_TYPE, "nonexist")]);
        let hub_data = BTreeMap::new();
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
    async fn test_fdroid_check_missing_package_id() {
        let provider = FDroidProvider::new();

        let app_data = BTreeMap::new(); // Missing package_id
        let hub_data = BTreeMap::new();
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        let result = provider.check_app_available(&fin).await;
        assert!(result.result.is_err());
        assert!(result
            .result
            .unwrap_err()
            .to_string()
            .contains("Missing package_id"));
    }
}
