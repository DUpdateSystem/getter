pub mod base_provider;
pub mod data;
pub mod providers;
pub mod registry;

// Re-export common types
pub use base_provider::{
    AppDataMap, BaseProvider, BaseProviderExt, CacheMap, DataMap, FIn, FOut, FunctionType,
    HubDataMap, ANDROID_APP_TYPE, ANDROID_CUSTOM_SHELL, ANDROID_CUSTOM_SHELL_ROOT,
    ANDROID_MAGISK_MODULE_TYPE, KEY_REPO_API_URL, KEY_REPO_URL, REVERSE_PROXY,
};
pub use data::{AssetData, ReleaseData};
pub use providers::*;

use std::collections::HashMap;
use std::error::Error;

pub struct ProviderManager {
    providers: HashMap<String, Box<dyn BaseProvider>>,
}

impl Default for ProviderManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderManager {
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    /// Create a new ProviderManager with all automatically registered providers
    pub fn with_auto_registered() -> Self {
        let providers = crate::registry::ProviderRegistry::global()
            .lock()
            .unwrap()
            .create_all();
        Self { providers }
    }

    pub fn register_provider(&mut self, provider: Box<dyn BaseProvider>) {
        self.providers
            .insert(provider.get_friendly_name().to_string(), provider);
    }

    pub fn get_provider(&self, name: &str) -> Option<&dyn BaseProvider> {
        self.providers.get(name).map(|p| p.as_ref())
    }

    /// Get the list of registered provider names (for testing)
    pub fn provider_names(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    pub async fn check_app_available(
        &self,
        provider_name: &str,
        fin: &FIn<'_>,
    ) -> Result<bool, Box<dyn Error + Send + Sync>> {
        if let Some(provider) = self.providers.get(provider_name) {
            provider.check_app_available(fin).await.result
        } else {
            Err(format!("Provider '{}' not found", provider_name).into())
        }
    }

    pub async fn get_latest_release(
        &self,
        provider_name: &str,
        fin: &FIn<'_>,
    ) -> Result<ReleaseData, Box<dyn Error + Send + Sync>> {
        if let Some(provider) = self.providers.get(provider_name) {
            provider.get_latest_release(fin).await.result
        } else {
            Err(format!("Provider '{}' not found", provider_name).into())
        }
    }

    pub async fn get_releases(
        &self,
        provider_name: &str,
        fin: &FIn<'_>,
    ) -> Result<Vec<ReleaseData>, Box<dyn Error + Send + Sync>> {
        if let Some(provider) = self.providers.get(provider_name) {
            provider.get_releases(fin).await.result
        } else {
            Err(format!("Provider '{}' not found", provider_name).into())
        }
    }
}
