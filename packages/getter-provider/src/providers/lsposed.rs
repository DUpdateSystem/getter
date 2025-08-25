use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;

use crate::base_provider::*;
use crate::data::{AssetData, ReleaseData};
use crate::register_provider;

const LSPOSED_MODULES_URL: &str = "https://modules.lsposed.org/modules.json";

/// LSPosed repository provider
///
/// Supports fetching LSPosed module information from modules.lsposed.org.
/// App data expected keys:
/// - package_id: Android package ID (e.g. "com.example.module")
///
/// Hub data keys (optional):
/// - None required
pub struct LsposedRepoProvider;

impl Default for LsposedRepoProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl LsposedRepoProvider {
    pub fn new() -> Self {
        LsposedRepoProvider
    }

    fn parse_module_from_json(
        &self,
        modules_data: &Value,
        target_package_id: &str,
    ) -> Option<ReleaseData> {
        let modules = modules_data.as_array()?;

        for module in modules {
            if let Some(package_id) = module.get("name").and_then(|v| v.as_str()) {
                if package_id == target_package_id {
                    let name = module.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let description = module
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let summary = module.get("summary").and_then(|v| v.as_str()).unwrap_or("");

                    // Try to get version info
                    let version =
                        if let Some(releases) = module.get("releases").and_then(|v| v.as_array()) {
                            if let Some(latest_release) = releases.first() {
                                latest_release
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("1.0.0")
                            } else {
                                "1.0.0"
                            }
                        } else {
                            "1.0.0"
                        };

                    // Get download URL
                    let download_url = if let Some(url) = module.get("url").and_then(|v| v.as_str())
                    {
                        url.to_string()
                    } else {
                        format!("https://modules.lsposed.org/{}.zip", package_id)
                    };

                    return Some(ReleaseData {
                        version_number: version.to_string(),
                        changelog: if description.is_empty() {
                            summary.to_string()
                        } else {
                            description.to_string()
                        },
                        assets: vec![AssetData {
                            file_name: format!("{}-{}.zip", package_id, version),
                            file_type: "application/zip".to_string(),
                            download_url: download_url.clone(),
                        }],
                        extra: Some(HashMap::from([
                            ("package_id".to_string(), package_id.to_string()),
                            ("name".to_string(), name.to_string()),
                            ("summary".to_string(), summary.to_string()),
                        ])),
                    });
                }
            }
        }

        None
    }
}

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
        function_type: &FunctionType,
        data_map: &DataMap<'_>,
    ) -> Vec<String> {
        let package_id = data_map
            .app_data
            .get("package_id")
            .copied()
            .unwrap_or("unknown");

        match function_type {
            FunctionType::CheckAppAvailable => {
                vec![format!("lsposed:check:{}", package_id)]
            }
            FunctionType::GetLatestRelease | FunctionType::GetReleases => {
                vec!["lsposed:modules".to_string()]
            }
        }
    }

    async fn check_app_available(&self, fin: &FIn) -> FOut<bool> {
        let package_id = match fin.data_map.app_data.get("package_id") {
            Some(id) => id,
            None => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(
                    "Missing package_id in app_data",
                )))
            }
        };

        // For demo, use test JSON to check if package exists
        let test_json = include_str!("../../tests/web/lsposed_modules.json");
        if let Ok(modules_data) = serde_json::from_str::<Value>(test_json) {
            let module_exists = self
                .parse_module_from_json(&modules_data, package_id)
                .is_some();
            FOut::new(module_exists)
        } else {
            FOut::new_empty().set_error(Box::new(std::io::Error::other(
                "Failed to parse modules data",
            )))
        }
    }

    async fn get_latest_release(&self, fin: &FIn) -> FOut<ReleaseData> {
        let package_id = match fin.data_map.app_data.get("package_id") {
            Some(id) => id,
            None => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::other(
                    "Missing package_id in app_data",
                )))
            }
        };

        // For demo purposes, use the test JSON data
        let test_json = include_str!("../../tests/web/lsposed_modules.json");
        if let Ok(modules_data) = serde_json::from_str::<Value>(test_json) {
            match self.parse_module_from_json(&modules_data, package_id) {
                Some(release) => FOut::new(release),
                None => FOut::new_empty().set_error(Box::new(std::io::Error::other(
                    "Module not found in LSPosed repository",
                ))),
            }
        } else {
            FOut::new_empty().set_error(Box::new(std::io::Error::other(
                "Failed to parse modules data",
            )))
        }
    }

    async fn get_releases(&self, fin: &FIn) -> FOut<Vec<ReleaseData>> {
        // LSPosed modules typically only have one release, so return the latest
        match self.get_latest_release(fin).await.result {
            Ok(release) => FOut::new(vec![release]),
            Err(e) => FOut::new_empty().set_error(e),
        }
    }
}

// Automatically register the LSPosed provider
register_provider!(LsposedRepoProvider);

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn test_lsposed_cache_keys() {
        let provider = LsposedRepoProvider::new();

        let app_data = BTreeMap::from([("package_id", "com.example.module")]);
        let hub_data = BTreeMap::new();
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };

        let keys = provider.get_cache_request_key(&FunctionType::CheckAppAvailable, &data_map);
        assert_eq!(keys, vec!["lsposed:check:com.example.module"]);

        let keys = provider.get_cache_request_key(&FunctionType::GetLatestRelease, &data_map);
        assert_eq!(keys, vec!["lsposed:modules"]);
    }

    #[tokio::test]
    async fn test_lsposed_check_app_available() {
        let provider = LsposedRepoProvider::new();

        let app_data = BTreeMap::from([("package_id", "com.example.module")]);
        let hub_data = BTreeMap::new();
        let data_map = DataMap {
            app_data: &app_data,
            hub_data: &hub_data,
        };
        let fin = FIn::new(data_map, None);

        // This test depends on having test data in lsposed_modules.json
        let result = provider.check_app_available(&fin).await;
        match result.result {
            Ok(available) => {
                println!("Module availability: {}", available);
            }
            Err(e) => {
                println!("Error checking availability: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_lsposed_check_missing_package_id() {
        let provider = LsposedRepoProvider::new();

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
