use bytes::Bytes;

use super::local::LocalCacheItem;
use crate::utils::time::get_now_unix;

#[derive(Debug, Eq, Hash, PartialEq)]
pub enum GroupType {
    #[allow(non_camel_case_types)]
    REPO_INSIDE,
    API,
}

pub struct CacheManager {
    local_cache_path: String,
    global_expire_time: Option<u64>,
}

impl CacheManager {
    pub fn new(local_cache_path: &str, global_expire_time: Option<u64>) -> Self {
        Self {
            local_cache_path: local_cache_path.to_string(),
            global_expire_time,
        }
    }

    fn get_local_cache_key(group: &GroupType, key: &str) -> String {
        format!("{:?}_{}", group, key)
    }

    async fn get_local(
        &self,
        group: &GroupType,
        key: &str,
        expire_time: Option<u64>,
    ) -> Option<Bytes> {
        let local_cache_key = Self::get_local_cache_key(group, key);
        let local_cache_item = LocalCacheItem::new(&self.local_cache_path);
        if let Ok(time) = local_cache_item.get_cache_time(&local_cache_key).await {
            if let Some(expire_time) = expire_time.or(self.global_expire_time) {
                if time + expire_time < get_now_unix() {
                    return None;
                }
            }
            if let Some(data) = local_cache_item
                .get(&local_cache_key, |data| data)
                .await
                .ok()
            {
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
        if let Some(data) = self.get_local(group, key, expire_time).await {
            return Some(data);
        } else {
            None
        }
    }

    pub async fn save(
        &mut self,
        group: &GroupType,
        key: &str,
        value: Bytes,
    ) -> Result<(), std::io::Error> {
        let local_cache_key = Self::get_local_cache_key(group, key);
        let local_cache_item = LocalCacheItem::new(&self.local_cache_path);
        local_cache_item
            .save(&local_cache_key, value, |data| data.into())
            .await
    }

    #[allow(dead_code)]
    pub async fn remove(&mut self, group: &GroupType, key: &str) -> Result<(), std::io::Error> {
        let local_cache_key = Self::get_local_cache_key(group, key);
        let local_cache_item = LocalCacheItem::new(&self.local_cache_path);
        local_cache_item.remove(&local_cache_key).await
    }

    #[allow(dead_code)]
    pub async fn clean(&mut self) -> Result<(), std::io::Error> {
        let local_cache_item = LocalCacheItem::new(&self.local_cache_path);
        local_cache_item.clean().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_manager() {
        let mut cache_manager = CacheManager::new("./test_cache_manager", None);
        let group = GroupType::REPO_INSIDE;
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

    #[tokio::test]
    async fn test_cache_manager_restart() {
        let mut cache_manager = CacheManager::new("./test_cache_manager_restart", None);
        let group = GroupType::REPO_INSIDE;
        let key = "test_key";
        let value = Bytes::from("test_value");
        let _ = cache_manager.remove(&group, key).await;
        cache_manager
            .save(&group, key, value.clone())
            .await
            .expect("save failed");
        let _cache_manager = CacheManager::new("./test_cache_manager_restart", None);
        let data = _cache_manager
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

    #[tokio::test]
    async fn test_cache_manager_no_exist() {
        let mut cache_manager = CacheManager::new("./test_cache_manager_no_exist", None);
        let group = GroupType::REPO_INSIDE;
        let key = "test_key_no_exist";
        let data = cache_manager.get(&group, key, None).await;
        assert_eq!(data, None);
        let clean_result = cache_manager.clean().await;
        assert!(clean_result.is_err());
    }

    #[tokio::test]
    async fn test_cache_manager_expire_non_global() {
        let mut cache_manager = CacheManager::new("./test_cache_manager_expire_non_global", None);
        let group = GroupType::REPO_INSIDE;
        let key = "test_key_expire";
        let value = Bytes::from("test_value_expire");
        let _ = cache_manager.remove(&group, key).await;
        cache_manager
            .save(&group, key, value.clone())
            .await
            .expect("save failed");
        // sleep 1 second
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        let data = cache_manager.get(&group, key, Some(0)).await;
        assert_eq!(data, None);
        let data = cache_manager
            .get(&group, key, Some(100))
            .await
            .expect("get failed");
        assert_eq!(data, value);
        cache_manager
            .remove(&group, key)
            .await
            .expect("remove failed");
        cache_manager.clean().await.expect("clean failed");
    }

    #[tokio::test]
    async fn test_cache_manager_expire_non_expire() {
        let mut cache_manager = CacheManager::new("./test_cache_manager_non_expire", None);
        let group = GroupType::REPO_INSIDE;
        let key = "test_key_expire";
        let value = Bytes::from("test_value_expire");
        let _ = cache_manager.remove(&group, key).await;
        cache_manager
            .save(&group, key, value.clone())
            .await
            .expect("save failed");
        // sleep 1 second
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        let data = cache_manager.get(&group, key, Some(0)).await;
        assert_eq!(data, None);
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


    #[tokio::test]
    async fn test_cache_manager_global_expire() {
        let mut cache_manager = CacheManager::new("./test_cache_manager_global_expire", Some(1));
        let group = GroupType::REPO_INSIDE;
        let key = "test_key_expire";
        let value = Bytes::from("test_value_expire");
        let _ = cache_manager.remove(&group, key).await;
        cache_manager
            .save(&group, key, value.clone())
            .await
            .expect("save failed");
        // sleep 1 second
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        let data = cache_manager.get(&group, key, Some(0)).await;
        assert_eq!(data, None);
        let data = cache_manager
            .get(&group, key, None)
            .await;
        assert_eq!(data, None);
        let data = cache_manager
            .get(&group, key, Some(100))
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
