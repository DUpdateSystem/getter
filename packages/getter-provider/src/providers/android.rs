use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::error::Error;

use crate::base_provider::{
    AppDataMap, BaseProvider, DataMap, FIn, FOut, FunctionType, HubDataMap, ANDROID_APP_TYPE,
    ANDROID_MAGISK_MODULE_TYPE,
};
use crate::data::{AssetData, ReleaseData};

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
        app_data.get(ANDROID_APP_TYPE).cloned()
    }
}

impl Default for AndroidAppProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BaseProvider for AndroidAppProvider {
    fn get_uuid(&self) -> &str {
        "android_app_provider"
    }

    fn get_friendly_name(&self) -> &str {
        &self.name
    }

    fn get_api_keywords(&self) -> Vec<&str> {
        vec![ANDROID_APP_TYPE]
    }

    async fn check_app_available(&self, fin: &FIn<'_>) -> FOut<bool> {
        let package_name = match Self::get_package_name(&fin.app_data) {
            Some(name) => name,
            None => return FOut::error("No Android package name provided"),
        };

        // Check if app is installed via JNI callback
        // For now, return a placeholder
        FOut::success(true)
    }

    async fn get_latest_release(&self, fin: &FIn<'_>) -> FOut<ReleaseData> {
        let package_name = match Self::get_package_name(&fin.app_data) {
            Some(name) => name,
            None => return FOut::error("No Android package name provided"),
        };

        // Get app version from PackageManager via JNI
        // For now, return placeholder data
        let release = ReleaseData {
            version_number: "1.0.0".to_string(),
            change_log: None,
            assets: vec![],
            extra: HashMap::new(),
        };

        FOut::success(release)
    }

    async fn get_releases(&self, fin: &FIn<'_>) -> FOut<Vec<ReleaseData>> {
        // Android apps typically only have the current installed version
        let latest = self.get_latest_release(fin).await;
        match latest.result {
            Ok(release) => FOut::success(vec![release]),
            Err(e) => FOut::error_boxed(e),
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
        app_data.get(ANDROID_MAGISK_MODULE_TYPE).cloned()
    }

    async fn fetch_module_info(
        &self,
        module_id: &str,
    ) -> Result<ReleaseData, Box<dyn Error + Send + Sync>> {
        // Fetch module info from repository
        // This would make HTTP requests to the module repos
        Ok(ReleaseData {
            version_number: "1.0.0".to_string(),
            change_log: Some("Module update".to_string()),
            assets: vec![AssetData {
                name: format!("{}.zip", module_id),
                download_url: format!(
                    "https://github.com/Magisk-Modules-Repo/{}/releases/latest/download/{}.zip",
                    module_id, module_id
                ),
                extra: HashMap::new(),
            }],
            extra: HashMap::new(),
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
    fn get_uuid(&self) -> &str {
        "magisk_module_provider"
    }

    fn get_friendly_name(&self) -> &str {
        &self.name
    }

    fn get_api_keywords(&self) -> Vec<&str> {
        vec![ANDROID_MAGISK_MODULE_TYPE]
    }

    async fn check_app_available(&self, fin: &FIn<'_>) -> FOut<bool> {
        let module_id = match Self::get_module_id(&fin.app_data) {
            Some(id) => id,
            None => return FOut::error("No Magisk module ID provided"),
        };

        // Check if module exists in repos
        // For now, return true
        FOut::success(true)
    }

    async fn get_latest_release(&self, fin: &FIn<'_>) -> FOut<ReleaseData> {
        let module_id = match Self::get_module_id(&fin.app_data) {
            Some(id) => id,
            None => return FOut::error("No Magisk module ID provided"),
        };

        match self.fetch_module_info(&module_id).await {
            Ok(release) => FOut::success(release),
            Err(e) => FOut::error_boxed(e),
        }
    }

    async fn get_releases(&self, fin: &FIn<'_>) -> FOut<Vec<ReleaseData>> {
        // Fetch all available versions from the module repo
        let latest = self.get_latest_release(fin).await;
        match latest.result {
            Ok(release) => FOut::success(vec![release]),
            Err(e) => FOut::error_boxed(e),
        }
    }
}

// Register providers
pub fn register_android_providers() {
    use crate::registry::ProviderRegistry;

    ProviderRegistry::global()
        .lock()
        .unwrap()
        .register("AndroidAppProvider", || Box::new(AndroidAppProvider::new()));

    ProviderRegistry::global()
        .lock()
        .unwrap()
        .register("MagiskModuleProvider", || {
            Box::new(MagiskModuleProvider::new())
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_android_app_provider() {
        let provider = AndroidAppProvider::new();

        assert_eq!(provider.get_uuid(), "android_app_provider");
        assert_eq!(provider.get_friendly_name(), "Android Apps");
        assert!(provider.get_api_keywords().contains(&ANDROID_APP_TYPE));
    }

    #[tokio::test]
    async fn test_magisk_module_provider() {
        let provider = MagiskModuleProvider::new();

        assert_eq!(provider.get_uuid(), "magisk_module_provider");
        assert_eq!(provider.get_friendly_name(), "Magisk Modules");
        assert!(provider
            .get_api_keywords()
            .contains(&ANDROID_MAGISK_MODULE_TYPE));
    }

    #[tokio::test]
    async fn test_check_app_without_package_name() {
        let provider = AndroidAppProvider::new();
        let fin = FIn {
            app_data: AppDataMap::new(),
            hub_data: HubDataMap::new(),
            other_data: DataMap::new(),
        };

        let result = provider.check_app_available(&fin).await;
        assert!(result.result.is_err());
    }

    #[tokio::test]
    async fn test_check_app_with_package_name() {
        let provider = AndroidAppProvider::new();
        let mut app_data = AppDataMap::new();
        app_data.insert(ANDROID_APP_TYPE.to_string(), "com.example.app".to_string());

        let fin = FIn {
            app_data,
            hub_data: HubDataMap::new(),
            other_data: DataMap::new(),
        };

        let result = provider.check_app_available(&fin).await;
        assert!(result.result.is_ok());
    }
}
