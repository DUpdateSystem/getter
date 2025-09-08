use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::error::Error;

use crate::base_provider::{AppDataMap, BaseProvider, DataMap, FIn, FOut, FunctionType};
use crate::data::{AssetData, ReleaseData};

const ANDROID_APP_TYPE: &str = "android_app";
const ANDROID_MAGISK_MODULE_TYPE: &str = "magisk_module";

/// Android App Provider - interfaces with Android PackageManager
#[derive(Debug, Clone)]
pub struct AndroidAppProvider {
    name: String,
    jni_callbacks: Option<AndroidJniCallbacks>,
}

/// JNI callbacks for Android-specific operations
#[derive(Debug, Clone)]
pub struct AndroidJniCallbacks {
    // These would be function pointers to JNI methods
    // For now, we'll use placeholder implementations
}

impl AndroidAppProvider {
    pub fn new() -> Self {
        Self {
            name: "Android Apps".to_string(),
            jni_callbacks: None,
        }
    }

    pub fn with_callbacks(mut self, callbacks: AndroidJniCallbacks) -> Self {
        self.jni_callbacks = Some(callbacks);
        self
    }

    fn get_package_name(app_data: &AppDataMap) -> Option<String> {
        app_data.get(ANDROID_APP_TYPE).map(|s| s.to_string())
    }
}

impl Default for AndroidAppProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BaseProvider for AndroidAppProvider {
    fn get_uuid(&self) -> &'static str {
        "android_app_provider"
    }

    fn get_friendly_name(&self) -> &'static str {
        "Android App Provider"
    }

    fn get_cache_request_key(
        &self,
        function_type: &FunctionType,
        data_map: &DataMap,
    ) -> Vec<String> {
        let package_name =
            Self::get_package_name(&data_map.app_data).unwrap_or_else(|| "unknown".to_string());
        match function_type {
            FunctionType::CheckAppAvailable => vec![format!("android:check:{}", package_name)],
            FunctionType::GetLatestRelease => vec![format!("android:latest:{}", package_name)],
            FunctionType::GetReleases => vec![format!("android:releases:{}", package_name)],
        }
    }

    async fn check_app_available(&self, fin: &FIn) -> FOut<bool> {
        let package_name = match Self::get_package_name(&fin.data_map.app_data) {
            Some(name) => name,
            None => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "No Android package name provided",
                )))
            }
        };

        // Check if app is installed via JNI callback
        // For now, return a placeholder
        FOut::new(true)
    }

    async fn get_latest_release(&self, fin: &FIn) -> FOut<ReleaseData> {
        let package_name = match Self::get_package_name(&fin.data_map.app_data) {
            Some(name) => name,
            None => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "No Android package name provided",
                )))
            }
        };

        // Get app version from PackageManager via JNI
        // For now, return placeholder data
        let release = ReleaseData {
            version_number: "1.0.0".to_string(),
            changelog: String::new(),
            assets: vec![],
            extra: Some(HashMap::new()),
        };

        FOut::new(release)
    }

    async fn get_releases(&self, fin: &FIn) -> FOut<Vec<ReleaseData>> {
        // Android apps typically only have the current installed version
        let latest = self.get_latest_release(fin).await;
        match latest.result {
            Ok(release) => FOut::new(vec![release]),
            Err(e) => FOut::new_empty().set_error(e),
        }
    }
}

/// Magisk Module Provider
#[derive(Debug, Clone)]
pub struct MagiskModuleProvider {
    name: String,
    module_repo_urls: Vec<String>,
}

impl MagiskModuleProvider {
    pub fn new() -> Self {
        Self {
            name: "Magisk Modules".to_string(),
            module_repo_urls: vec![
                "https://github.com/Magisk-Modules-Repo".to_string(),
                "https://github.com/Magisk-Modules-Alt-Repo".to_string(),
            ],
        }
    }

    fn get_module_id(app_data: &AppDataMap) -> Option<String> {
        app_data
            .get(ANDROID_MAGISK_MODULE_TYPE)
            .map(|s| s.to_string())
    }

    async fn fetch_module_info(
        &self,
        module_id: &str,
    ) -> Result<ReleaseData, Box<dyn Error + Send + Sync>> {
        // Fetch module info from repository
        // This would make HTTP requests to the module repos
        Ok(ReleaseData {
            version_number: "1.0.0".to_string(),
            changelog: "Module update".to_string(),
            assets: vec![AssetData {
                file_name: format!("{}.zip", module_id),
                file_type: "zip".to_string(),
                download_url: format!(
                    "https://github.com/Magisk-Modules-Repo/{}/releases/latest/download/{}.zip",
                    module_id, module_id
                ),
            }],
            extra: Some(HashMap::new()),
        })
    }
}

impl Default for MagiskModuleProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BaseProvider for MagiskModuleProvider {
    fn get_uuid(&self) -> &'static str {
        "magisk_module_provider"
    }

    fn get_friendly_name(&self) -> &'static str {
        "Magisk Module Provider"
    }

    fn get_cache_request_key(
        &self,
        function_type: &FunctionType,
        data_map: &DataMap,
    ) -> Vec<String> {
        let module_id =
            Self::get_module_id(&data_map.app_data).unwrap_or_else(|| "unknown".to_string());
        match function_type {
            FunctionType::CheckAppAvailable => vec![format!("magisk:check:{}", module_id)],
            FunctionType::GetLatestRelease => vec![format!("magisk:latest:{}", module_id)],
            FunctionType::GetReleases => vec![format!("magisk:releases:{}", module_id)],
        }
    }

    async fn check_app_available(&self, fin: &FIn) -> FOut<bool> {
        let _module_id = match Self::get_module_id(&fin.data_map.app_data) {
            Some(id) => id,
            None => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "No Magisk module ID provided",
                )))
            }
        };

        // Check if module exists in repos
        // For now, return true
        FOut::new(true)
    }

    async fn get_latest_release(&self, fin: &FIn) -> FOut<ReleaseData> {
        let module_id = match Self::get_module_id(&fin.data_map.app_data) {
            Some(id) => id,
            None => {
                return FOut::new_empty().set_error(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "No Magisk module ID provided",
                )))
            }
        };

        match self.fetch_module_info(&module_id).await {
            Ok(release) => FOut::new(release),
            Err(e) => FOut::new_empty().set_error(e),
        }
    }

    async fn get_releases(&self, fin: &FIn) -> FOut<Vec<ReleaseData>> {
        // Fetch all available versions from the module repo
        let latest = self.get_latest_release(fin).await;
        match latest.result {
            Ok(release) => FOut::new(vec![release]),
            Err(e) => FOut::new_empty().set_error(e),
        }
    }
}

// Register providers
pub fn register_android_providers() {
    use crate::registry::ProviderRegistry;

    ProviderRegistry::global()
        .lock()
        .unwrap()
        .register::<AndroidAppProvider>();

    ProviderRegistry::global()
        .lock()
        .unwrap()
        .register::<MagiskModuleProvider>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_android_app_provider() {
        let provider = AndroidAppProvider::new();

        assert_eq!(provider.get_uuid(), "android_app_provider");
        assert_eq!(provider.get_friendly_name(), "Android App Provider");
    }

    #[tokio::test]
    async fn test_magisk_module_provider() {
        let provider = MagiskModuleProvider::new();

        assert_eq!(provider.get_uuid(), "magisk_module_provider");
        assert_eq!(provider.get_friendly_name(), "Magisk Module Provider");
    }

    #[tokio::test]
    async fn test_check_app_without_package_name() {
        let provider = AndroidAppProvider::new();
        let data_map = DataMap {
            app_data: AppDataMap::new(),
            hub_data: Default::default(),
        };
        let fin = FIn { data_map };

        let result = provider.check_app_available(&fin).await;
        assert!(result.result.is_err());
    }

    #[tokio::test]
    async fn test_check_app_with_package_name() {
        let provider = AndroidAppProvider::new();
        let mut app_data = AppDataMap::new();
        app_data.insert(ANDROID_APP_TYPE.to_string(), "com.example.app".to_string());

        let data_map = DataMap {
            app_data,
            hub_data: Default::default(),
        };
        let fin = FIn { data_map };

        let result = provider.check_app_available(&fin).await;
        assert!(result.result.is_ok());
    }
}
