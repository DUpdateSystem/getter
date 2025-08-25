#[cfg(feature = "concurrent")]
use crate::CacheBackend;
use async_trait::async_trait;
use std::collections::HashMap;
use std::error::Error;
use tokio::sync::RwLock;

pub struct ConcurrentCache {
    data: RwLock<HashMap<String, String>>,
}

impl Default for ConcurrentCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ConcurrentCache {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl CacheBackend for ConcurrentCache {
    async fn get(&self, key: &str) -> Result<Option<String>, Box<dyn Error>> {
        let data = self.data.read().await;
        Ok(data.get(key).cloned())
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), Box<dyn Error>> {
        let mut data = self.data.write().await;
        data.insert(key.to_string(), value.to_string());
        Ok(())
    }

    async fn remove(&self, key: &str) -> Result<(), Box<dyn Error>> {
        let mut data = self.data.write().await;
        data.remove(key);
        Ok(())
    }

    async fn clear(&self) -> Result<(), Box<dyn Error>> {
        let mut data = self.data.write().await;
        data.clear();
        Ok(())
    }
}
