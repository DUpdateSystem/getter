use std::path::{Path, PathBuf};
use bytes::Bytes;
use super::local::LocalCache;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Eq, Hash, PartialEq)]
pub enum GroupType {
    RepoInside,
    Api,
}

pub struct LegacyCacheManager {
    local_cache_dir: Option<PathBuf>,
    global_expire_time: Option<u64>,
    local_cache: Arc<RwLock<HashMap<String, LocalCacheItem>>>,
}

pub struct LocalCacheItem {
    cache_path: PathBuf,
}

impl LocalCacheItem {
    pub fn new(cache_dir: &Path, key: &str) -> Self {
        Self {
            cache_path: cache_dir.join(key),
        }
    }

    pub async fn get<T>(&self, decoder: fn(Vec<u8>) -> T) -> Result<T, std::io::Error> {
        tokio::fs::read(&self.cache_path).await.map(decoder)
    }

    pub async fn save<T>(&self, data: T, encoder: fn(T) -> Vec<u8>) -> Result<(), std::io::Error> {
        let parent = self.cache_path.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "cache path not found")
        })?;
        tokio::fs::create_dir_all(parent).await?;
        tokio::fs::write(&self.cache_path, encoder(data)).await
    }

    pub async fn remove(&self) -> Result<(), std::io::Error> {
        tokio::fs::remove_file(&self.cache_path).await
    }

    pub async fn get_cache_time(&self) -> Result<u64, std::io::Error> {
        let metadata = tokio::fs::metadata(&self.cache_path).await?;
        let modified_time = metadata.modified()?;
        Ok(modified_time
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs())
    }
}

use std::collections::HashMap;

impl LegacyCacheManager {
    pub fn new() -> Self {
        Self {
            local_cache_dir: None,
            global_expire_time: None,
            local_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn set_local_cache_dir(&mut self, local_cache_dir: &Path) -> &mut Self {
        self.local_cache_dir = Some(local_cache_dir.to_path_buf());
        self
    }

    pub fn set_global_expire_time(&mut self, global_expire_time: u64) -> &mut Self {
        self.global_expire_time = Some(global_expire_time);
        self
    }

    fn get_local_cache_key(group: &GroupType, key: &str) -> String {
        format!("{:?}_{}", group, key)
    }

    fn get_now_unix() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    async fn get_local(
        &self,
        group: &GroupType,
        key: &str,
        expire_time: Option<u64>,
    ) -> Option<Bytes> {
        let local_cache_item = self.get_local_cache_item(group, key).ok()?;
        if let Ok(time) = local_cache_item.get_cache_time().await {
            if let Some(expire_time) = expire_time.or(self.global_expire_time) {
                if time + expire_time < Self::get_now_unix() {
                    return None;
                }
            }
            if let Ok(data) = local_cache_item.get(|data| data).await {
                return Some(Bytes::from(data));
            }
        }
        None
    }

    pub async fn get(
        &self,
        group: &GroupType,
        key: &str,
        expire_time: Option<u64>,
    ) -> Option<Bytes> {
        self.get_local(group, key, expire_time).await
    }

    pub async fn save(
        &mut self,
        group: &GroupType,
        key: &str,
        value: Bytes,
    ) -> Result<(), std::io::Error> {
        let local_cache_item = self.get_local_cache_item(group, key)?;
        local_cache_item.save(value, |data| data.into()).await
    }

    pub async fn remove(&mut self, group: &GroupType, key: &str) -> Result<(), std::io::Error> {
        let local_cache_item = self.get_local_cache_item(group, key)?;
        local_cache_item.remove().await
    }

    pub async fn clean(&mut self) -> Result<(), std::io::Error> {
        self.clean_local().await
    }

    async fn clean_local(&mut self) -> Result<(), std::io::Error> {
        let local_cache_dir = self.local_cache_dir.as_ref().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "local cache dir not found")
        })?;
        tokio::fs::remove_dir_all(local_cache_dir).await
    }

    fn get_local_cache_item(
        &self,
        group: &GroupType,
        key: &str,
    ) -> Result<LocalCacheItem, std::io::Error> {
        let local_cache_key = Self::get_local_cache_key(group, key);
        let local_cache_dir = self.local_cache_dir.as_ref().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "local cache dir not found")
        })?;
        Ok(LocalCacheItem::new(local_cache_dir, &local_cache_key))
    }
}

// Legacy instance container for backward compatibility
use once_cell::sync::Lazy;
use std::sync::Arc as StdArc;
use tokio::sync::Mutex;

pub struct InstanceContainer<T> {
    instance: StdArc<Mutex<Option<StdArc<Mutex<T>>>>>,
}

impl<T> InstanceContainer<T> {
    pub fn new(item: T) -> Self {
        Self {
            instance: StdArc::new(Mutex::new(Some(StdArc::new(Mutex::new(item))))),
        }
    }

    pub async fn get(&self) -> StdArc<Mutex<T>> {
        let guard = self.instance.lock().await;
        guard.as_ref().unwrap().clone()
    }
}

static INSTANCE_CONTAINER: Lazy<InstanceContainer<LegacyCacheManager>> =
    Lazy::new(|| InstanceContainer::new(LegacyCacheManager::new()));

pub async fn init_cache_manager(local_cache_dir: &Path) {
    get_cache_manager()
        .await
        .lock()
        .await
        .set_local_cache_dir(local_cache_dir);
}

pub async fn init_cache_manager_with_expire(local_cache_path: &Path, expire_time: u64) {
    get_cache_manager()
        .await
        .lock()
        .await
        .set_local_cache_dir(local_cache_path)
        .set_global_expire_time(expire_time);
}

pub async fn get_cache_manager() -> StdArc<Mutex<LegacyCacheManager>> {
    INSTANCE_CONTAINER.get().await.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_manager() {
        let mut cache_manager = LegacyCacheManager::new();
        cache_manager.set_local_cache_dir(Path::new("./test_cache_manager"));
        let group = GroupType::RepoInside;
        let key = "test_key";
        let value = Bytes::from("test_value");
        let _ = cache_manager.remove(&group, key).await;
        cache_manager
            .save(&group, key, value.clone())
            .await
            .expect("save failed");
        let data = cache_manager
            .get(&group, key, None)
            .await
            .expect("get failed");
        assert_eq!(data, value);
        cache_manager
            .remove(&group, key)
            .await
            .expect("remove failed");
        cache_manager.clean().await.expect("clean failed");
    }
}