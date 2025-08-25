pub mod manager;
pub mod local;
pub mod legacy_manager;

#[cfg(feature = "concurrent")]
pub mod concurrent;

use async_trait::async_trait;
use std::error::Error;

// New cache backend trait
#[async_trait]
pub trait CacheBackend: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<String>, Box<dyn Error>>;
    async fn set(&self, key: &str, value: &str) -> Result<(), Box<dyn Error>>;
    async fn remove(&self, key: &str) -> Result<(), Box<dyn Error>>;
    async fn clear(&self) -> Result<(), Box<dyn Error>>;
}

pub struct CacheManager {
    backend: Box<dyn CacheBackend>,
}

impl CacheManager {
    pub fn new(backend: Box<dyn CacheBackend>) -> Self {
        Self { backend }
    }

    pub async fn get(&self, key: &str) -> Result<Option<String>, Box<dyn Error>> {
        self.backend.get(key).await
    }

    pub async fn set(&self, key: &str, value: &str) -> Result<(), Box<dyn Error>> {
        self.backend.set(key, value).await
    }

    pub async fn remove(&self, key: &str) -> Result<(), Box<dyn Error>> {
        self.backend.remove(key).await
    }

    pub async fn clear(&self) -> Result<(), Box<dyn Error>> {
        self.backend.clear().await
    }
}

// Re-export legacy API for compatibility
pub use legacy_manager::{
    LegacyCacheManager, GroupType, LocalCacheItem,
    init_cache_manager, init_cache_manager_with_expire, get_cache_manager
};