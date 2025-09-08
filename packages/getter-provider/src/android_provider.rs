use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::base::Provider;
use crate::error::ProviderError;
use crate::types::{App, Release, ReleaseAsset};

/// Android-specific provider that interfaces with Android system through JNI
#[derive(Clone)]
pub struct AndroidProvider {
    provider_id: String,
    config: AndroidProviderConfig,
    jni_callback: Arc<Mutex<Option<AndroidJniCallback>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AndroidProviderConfig {
    pub name: String,
    pub api_keywords: Vec<String>,
    pub app_url_templates: Vec<String>,
    pub applications_mode: bool,
}

/// JNI callback interface for Android-specific operations
pub struct AndroidJniCallback {
    /// Get installed app version from Android PackageManager
    pub get_installed_version: Box<dyn Fn(&str) -> Option<String> + Send + Sync>,
    /// Get installed apps list from Android
    pub get_installed_apps: Box<dyn Fn() -> Vec<AndroidAppInfo> + Send + Sync>,
    /// Check if app is installed
    pub is_app_installed: Box<dyn Fn(&str) -> bool + Send + Sync>,
    /// Get app info from Android
    pub get_app_info: Box<dyn Fn(&str) -> Option<AndroidAppInfo> + Send + Sync>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AndroidAppInfo {
    pub package_name: String,
    pub app_name: String,
    pub version_name: String,
    pub version_code: i32,
    pub is_system_app: bool,
}

impl AndroidProvider {
    pub fn new(provider_id: String, config: AndroidProviderConfig) -> Self {
        Self {
            provider_id,
            config,
            jni_callback: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn set_jni_callback(&self, callback: AndroidJniCallback) {
        let mut cb = self.jni_callback.lock().await;
        *cb = Some(callback);
    }

    async fn get_callback(&self) -> Result<AndroidJniCallback, ProviderError> {
        let cb = self.jni_callback.lock().await;
        cb.as_ref()
            .map(|c| AndroidJniCallback {
                get_installed_version: c.get_installed_version.clone(),
                get_installed_apps: c.get_installed_apps.clone(),
                is_app_installed: c.is_app_installed.clone(),
                get_app_info: c.get_app_info.clone(),
            })
            .ok_or_else(|| ProviderError::ConfigError("JNI callback not set".to_string()))
    }

    /// Check if this provider handles Android apps
    pub fn is_android_app_provider(&self) -> bool {
        self.config.api_keywords.contains(&"android_app_package".to_string())
    }

    /// Check if this provider handles Magisk modules
    pub fn is_magisk_provider(&self) -> bool {
        self.config.api_keywords.contains(&"android_magisk_module".to_string())
    }
}

#[async_trait]
impl Provider for AndroidProvider {
    fn id(&self) -> &str {
        &self.provider_id
    }

    fn name(&self) -> &str {
        &self.config.name
    }

    async fn check_app(&self, app_id: &str) -> Result<bool, ProviderError> {
        if !self.is_android_app_provider() {
            return Ok(false);
        }

        let callback = self.get_callback().await?;
        Ok((callback.is_app_installed)(app_id))
    }

    async fn get_latest_release(&self, app: &App) -> Result<Option<Release>, ProviderError> {
        let app_id = app.id.get("android_app_package")
            .or_else(|| app.id.get("android_magisk_module"))
            .ok_or_else(|| ProviderError::InvalidApp("No Android app ID found".to_string()))?;

        let callback = self.get_callback().await?;
        
        // Get installed version
        let installed_version = (callback.get_installed_version)(app_id);
        
        // Get app info from Android
        let app_info = (callback.get_app_info)(app_id)
            .ok_or_else(|| ProviderError::AppNotFound(app_id.to_string()))?;

        // For Android apps, we check against online sources (Play Store, F-Droid, etc.)
        // This would be implemented by calling the appropriate API
        // For now, return the installed version as the latest
        Ok(installed_version.map(|version| Release {
            version: version.clone(),
            name: Some(app_info.app_name.clone()),
            description: None,
            release_date: None,
            download_urls: vec![],
            assets: vec![],
            metadata: HashMap::new(),
        }))
    }

    async fn get_releases(&self, app: &App, count: usize) -> Result<Vec<Release>, ProviderError> {
        // For Android apps, typically we only have the current installed version
        // unless we're checking against an app store API
        let latest = self.get_latest_release(app).await?;
        Ok(latest.into_iter().collect())
    }

    async fn search_app(&self, query: &str) -> Result<Vec<App>, ProviderError> {
        if !self.is_android_app_provider() {
            return Ok(vec![]);
        }

        let callback = self.get_callback().await?;
        let installed_apps = (callback.get_installed_apps)();

        let results: Vec<App> = installed_apps
            .into_iter()
            .filter(|app| {
                app.package_name.contains(query) || 
                app.app_name.to_lowercase().contains(&query.to_lowercase())
            })
            .map(|app_info| {
                let mut id = HashMap::new();
                id.insert("android_app_package".to_string(), app_info.package_name.clone());
                
                App {
                    id,
                    name: app_info.app_name,
                    description: None,
                    metadata: HashMap::new(),
                }
            })
            .collect();

        Ok(results)
    }

    async fn download_asset(&self, asset: &ReleaseAsset) -> Result<Vec<u8>, ProviderError> {
        // Android apps are typically installed through PackageManager
        // This would trigger an installation intent
        Err(ProviderError::NotSupported("Direct download not supported for Android apps".to_string()))
    }
}

/// Magisk Module Provider - extends AndroidProvider for Magisk modules
pub struct MagiskProvider {
    base: AndroidProvider,
    module_repo_url: String,
}

impl MagiskProvider {
    pub fn new(provider_id: String, config: AndroidProviderConfig, repo_url: String) -> Self {
        Self {
            base: AndroidProvider::new(provider_id, config),
            module_repo_url: repo_url,
        }
    }
}

#[async_trait]
impl Provider for MagiskProvider {
    fn id(&self) -> &str {
        self.base.id()
    }

    fn name(&self) -> &str {
        self.base.name()
    }

    async fn check_app(&self, app_id: &str) -> Result<bool, ProviderError> {
        // Check if Magisk module is installed
        // This would check /data/adb/modules/ directory
        self.base.check_app(app_id).await
    }

    async fn get_latest_release(&self, app: &App) -> Result<Option<Release>, ProviderError> {
        let module_id = app.id.get("android_magisk_module")
            .ok_or_else(|| ProviderError::InvalidApp("No Magisk module ID found".to_string()))?;

        // Check module repository for updates
        // This would fetch from the configured Magisk module repo
        // For now, delegate to base implementation
        self.base.get_latest_release(app).await
    }

    async fn get_releases(&self, app: &App, count: usize) -> Result<Vec<Release>, ProviderError> {
        self.base.get_releases(app, count).await
    }

    async fn search_app(&self, query: &str) -> Result<Vec<App>, ProviderError> {
        // Search in Magisk module repository
        self.base.search_app(query).await
    }

    async fn download_asset(&self, asset: &ReleaseAsset) -> Result<Vec<u8>, ProviderError> {
        // Download Magisk module ZIP file
        self.base.download_asset(asset).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_android_provider_creation() {
        let config = AndroidProviderConfig {
            name: "Android Apps".to_string(),
            api_keywords: vec!["android_app_package".to_string()],
            app_url_templates: vec![],
            applications_mode: true,
        };

        let provider = AndroidProvider::new("android_provider".to_string(), config);
        
        assert_eq!(provider.id(), "android_provider");
        assert_eq!(provider.name(), "Android Apps");
        assert!(provider.is_android_app_provider());
        assert!(!provider.is_magisk_provider());
    }

    #[tokio::test]
    async fn test_magisk_provider_creation() {
        let config = AndroidProviderConfig {
            name: "Magisk Modules".to_string(),
            api_keywords: vec!["android_magisk_module".to_string()],
            app_url_templates: vec![],
            applications_mode: false,
        };

        let provider = MagiskProvider::new(
            "magisk_provider".to_string(),
            config,
            "https://magisk-modules-repo.com".to_string()
        );
        
        assert_eq!(provider.id(), "magisk_provider");
        assert_eq!(provider.name(), "Magisk Modules");
    }

    #[tokio::test]
    async fn test_provider_without_callback() {
        let config = AndroidProviderConfig {
            name: "Test Provider".to_string(),
            api_keywords: vec!["android_app_package".to_string()],
            app_url_templates: vec![],
            applications_mode: true,
        };

        let provider = AndroidProvider::new("test".to_string(), config);
        
        let mut app_id = HashMap::new();
        app_id.insert("android_app_package".to_string(), "com.test.app".to_string());
        
        let app = App {
            id: app_id,
            name: "Test App".to_string(),
            description: None,
            metadata: HashMap::new(),
        };

        // Should fail without JNI callback
        let result = provider.get_latest_release(&app).await;
        assert!(result.is_err());
    }
}