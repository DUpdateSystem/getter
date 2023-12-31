pub mod convert;
pub mod item;

use bytes::Bytes;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;

use crate::provider::base_provider::{CacheMap, CacheMapBuilder};

use self::item::CacheItem;

pub struct CacheManager {
    cache_map: HashMap<GroupType, HashMap<String, CacheItem>>,
}

impl CacheManager {
    pub fn new() -> Self {
        Self {
            cache_map: HashMap::new(),
        }
    }

    pub fn get(&self, group: &GroupType, key: &str, expire_time: Option<u64>) -> Option<&Bytes> {
        if let Some(map) = self.cache_map.get(&group) {
            if let Some(item) = map.get(key) {
                if let Some(expire_time) = expire_time {
                    if item.check_expire(expire_time) {
                        return Some(item.get_data());
                    }
                } else {
                    return Some(item.get_data());
                }
            }
        }
        None
    }

    // get cache map from key list
    pub fn get_cache_map(
        &self,
        group: &GroupType,
        key_list: &Vec<String>,
        expire_time: Option<u64>,
    ) -> Option<CacheMap<String, Bytes>> {
        let mut builder = CacheMapBuilder::new();
        for key in key_list {
            if let Some(value) = self.get(group, key, expire_time) {
                builder.set(key.to_string(), value.clone());
            }
        }
        builder.build()
    }

    pub fn save(&mut self, group: GroupType, key: &str, value: Bytes) {
        let map = self
            .cache_map
            .entry(group)
            .or_insert_with(|| HashMap::new());
        let item = CacheItem::new(value);
        map.insert(key.to_string(), item);
    }

    pub fn clean(&mut self, group: &GroupType) {
        self.cache_map.remove(&group);
    }
}

static CACHE_MANAGER: Lazy<Mutex<CacheManager>> = Lazy::new(|| Mutex::new(CacheManager::new()));

pub fn get_cache_manager() -> std::sync::MutexGuard<'static, CacheManager> {
    CACHE_MANAGER.lock().unwrap()
}

#[derive(Debug, Eq, Hash, PartialEq)]
pub enum GroupType {
    #[allow(non_camel_case_types)]
    REPO_INSIDE,
    API,
}
