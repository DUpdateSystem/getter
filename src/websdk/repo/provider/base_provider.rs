use std::{
    collections::{BTreeMap, HashMap},
    error::Error,
    hash::Hash,
};
use async_trait::async_trait;
use bytes::Bytes;

use super::super::data::release::*;

pub type IdMap<'a> = BTreeMap<&'a str, &'a str>;

pub type CacheMap<K, T> = HashMap<K, T>;

#[derive(Debug, PartialEq, Clone)]
pub struct CacheMapBuilder<K: Eq + Hash + Clone, T: PartialEq + Clone> {
    map: Option<CacheMap<K, T>>,
}

impl<K: Eq + Hash + Clone, T: PartialEq + Clone> CacheMapBuilder<K, T> {
    pub fn new() -> Self {
        CacheMapBuilder { map: None }
    }

    pub fn set(&mut self, key: K, value: T) {
        if let Some(map) = &mut self.map {
            map.insert(key, value);
        } else {
            let mut map = CacheMap::new();
            map.insert(key, value);
            self.map = Some(map);
        }
    }

    pub fn get(&self, key: K) -> Option<&T> {
        if let Some(map) = &self.map {
            if let Some(value) = map.get(&key) {
                return Some(value);
            }
        }
        None
    }

    pub fn extend(mut self, other: &Self) -> Self {
        if let Some(map) = &mut self.map {
            if let Some(other_map) = &other.map {
                map.extend(other_map.clone().drain())
            }
        } else {
            if let Some(other_map) = &other.map {
                self.map = Some(other_map.clone());
            }
        }
        self
    }

    pub fn build(self) -> Option<CacheMap<K, T>> {
        self.map
    }
}

pub enum FunctionType {
    CheckAppAvailable,
    GetLatestRelease,
    GetReleases,
}

pub struct FIn<'a> {
    pub id_map: &'a IdMap<'a>,
    cache_map: Option<HashMap<String, Bytes>>,
}

impl<'a> FIn<'a> {
    pub fn new(id_map: &'a IdMap<'a>, cache_map: Option<CacheMap<String, Bytes>>) -> Self {
        FIn { id_map, cache_map }
    }

    pub fn get_cache(&self, key: &str) -> Option<&Bytes> {
        if let Some(cache_map) = &self.cache_map {
            if let Some(value) = cache_map.get(key) {
                return Some(value);
            }
        }
        None
    }
}

#[derive(Debug)]
pub struct FOut<T> {
    pub result: Result<T, Box<dyn Error + Send + Sync>>,
    pub cached_map: Option<HashMap<String, Bytes>>,
}

impl<T> FOut<T> {
    pub fn new(data: T) -> Self {
        FOut {
            result: Ok(data),
            cached_map: None,
        }
    }

    pub fn new_empty() -> Self {
        FOut {
            result: Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                "no data",
            ))),
            cached_map: None,
        }
    }

    pub fn set_data(mut self, data: T) -> Self {
        self.result = Ok(data);
        self
    }

    pub fn set_cached_map(mut self, cached_map: HashMap<String, Bytes>) -> Self {
        self.cached_map = Some(cached_map);
        self
    }
}

#[async_trait]
pub trait BaseProvider {
    fn get_cache_request_key(&self, function_type: &FunctionType, id_map: &IdMap) -> Vec<String>;

    async fn check_app_available(&self, fin: &FIn) -> FOut<bool>;

    async fn get_latest_release(&self, fin: &FIn) -> FOut<ReleaseData> {
        let result = self.get_releases(fin).await;

        let fout_result = match result.result {
            Ok(releases) => {
                let release = releases.first().cloned();
                if let Some(release) = release {
                    Ok(release)
                } else {
                    Err(
                        Box::new(std::io::Error::new(std::io::ErrorKind::NotFound, "no data"))
                            as Box<dyn std::error::Error + Send + Sync>,
                    )
                }
            }
            Err(e) => Err(e),
        };

        FOut {
            result: fout_result,
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
        fn get_cache_request_key(
            &self,
            function_type: &FunctionType,
            id_map: &IdMap,
        ) -> Vec<String> {
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
            let cache_map = fin.cache_map.clone();
            FOut::new(cache_map.unwrap_or_default().get(id_map["id"]).is_some())
        }

        async fn get_releases(&self, fin: &FIn) -> FOut<Vec<ReleaseData>> {
            let id_map = fin.id_map;
            let cache_map = fin.cache_map.clone();
            FOut::new(
                cache_map
                    .unwrap_or_default()
                    .get(id_map["id"])
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
        let id_map = IdMap::from([("id", "123")]);

        let key = mock.get_cache_request_key(&FunctionType::CheckAppAvailable, &id_map);
        assert_eq!(key, vec!["check_app_available:id=123"]);
        let key = mock.get_cache_request_key(&FunctionType::GetReleases, &id_map);
        assert_eq!(key, vec!["get_releases:id=123"]);
    }

    #[tokio::test]
    async fn test_check_app_available() {
        let mock = MockProvider;
        let id_map = IdMap::from([("id", "123")]);
        let cache_map = CacheMap::from([("123".to_string(), Bytes::from(vec![1u8]))]);

        let fin = FIn {
            id_map: &id_map,
            cache_map: Some(cache_map),
        };

        let available = mock.check_app_available(&fin).await;
        assert_eq!(available.result.ok(), Some(true));
    }

    #[tokio::test]
    async fn test_get_releases() {
        let mock = MockProvider;
        let id_map = IdMap::from([("id", "123")]);
        let cache_map = CacheMap::from([("123".to_string(), Bytes::from(vec![1u8, 2u8, 3u8]))]);

        let fin = FIn {
            id_map: &id_map,
            cache_map: Some(cache_map),
        };

        let releases = mock.get_releases(&fin).await;
        assert_eq!(releases.result.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn test_get_latest_release() {
        let mock = MockProvider;
        let id_map = IdMap::from([("id", "123")]);
        let cache_map = CacheMap::from([("123".to_string(), Bytes::from(vec![1u8, 2u8, 3u8]))]);

        let fin = FIn {
            id_map: &id_map,
            cache_map: Some(cache_map),
        };

        let latest_release = mock.get_latest_release(&fin).await;
        let latest_version = latest_release.result.unwrap().version_number;
        assert_eq!(latest_version, "1");
    }
}
