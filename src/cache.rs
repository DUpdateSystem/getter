use std::collections::HashMap;

pub struct CacheManager {
    cache_map: HashMap<i32, HashMap<String, String>>,
}

impl CacheManager {
    pub fn new() -> Self {
        Self {
            cache_map: HashMap::new(),
        }
    }

    pub fn get(&self, group: i32, key: &str) -> Option<&String> {
        if let Some(map) = self.cache_map.get(&group) {
            map.get(key)
        } else {
            None
        }
    }

    pub fn save(&mut self, group: i32, key: &str, value: &str) {
        if let Some(map) = self.cache_map.get_mut(&group) {
            map.insert(key.to_string(), value.to_string());
        } else {
            let mut map = HashMap::new();
            map.insert(key.to_string(), value.to_string());
            self.cache_map.insert(group, map);
        }
    }

    pub fn clean(&mut self, group: i32) {
        self.cache_map.remove(&group);
    }
}
