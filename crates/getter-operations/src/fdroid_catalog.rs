//! Fixture-backed F-Droid catalog cache/query operations.
//!
//! This is the first getter-owned F-Droid operation slice after the provider
//! parser. It wires parsed catalog facts through `cache.db` refresh semantics
//! without introducing live HTTP, Flutter-owned provider parsing, or downloader
//! task state.

use crate::provider_cache::{
    read_or_refresh_provider_response_with_provenance, source_response_sha512,
    ProviderCacheDiagnostic, ProviderCacheMode, ProviderCacheOperationError, ProviderCacheRequest,
    ProviderCacheSource, ProviderResponseRefresh,
};
use getter_providers::{parse_fdroid_index_xml, FdroidApp, FdroidCatalog, FdroidCatalogError};
use getter_storage::{CacheDb, StorageError};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha512};

pub const FDROID_PROVIDER_ID: &str = "fdroid";
pub const FDROID_CATALOG_CACHE_VERSION: &str = "fdroid-index-v1";
pub const DEFAULT_FDROID_ENDPOINT_ID: &str = "official";
pub const DEFAULT_FDROID_ENDPOINT_URL: &str = "https://f-droid.org/repo";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdroidEndpointConfig {
    pub endpoint_id: String,
    pub endpoint_url: String,
}

impl Default for FdroidEndpointConfig {
    fn default() -> Self {
        Self {
            endpoint_id: DEFAULT_FDROID_ENDPOINT_ID.to_owned(),
            endpoint_url: DEFAULT_FDROID_ENDPOINT_URL.to_owned(),
        }
    }
}

impl FdroidEndpointConfig {
    pub fn cache_key(&self) -> String {
        format!(
            "{FDROID_PROVIDER_ID}:{FDROID_CATALOG_CACHE_VERSION}:{}:{}",
            self.endpoint_id,
            digest_hex(&self.endpoint_url),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdroidCatalogResult {
    pub endpoint: FdroidEndpointConfig,
    pub cache_key: String,
    pub catalog: FdroidCatalog,
    pub source: ProviderCacheSource,
    pub source_response_sha512: Vec<String>,
    pub provenance_schema_version: Option<String>,
    pub diagnostics: Vec<ProviderCacheDiagnostic>,
}

impl FdroidCatalogResult {
    pub fn app(&self, package_name: &str) -> Option<&FdroidApp> {
        self.catalog.app(package_name)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FdroidCatalogOperationError {
    #[error("provider cache operation failed: {0}")]
    Cache(#[from] ProviderCacheOperationError),
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("F-Droid catalog parse failed: {0}")]
    Catalog(#[from] FdroidCatalogError),
    #[error("F-Droid catalog serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid F-Droid catalog request: {0}")]
    InvalidRequest(String),
}

pub fn read_or_refresh_fdroid_catalog<F>(
    db: &CacheDb,
    endpoint: FdroidEndpointConfig,
    mode: ProviderCacheMode,
    refresh_xml: F,
) -> Result<FdroidCatalogResult, FdroidCatalogOperationError>
where
    F: FnOnce() -> Result<String, String>,
{
    let cache_key = endpoint.cache_key();
    let cache_result = read_or_refresh_provider_response_with_provenance(
        db,
        ProviderCacheRequest {
            cache_key: &cache_key,
            provider: FDROID_PROVIDER_ID,
            mode,
        },
        || {
            let xml = refresh_xml()?;
            let source_digest = source_response_sha512(xml.as_bytes());
            let catalog = parse_fdroid_index_xml(&xml).map_err(|source| source.to_string())?;
            let response_json =
                serde_json::to_value(catalog).map_err(|source| source.to_string())?;
            Ok(ProviderResponseRefresh {
                response_json,
                source_response_sha512: vec![source_digest],
                freshness_json: json!({}),
            })
        },
    )?;
    let response = cache_result.response;
    let catalog = serde_json::from_value(response.response_json)?;

    Ok(FdroidCatalogResult {
        endpoint,
        cache_key,
        catalog,
        source: cache_result.source,
        source_response_sha512: response.source_response_sha512,
        provenance_schema_version: response.provenance_schema_version,
        diagnostics: cache_result.diagnostics,
    })
}

pub fn fdroid_catalog_json(
    db: &CacheDb,
    request_json: &str,
) -> Result<Value, FdroidCatalogOperationError> {
    let request: FdroidCatalogJsonRequest = serde_json::from_str(request_json)?;
    let endpoint = FdroidEndpointConfig {
        endpoint_id: request
            .endpoint_id
            .unwrap_or_else(|| DEFAULT_FDROID_ENDPOINT_ID.to_owned()),
        endpoint_url: request
            .endpoint_url
            .unwrap_or_else(|| DEFAULT_FDROID_ENDPOINT_URL.to_owned()),
    };
    let mode = match request.mode.as_deref() {
        Some("force_refresh") => ProviderCacheMode::ForceRefresh,
        Some("use_cached") | None => ProviderCacheMode::UseCached,
        Some(other) => {
            return Err(FdroidCatalogOperationError::InvalidRequest(format!(
                "unknown mode '{other}'"
            )))
        }
    };
    let result = read_or_refresh_fdroid_catalog(db, endpoint, mode, || {
        request
            .index_xml
            .ok_or_else(|| "fixture-backed F-Droid catalog refresh requires index_xml".to_owned())
    })?;
    let package_matches: Vec<Value> = request
        .package_names
        .iter()
        .filter_map(|package_name| result.app(package_name))
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;

    Ok(json!({
        "operation": "fdroid.catalog",
        "provider": FDROID_PROVIDER_ID,
        "endpoint_id": result.endpoint.endpoint_id,
        "endpoint_url": result.endpoint.endpoint_url,
        "cache_key": result.cache_key,
        "source": provider_source_json(result.source),
        "catalog": result.catalog,
        "matches": package_matches,
        "diagnostics": result.diagnostics.iter().map(diagnostic_json).collect::<Vec<_>>(),
    }))
}

#[derive(Debug, Deserialize)]
struct FdroidCatalogJsonRequest {
    #[serde(default)]
    endpoint_id: Option<String>,
    #[serde(default)]
    endpoint_url: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    index_xml: Option<String>,
    #[serde(default)]
    package_names: Vec<String>,
}

fn provider_source_json(source: ProviderCacheSource) -> &'static str {
    match source {
        ProviderCacheSource::Cache => "cache",
        ProviderCacheSource::Refreshed => "refreshed",
        ProviderCacheSource::Stale => "stale",
    }
}

fn diagnostic_json(diagnostic: &ProviderCacheDiagnostic) -> Value {
    json!({
        "code": diagnostic.code,
        "message": diagnostic.message,
        "cache_key": diagnostic.cache_key,
        "provider": diagnostic.provider,
        "stale_fetched_at_unix": diagnostic.stale_fetched_at_unix,
    })
}

fn digest_hex(value: &str) -> String {
    let mut hasher = Sha512::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_cache::{CACHE_REFRESH_FAILED, USED_STALE_CACHE};
    use serde_json::json;
    use std::cell::Cell;

    const FDROID_FIXTURE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<fdroid>
  <repo name="F-Droid" timestamp="1700000000" url="https://f-droid.org/repo" />
  <application id="org.fdroid.fdroid">
    <name>F-Droid</name>
    <summary>App repository client</summary>
    <package>
      <version>1.20.0</version>
      <versioncode>1020000</versioncode>
      <apkname>org.fdroid.fdroid_1020000.apk</apkname>
      <hash type="sha256">aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</hash>
      <size>1234567</size>
    </package>
  </application>
</fdroid>
"#;

    fn endpoint() -> FdroidEndpointConfig {
        FdroidEndpointConfig::default()
    }

    #[test]
    fn cache_miss_parses_and_stores_fixture_catalog() {
        let db = CacheDb::open_in_memory().unwrap();
        let endpoint = endpoint();
        let cache_key = endpoint.cache_key();

        let result =
            read_or_refresh_fdroid_catalog(&db, endpoint, ProviderCacheMode::UseCached, || {
                Ok(FDROID_FIXTURE.to_owned())
            })
            .unwrap();

        assert_eq!(result.source, ProviderCacheSource::Refreshed);
        assert_eq!(
            result
                .catalog
                .app("org.fdroid.fdroid")
                .unwrap()
                .packages
                .len(),
            1
        );
        let cached = db.provider_response(&cache_key).unwrap().unwrap();
        assert_eq!(cached.provider, FDROID_PROVIDER_ID);
        assert_eq!(
            cached.response_json["apps"][0]["package_name"],
            "org.fdroid.fdroid"
        );
    }

    #[test]
    fn cache_hit_avoids_refreshing_fixture_catalog() {
        let db = CacheDb::open_in_memory().unwrap();
        read_or_refresh_fdroid_catalog(&db, endpoint(), ProviderCacheMode::UseCached, || {
            Ok(FDROID_FIXTURE.to_owned())
        })
        .unwrap();
        let refreshed = Cell::new(false);

        let result =
            read_or_refresh_fdroid_catalog(&db, endpoint(), ProviderCacheMode::UseCached, || {
                refreshed.set(true);
                Err("should not refresh".to_owned())
            })
            .unwrap();

        assert!(!refreshed.get());
        assert_eq!(result.source, ProviderCacheSource::Cache);
        assert_eq!(result.catalog.endpoint.name.as_deref(), Some("F-Droid"));
    }

    #[test]
    fn forced_refresh_failure_returns_stale_catalog_with_diagnostics() {
        let db = CacheDb::open_in_memory().unwrap();
        read_or_refresh_fdroid_catalog(&db, endpoint(), ProviderCacheMode::UseCached, || {
            Ok(FDROID_FIXTURE.to_owned())
        })
        .unwrap();

        let result = read_or_refresh_fdroid_catalog(
            &db,
            endpoint(),
            ProviderCacheMode::ForceRefresh,
            || Err("HTTP 503".to_owned()),
        )
        .unwrap();

        assert_eq!(result.source, ProviderCacheSource::Stale);
        assert_eq!(
            result
                .catalog
                .app("org.fdroid.fdroid")
                .unwrap()
                .name
                .as_deref(),
            Some("F-Droid")
        );
        let codes: Vec<_> = result
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect();
        assert_eq!(codes, vec![CACHE_REFRESH_FAILED, USED_STALE_CACHE]);
    }

    #[test]
    fn json_operation_returns_package_name_matches_from_cached_catalog() {
        let db = CacheDb::open_in_memory().unwrap();
        let first = fdroid_catalog_json(
            &db,
            &json!({
                "index_xml": FDROID_FIXTURE,
                "package_names": ["org.fdroid.fdroid", "missing.package"]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(first["source"], "refreshed");

        let second = fdroid_catalog_json(
            &db,
            &json!({
                "package_names": ["org.fdroid.fdroid"]
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(second["source"], "cache");
        assert_eq!(second["matches"].as_array().unwrap().len(), 1);
        assert_eq!(second["matches"][0]["package_name"], "org.fdroid.fdroid");
    }
}
