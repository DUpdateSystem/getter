use crate::data::release::*;
use std::{collections::HashMap, hash::Hash};

use async_trait::async_trait;
use bytes::Bytes;

pub type IdMap<'a> = HashMap<&'a str, &'a str>;

#[derive(Debug, PartialEq)]
pub struct CacheMap<K: Eq + Hash, T: PartialEq> {
    pub map: Option<HashMap<K, T>>,
}

impl<K: Eq + Hash, T: PartialEq> CacheMap<K, T> {
    pub fn new() -> Self {
        CacheMap {
            map: Some(HashMap::new()),
        }
    }

    pub fn set(mut self, key: K, value: T) -> Self {
        if let Some(map) = &mut self.map {
            map.insert(key, value);
        }
        self
    }

    pub fn get(&self, key: K) -> Option<&T> {
        if let Some(map) = &self.map {
            if let Some(value) = map.get(&key) {
                return Some(value);
            }
        }
        None
    }
}

pub enum FunctionType {
    CheckAppAvailable,
    GetLatestRelease,
    GetReleases,
}

pub struct FIn<'a> {
    pub id_map: &'a IdMap<'a>,
    pub cache_map: &'a CacheMap<&'a str, &'a Bytes>,
}

impl<'a> FIn<'a> {
    pub fn new<'b>(id_map: &'a IdMap) -> Self {
        FIn {
            id_map,
            cache_map: &CacheMap { map: None },
        }
    }

    pub fn set_cache_map(mut self, cache_map: &'a CacheMap<&'a str, &'a Bytes>) -> Self {
        self.cache_map = cache_map;
        self
    }
}

#[derive(Debug, PartialEq)]
pub struct FOut<T> {
    pub data: Option<T>,
    pub cached_map: Option<CacheMap<String, Bytes>>,
}

impl<T> FOut<T> {
    pub fn new(data: T) -> Self {
        FOut {
            data: Some(data),
            cached_map: None,
        }
    }

    pub fn new_empty() -> Self {
        FOut {
            data: None,
            cached_map: None,
        }
    }

    pub fn set_data(mut self, data: T) -> Self {
        self.data = Some(data);
        self
    }

    pub fn set_cached_map(mut self, cached_map: CacheMap<String, Bytes>) -> Self {
        self.cached_map = Some(cached_map);
        self
    }
}

#[async_trait]
pub trait BaseProvider {
    fn get_cache_request_key(&self, function_type: FunctionType, id_map: IdMap) -> Vec<String>;

    async fn check_app_available(&self, fin: &FIn) -> FOut<bool>;

    async fn get_latest_release(&self, fin: &FIn) -> FOut<ReleaseData> {
        let result = self.get_releases(fin).await;
        let release = if let Some(releases) = result.data {
            releases.first().cloned()
        } else {
            None
        };
        FOut {
            data: release,
            cached_map: result.cached_map,
        }
    }

    async fn get_releases(&self, fin: &FIn) -> FOut<Vec<ReleaseData>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    pub struct MockProvider;

    #[async_trait]
    impl BaseProvider for MockProvider {
        fn get_cache_request_key(&self, function_type: FunctionType, id_map: IdMap) -> Vec<String> {
            let key_name = match function_type {
                FunctionType::CheckAppAvailable => "check_app_available",
                FunctionType::GetLatestRelease => "get_latest_release",
                FunctionType::GetReleases => "get_releases",
            };
            vec![format!(
                "{}:{}",
                key_name,
                id_map
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<String>>()
                    .join(",")
            )]
        }

        async fn check_app_available(&self, fin: &FIn) -> FOut<bool> {
            let id_map = fin.id_map;
            let cache_map = fin.cache_map;
            FOut {
                data: Some(
                    cache_map
                        .get(&id_map["id"])
                        .is_some_and(|x| x.first().is_some_and(|i| i == &1u8)),
                ),
                cached_map: None,
            }
        }

        async fn get_releases(&self, fin: &FIn) -> FOut<Vec<ReleaseData>> {
            let id_map = fin.id_map;
            let cache_map = fin.cache_map;
            FOut::new(
                cache_map
                    .get(&id_map["id"])
                    .unwrap()
                    .iter()
                    .map(|i| ReleaseData {
                        version_number: i.to_string(),
                        changelog: "".to_string(),
                        assets: vec![],
                        extra: None,
                    })
                    .collect::<Vec<ReleaseData>>(),
            )
        }
    }

    #[tokio::test]
    async fn test_get_cache_request_key() {
        let mock = MockProvider;
        let mut id_map = HashMap::new();
        id_map.insert("id", "123");

        let key = mock.get_cache_request_key(FunctionType::CheckAppAvailable, &id_map);
        assert_eq!(key, vec!["check_app_available:id=123"]);
        let key = mock.get_cache_request_key(FunctionType::GetReleases, &id_map);
        assert_eq!(key, vec!["get_releases:id=123"]);
    }

    #[tokio::test]
    async fn test_check_app_available() {
        let mock = MockProvider;
        let mut id_map = HashMap::new();
        id_map.insert("id", "123");
        let mut cache_map = HashMap::new();
        let some_vec = Bytes::from(vec![1u8]);
        cache_map.insert("123", &some_vec);

        let fin = FIn {
            id_map: &id_map,
            cache_map: &CacheMap {
                map: Some(cache_map),
            },
        };

        let available = mock.check_app_available(&fin).await;
        assert_eq!(available.data, Some(true));
    }

    #[tokio::test]
    async fn test_get_releases() {
        let mock = MockProvider;
        let mut id_map = HashMap::new();
        id_map.insert("id", "123");
        let mut cache_map = HashMap::new();
        let some_vec = Bytes::from(vec![1u8, 2u8, 3u8]);
        cache_map.insert("123", &some_vec);

        let fin = FIn {
            id_map: &id_map,
            cache_map: &CacheMap {
                map: Some(cache_map),
            },
        };

        let releases = mock.get_releases(&fin).await;
        assert_eq!(releases.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn test_get_latest_release() {
        let mock = MockProvider;
        let mut id_map = HashMap::new();
        id_map.insert("id", "123");
        let mut cache_map = HashMap::new();
        let some_vec = Bytes::from(vec![1u8, 2u8, 3u8]);
        cache_map.insert("123", &some_vec);

        let fin = FIn {
            id_map: &id_map,
            cache_map: &CacheMap {
                map: Some(cache_map),
            },
        };

        let latest_release = mock.get_latest_release(&fin).await;
        let latest_version = latest_release.unwrap().version_number;
        assert_eq!(latest_version, "1");
    }
}
