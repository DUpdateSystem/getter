use async_trait::async_trait;

use bytes::Bytes;
use core::fmt;
use regex::Regex;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::{
    collections::{BTreeMap, HashMap},
    error::Error,
};

use super::super::data::release::*;

pub type HubDataMap<'a> = BTreeMap<&'a str, &'a str>;
pub type AppDataMap<'a> = BTreeMap<&'a str, &'a str>;

#[derive(Hash)]
pub struct DataMap<'a> {
    pub app_data: &'a AppDataMap<'a>,
    pub hub_data: &'a HubDataMap<'a>,
}

impl fmt::Display for DataMap<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "app_data: {:?}, hub_data: {:?}",
            self.app_data, self.hub_data
        )
    }
}

impl DataMap<'_> {
    pub fn get_hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
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
            data_map: DataMap { app_data, hub_data },
            cache_map,
        }
    }
    pub fn new(data_map: DataMap<'a>, cache_map: Option<CacheMap<String, Bytes>>) -> Self {
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

    pub fn set_cache(mut self, key: &str, value: Bytes) -> Self {
        let cache_map = self.cached_map.get_or_insert_with(HashMap::new);
        cache_map.insert(key.to_string(), value);
        self
    }

    pub fn set_error(mut self, error: Box<dyn Error + Send + Sync>) -> Self {
        self.result = Err(error);
        self
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
    fn url_proxy_map(&self, fin: &FIn) -> HashMap<String, String> {
        let hub_data = fin.data_map.hub_data;
        if let Some(proxy_map) = hub_data.get(REVERSE_PROXY) {
            proxy_map
                .lines()
                .map(|line| {
                    let mut parts = line.splitn(2, "->");
                    let url_prefix = parts.next().unwrap_or_default().trim();
                    let proxy_url = parts.next().unwrap_or_default().trim();
                    (url_prefix.to_string(), proxy_url.to_string())
                })
                .filter(|v| !v.0.is_empty() && !v.1.is_empty())
                .collect::<HashMap<String, String>>()
        } else {
            return HashMap::new();
        }
    }

    fn replace_proxy_url(&self, fin: &FIn, url: &str) -> String {
        let mut result_url = url.to_string();
        for (url_prefix, proxy_url) in self.url_proxy_map(fin).iter() {
            let regex_prefix = "regex:";
            if url_prefix.starts_with(regex_prefix) {
                let url_prefix = &url_prefix[regex_prefix.len()..].trim();
                if let Ok(re) = Regex::new(&url_prefix) {
                    result_url = re.replace_all(&result_url, proxy_url.clone()).to_string();
                }
            } else {
                result_url = result_url.replace(url_prefix, proxy_url);
            }
        }
        result_url
    }
}

pub const ANDROID_APP_TYPE: &str = "android_app_package";
pub const ANDROID_MAGISK_MODULE_TYPE: &str = "android_magisk_module";
pub const ANDROID_CUSTOM_SHELL: &str = "android_custom_shell";
pub const ANDROID_CUSTOM_SHELL_ROOT: &str = "android_custom_shell_root";

pub const KEY_REPO_URL: &str = "repo_url";
pub const KEY_REPO_API_URL: &str = "repo_api_url";

pub const REVERSE_PROXY: &str = "reverse_proxy";

#[cfg(test)]
mod tests {
    use super::*;

    pub struct MockProvider;

    impl MockProvider {
        pub fn new() -> MockProvider {
            MockProvider {}
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

    impl BaseProviderExt for MockProvider {}

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

    #[test]
    fn test_replace_proxy_url() {
        let mock = MockProvider::new();
        let url = "https://github.com";
        let url_r = "https://github.com.proxy";
        let proxy_url = format!("{} -> {}", url, url_r);
        let data_map = HubDataMap::from([("reverse_proxy", proxy_url.as_str())]);
        let result = mock.replace_proxy_url(
            &FIn::new_with_frag(&AppDataMap::new(), &data_map, None),
            url,
        );
        assert_eq!(result, url_r);

        let proxy_url = format!("{}->{}", url, url_r);
        let data_map = HubDataMap::from([("reverse_proxy", proxy_url.as_str())]);
        let result = mock.replace_proxy_url(
            &FIn::new_with_frag(&AppDataMap::new(), &data_map, None),
            url,
        );
        assert_eq!(result, url_r);

        let url_r = format!("{} -> proxy", url);
        let proxy_url = format!(" {}  ->{} ", url, url_r);
        let data_map = HubDataMap::from([("reverse_proxy", proxy_url.as_str())]);
        let result = mock.replace_proxy_url(
            &FIn::new_with_frag(&AppDataMap::new(), &data_map, None),
            url,
        );
        assert_eq!(result, url_r);
    }

    #[test]
    fn test_replace_proxy_url_with_regex() {
        let mock = MockProvider::new();

        let regex_url = "regex:^https:.*/";
        let url_r = "https://github-proxy.com/";

        let proxy_url = format!("{} -> {}", regex_url, url_r);
        let data_map = HubDataMap::from([("reverse_proxy", proxy_url.as_str())]);

        let url = "https://github.com/GitHub";
        let expected_url = "https://github-proxy.com/GitHub";

        let result = mock.replace_proxy_url(
            &FIn::new_with_frag(&AppDataMap::new(), &data_map, None),
            url,
        );

        assert_eq!(
            result, expected_url,
            "URL should be rewritten to use the proxy domain."
        );

        let non_matching_url = "http://example.com";
        let result = mock.replace_proxy_url(
            &FIn::new_with_frag(&AppDataMap::new(), &data_map, None),
            non_matching_url,
        );

        assert_eq!(
            result, non_matching_url,
            "Non-matching URL should not be rewritten."
        );
    }

    #[test]
    fn test_replace_proxy_url_multiple() {
        let mock = MockProvider::new();
        let url = "https://github.com";
        let proxy_url = "https -> http\ngithub -> github-proxy";
        let url_r = "http://github-proxy.com";
        let data_map = HubDataMap::from([("reverse_proxy", proxy_url)]);
        let result = mock.replace_proxy_url(
            &FIn::new_with_frag(&AppDataMap::new(), &data_map, None),
            url,
        );
        assert_eq!(result, url_r);
    }
}
