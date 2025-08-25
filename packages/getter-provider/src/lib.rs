pub mod data;
pub mod base_provider;
pub mod github;

// Re-export common types
pub use data::{ReleaseData, AssetData};
pub use base_provider::{
    BaseProvider, BaseProviderExt, DataMap, FIn, FOut, FunctionType, 
    HubDataMap, AppDataMap, CacheMap,
    ANDROID_APP_TYPE, ANDROID_MAGISK_MODULE_TYPE, ANDROID_CUSTOM_SHELL, 
    ANDROID_CUSTOM_SHELL_ROOT, KEY_REPO_URL, KEY_REPO_API_URL, REVERSE_PROXY
};
pub use github::GitHubProvider;

use std::collections::HashMap;
use std::error::Error;

pub struct ProviderManager {
    providers: HashMap<String, Box<dyn BaseProvider>>,
}

impl ProviderManager {
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    pub fn register_provider(&mut self, provider: Box<dyn BaseProvider>) {
        self.providers.insert(provider.get_friendly_name().to_string(), provider);
    }

    pub fn get_provider(&self, name: &str) -> Option<&dyn BaseProvider> {
        self.providers.get(name).map(|p| p.as_ref())
    }

    pub async fn check_app_available(&self, provider_name: &str, fin: &FIn<'_>) -> Result<bool, Box<dyn Error + Send + Sync>> {
        if let Some(provider) = self.providers.get(provider_name) {
            provider.check_app_available(fin).await.result
        } else {
            Err(format!("Provider '{}' not found", provider_name).into())
        }
    }

    pub async fn get_latest_release(&self, provider_name: &str, fin: &FIn<'_>) -> Result<ReleaseData, Box<dyn Error + Send + Sync>> {
        if let Some(provider) = self.providers.get(provider_name) {
            provider.get_latest_release(fin).await.result
        } else {
            Err(format!("Provider '{}' not found", provider_name).into())
        }
    }

    pub async fn get_releases(&self, provider_name: &str, fin: &FIn<'_>) -> Result<Vec<ReleaseData>, Box<dyn Error + Send + Sync>> {
        if let Some(provider) = self.providers.get(provider_name) {
            provider.get_releases(fin).await.result
        } else {
            Err(format!("Provider '{}' not found", provider_name).into())
        }
    }
}