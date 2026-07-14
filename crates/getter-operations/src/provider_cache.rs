//! Shared provider/source cache refresh helper.
//!
//! Provider implementations can use this small helper to keep ADR-0010/0012
//! forced-refresh semantics consistent: a successful refresh replaces the cache,
//! while a failed forced refresh may return old cache only with explicit stale
//! diagnostics.

use getter_storage::{CacheDb, ProviderResponseUpsert, StorageError, StoredProviderResponse};
use serde_json::{json, Value};
use sha2::{Digest, Sha512};

pub const CACHE_REFRESH_FAILED: &str = "cache.refresh_failed";
pub const CACHE_ONLY_MISS: &str = "cache.cache_only_miss";
pub const USED_STALE_CACHE: &str = "used_stale_cache";
pub const PROVIDER_RESPONSE_PROVENANCE_SCHEMA_V1: &str = "provider-response-provenance-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCacheMode {
    UseCached,
    CacheOnly,
    ForceRefresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCacheSource {
    Cache,
    Refreshed,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCacheRequest<'a> {
    pub cache_key: &'a str,
    pub provider: &'a str,
    pub mode: ProviderCacheMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderResponseRefresh {
    pub response_json: Value,
    pub source_response_sha512: Vec<String>,
    pub freshness_json: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCacheDiagnostic {
    pub code: String,
    pub message: String,
    pub cache_key: String,
    pub provider: String,
    pub stale_fetched_at_unix: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCacheResult {
    pub response: StoredProviderResponse,
    pub source: ProviderCacheSource,
    pub diagnostics: Vec<ProviderCacheDiagnostic>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderCacheOperationError {
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("provider refresh failed: {0}")]
    RefreshFailed(String),
    #[error("provider cache-only miss for '{cache_key}' ({provider})")]
    CacheOnlyMiss { cache_key: String, provider: String },
}

pub fn read_or_refresh_provider_response<F>(
    db: &CacheDb,
    request: ProviderCacheRequest<'_>,
    refresh: F,
) -> Result<ProviderCacheResult, ProviderCacheOperationError>
where
    F: FnOnce() -> Result<Value, String>,
{
    read_or_refresh_provider_response_with_provenance(db, request, || {
        refresh().map(|response_json| ProviderResponseRefresh {
            response_json,
            source_response_sha512: Vec::new(),
            freshness_json: json!({}),
        })
    })
}

pub fn read_or_refresh_provider_response_with_provenance<F>(
    db: &CacheDb,
    request: ProviderCacheRequest<'_>,
    refresh: F,
) -> Result<ProviderCacheResult, ProviderCacheOperationError>
where
    F: FnOnce() -> Result<ProviderResponseRefresh, String>,
{
    let cached = db.provider_response(request.cache_key)?;
    if matches!(
        request.mode,
        ProviderCacheMode::UseCached | ProviderCacheMode::CacheOnly
    ) {
        if let Some(response) = cached {
            return Ok(ProviderCacheResult {
                response,
                source: ProviderCacheSource::Cache,
                diagnostics: Vec::new(),
            });
        }
        if request.mode == ProviderCacheMode::CacheOnly {
            return Err(ProviderCacheOperationError::CacheOnlyMiss {
                cache_key: request.cache_key.to_owned(),
                provider: request.provider.to_owned(),
            });
        }
    }

    match refresh() {
        Ok(refresh) => {
            let response = db.upsert_provider_response(&ProviderResponseUpsert {
                cache_key: request.cache_key.to_owned(),
                provider: request.provider.to_owned(),
                response_json: refresh.response_json,
                source_response_sha512: refresh.source_response_sha512,
                provenance_schema_version: Some(PROVIDER_RESPONSE_PROVENANCE_SCHEMA_V1.to_owned()),
                freshness_json: refresh.freshness_json,
            })?;
            Ok(ProviderCacheResult {
                response,
                source: ProviderCacheSource::Refreshed,
                diagnostics: Vec::new(),
            })
        }
        Err(detail) => match cached {
            Some(response) => {
                let stale_fetched_at_unix = Some(response.fetched_at_unix);
                Ok(ProviderCacheResult {
                    response,
                    source: ProviderCacheSource::Stale,
                    diagnostics: vec![
                        ProviderCacheDiagnostic {
                            code: CACHE_REFRESH_FAILED.to_owned(),
                            message: detail,
                            cache_key: request.cache_key.to_owned(),
                            provider: request.provider.to_owned(),
                            stale_fetched_at_unix,
                        },
                        ProviderCacheDiagnostic {
                            code: USED_STALE_CACHE.to_owned(),
                            message: "using stale provider cache after refresh failure".to_owned(),
                            cache_key: request.cache_key.to_owned(),
                            provider: request.provider.to_owned(),
                            stale_fetched_at_unix,
                        },
                    ],
                })
            }
            None => Err(ProviderCacheOperationError::RefreshFailed(detail)),
        },
    }
}

pub fn source_response_sha512(body: impl AsRef<[u8]>) -> String {
    let mut hasher = Sha512::new();
    hasher.update(body.as_ref());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use getter_storage::ProviderResponseUpsert;
    use serde_json::json;
    use std::cell::Cell;

    #[test]
    fn cache_only_hit_returns_cached_response_without_refreshing() {
        let db = CacheDb::open_in_memory().unwrap();
        db.upsert_provider_response(&ProviderResponseUpsert {
            cache_key: "fdroid:official:index".to_owned(),
            provider: "fdroid".to_owned(),
            response_json: json!({ "revision": "cached" }),
            source_response_sha512: Vec::new(),
            provenance_schema_version: None,
            freshness_json: json!({}),
        })
        .unwrap();
        let refreshed = Cell::new(false);

        let result = read_or_refresh_provider_response(
            &db,
            ProviderCacheRequest {
                cache_key: "fdroid:official:index",
                provider: "fdroid",
                mode: ProviderCacheMode::CacheOnly,
            },
            || {
                refreshed.set(true);
                panic!("cache-only mode must not refresh a hit")
            },
        )
        .unwrap();

        assert!(!refreshed.get());
        assert_eq!(result.source, ProviderCacheSource::Cache);
        assert_eq!(result.response.response_json["revision"], "cached");
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn cache_only_miss_never_invokes_refresh() {
        let db = CacheDb::open_in_memory().unwrap();
        let refresh_called = Cell::new(false);
        let error = read_or_refresh_provider_response(
            &db,
            ProviderCacheRequest {
                cache_key: "github:missing",
                provider: "github-releases",
                mode: ProviderCacheMode::CacheOnly,
            },
            || {
                refresh_called.set(true);
                panic!("cache-only mode must not invoke refresh")
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ProviderCacheOperationError::CacheOnlyMiss { .. }
        ));
        assert!(!refresh_called.get());
    }

    #[test]
    fn cache_miss_refreshes_and_stores_provider_response() {
        let db = CacheDb::open_in_memory().unwrap();

        let result = read_or_refresh_provider_response_with_provenance(
            &db,
            ProviderCacheRequest {
                cache_key: "github:f-droid/fdroidclient:releases",
                provider: "github",
                mode: ProviderCacheMode::UseCached,
            },
            || {
                Ok(ProviderResponseRefresh {
                    response_json: json!({ "etag": "fresh" }),
                    source_response_sha512: vec![source_response_sha512("fresh body")],
                    freshness_json: json!({ "etag": "fresh" }),
                })
            },
        )
        .unwrap();

        assert_eq!(result.source, ProviderCacheSource::Refreshed);
        assert_eq!(result.response.response_json["etag"], "fresh");
        assert_eq!(
            result.response.provenance_schema_version.as_deref(),
            Some(PROVIDER_RESPONSE_PROVENANCE_SCHEMA_V1)
        );
        assert_eq!(
            result.response.source_response_sha512,
            vec![source_response_sha512("fresh body")]
        );
        assert_eq!(result.response.freshness_json["etag"], "fresh");
        let cached = db
            .provider_response("github:f-droid/fdroidclient:releases")
            .unwrap()
            .unwrap();
        assert_eq!(cached.response_json["etag"], "fresh");
        assert_eq!(
            cached.source_response_sha512,
            result.response.source_response_sha512
        );
    }

    #[test]
    fn forced_refresh_replaces_cached_provider_response() {
        let db = CacheDb::open_in_memory().unwrap();
        db.upsert_provider_response(&ProviderResponseUpsert {
            cache_key: "fdroid:official:index".to_owned(),
            provider: "fdroid".to_owned(),
            response_json: json!({ "revision": "old" }),
            source_response_sha512: Vec::new(),
            provenance_schema_version: None,
            freshness_json: json!({}),
        })
        .unwrap();

        let result = read_or_refresh_provider_response(
            &db,
            ProviderCacheRequest {
                cache_key: "fdroid:official:index",
                provider: "fdroid",
                mode: ProviderCacheMode::ForceRefresh,
            },
            || Ok(json!({ "revision": "new" })),
        )
        .unwrap();

        assert_eq!(result.source, ProviderCacheSource::Refreshed);
        assert_eq!(result.response.response_json["revision"], "new");
        assert_eq!(
            db.provider_response("fdroid:official:index")
                .unwrap()
                .unwrap()
                .response_json["revision"],
            "new"
        );
    }

    #[test]
    fn forced_refresh_failure_returns_stale_cache_with_diagnostics() {
        let db = CacheDb::open_in_memory().unwrap();
        db.upsert_provider_response(&ProviderResponseUpsert {
            cache_key: "github:f-droid/fdroidclient:releases".to_owned(),
            provider: "github".to_owned(),
            response_json: json!({ "etag": "old" }),
            source_response_sha512: Vec::new(),
            provenance_schema_version: None,
            freshness_json: json!({}),
        })
        .unwrap();

        let result = read_or_refresh_provider_response(
            &db,
            ProviderCacheRequest {
                cache_key: "github:f-droid/fdroidclient:releases",
                provider: "github",
                mode: ProviderCacheMode::ForceRefresh,
            },
            || Err("HTTP 503".to_owned()),
        )
        .unwrap();

        assert_eq!(result.source, ProviderCacheSource::Stale);
        assert_eq!(result.response.response_json["etag"], "old");
        let codes: Vec<_> = result
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect();
        assert_eq!(codes, vec![CACHE_REFRESH_FAILED, USED_STALE_CACHE]);
        assert!(result
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.stale_fetched_at_unix.is_some()));
    }

    #[test]
    fn refresh_failure_without_stale_cache_is_an_error() {
        let db = CacheDb::open_in_memory().unwrap();
        let error = read_or_refresh_provider_response(
            &db,
            ProviderCacheRequest {
                cache_key: "fdroid:official:index",
                provider: "fdroid",
                mode: ProviderCacheMode::ForceRefresh,
            },
            || Err("network unavailable".to_owned()),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ProviderCacheOperationError::RefreshFailed(_)
        ));
    }
}
