use async_trait::async_trait;
use bytes::Bytes;
use std::{
    collections::{BTreeMap, HashMap},
    error::Error,
};

use super::super::data::release::*;

pub type HubDataMap<'a> = BTreeMap<&'a str, &'a str>;
pub type AppDataMap<'a> = BTreeMap<&'a str, &'a str>;

pub struct DataMap<'a> {
    pub app_data: &'a AppDataMap<'a>,
    pub hub_data: &'a HubDataMap<'a>,
}

pub type CacheMap<K, T> = HashMap<K, T>;

pub enum FunctionType {
    CheckAppAvailable,
    GetLatestRelease,
    GetReleases,
}

pub struct FIn<'a> {
    pub data_map: DataMap<'a>,
    cache_map: Option<HashMap<String, Bytes>>,
}

impl<'a> FIn<'a> {
    pub fn new_with_frag(
        app_data: &'a AppDataMap<'a>,
        hub_data: &'a HubDataMap<'a>,
        cache_map: Option<CacheMap<String, Bytes>>,
    ) -> Self {
        FIn {
            data_map: DataMap {
                app_data,
                hub_data,
            },
            cache_map,
        }
    }
    pub fn new(
        data_map: DataMap<'a>,
        cache_map: Option<CacheMap<String, Bytes>>,
    ) -> Self {
        FIn {
            data_map,
            cache_map,
        }
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
    fn get_cache_request_key(
        &self,
        function_type: &FunctionType,
        data_map: &DataMap,
    ) -> Vec<String>;

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

pub trait BaseProviderExt: BaseProvider {
    fn url_proxy_map(&self) -> &HashMap<String, String>;

    fn replace_proxy_url(&self, url: &str) -> String {
        let mut url = url.to_string();
        for (url_prefix, proxy_url) in self.url_proxy_map() {
            url = url.replace(url_prefix, proxy_url);
        }
        url
    }
}

pub const ANDROID_APP_TYPE: &str = "android_app_package";
pub const ANDROID_MAGISK_MODULE_TYPE: &str = "android_magisk_module";
pub const ANDROID_CUSTOM_SHELL: &str = "android_custom_shell";
pub const ANDROID_CUSTOM_SHELL_ROOT: &str = "android_custom_shell_root";

pub const KEY_REPO_URL: &str = "repo_url";
pub const KEY_REPO_API_URL: &str = "repo_api_url";

#[cfg(test)]
mod tests {
    use super::*;

    pub struct MockProvider {
        url_proxy_map: HashMap<String, String>,
    }

    impl MockProvider {
        pub fn new() -> MockProvider {
            MockProvider {
                url_proxy_map: HashMap::new(),
            }
        }
    }

    #[async_trait]
    impl BaseProvider for MockProvider {
        fn get_cache_request_key(
            &self,
            function_type: &FunctionType,
            data_map: &DataMap,
        ) -> Vec<String> {
            let key_name = match function_type {
                FunctionType::CheckAppAvailable => "check_app_available",
                FunctionType::GetLatestRelease => "get_latest_release",
                FunctionType::GetReleases => "get_releases",
            };
            let id_map = data_map.app_data;
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
            let id_map = fin.data_map.app_data;
            let cache_map = fin.cache_map.clone();
            FOut::new(cache_map.unwrap_or_default().get(id_map["id"]).is_some())
        }

        async fn get_releases(&self, fin: &FIn) -> FOut<Vec<ReleaseData>> {
            let id_map = fin.data_map.app_data;
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

    impl BaseProviderExt for MockProvider {
        fn url_proxy_map(&self) -> &HashMap<String, String> {
            &self.url_proxy_map
        }
    }

    #[tokio::test]
    async fn test_get_cache_request_key() {
        let mock = MockProvider::new();
        let data_map = DataMap {
            app_data: &AppDataMap::from([("id", "123")]),
            hub_data: &HubDataMap::new(),
        };

        let key = mock.get_cache_request_key(&FunctionType::CheckAppAvailable, &data_map);
        assert_eq!(key, vec!["check_app_available:id=123"]);
        let key = mock.get_cache_request_key(&FunctionType::GetReleases, &data_map);
        assert_eq!(key, vec!["get_releases:id=123"]);
    }

    #[tokio::test]
    async fn test_check_app_available() {
        let mock = MockProvider::new();
        let id_map = AppDataMap::from([("id", "123")]);
        let cache_map = CacheMap::from([("123".to_string(), Bytes::from(vec![1u8]))]);

        let fin = FIn {
            data_map: DataMap {
                app_data: &id_map,
                hub_data: &BTreeMap::new(),
            },
            cache_map: Some(cache_map),
        };

        let available = mock.check_app_available(&fin).await;
        assert_eq!(available.result.ok(), Some(true));
    }

    #[tokio::test]
    async fn test_get_releases() {
        let mock = MockProvider::new();
        let cache_map = CacheMap::from([("123".to_string(), Bytes::from(vec![1u8, 2u8, 3u8]))]);

        let fin = FIn {
            data_map: DataMap {
                app_data: &AppDataMap::from([("id", "123")]),
                hub_data: &BTreeMap::new(),
            },
            cache_map: Some(cache_map),
        };

        let releases = mock.get_releases(&fin).await;
        assert_eq!(releases.result.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn test_get_latest_release() {
        let mock = MockProvider::new();
        let cache_map = CacheMap::from([("123".to_string(), Bytes::from(vec![1u8, 2u8, 3u8]))]);

        let fin = FIn {
            data_map: DataMap {
                app_data: &AppDataMap::from([("id", "123")]),
                hub_data: &BTreeMap::new(),
            },
            cache_map: Some(cache_map),
        };

        let latest_release = mock.get_latest_release(&fin).await;
        let latest_version = latest_release.result.unwrap().version_number;
        assert_eq!(latest_version, "1");
    }

    fn test_replace_proxy_url() {
        let mut mock = MockProvider::new();
        let url = "https://github.com";
        let url_r = "https://github.com.proxy";
        mock.url_proxy_map
            .insert(url.to_string(), url_r.to_string());
        let result = mock.replace_proxy_url(url);
        assert_eq!(result, url_r);
    }
}
