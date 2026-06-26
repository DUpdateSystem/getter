//! Shared provider/source cache refresh helper.
//!
//! Provider implementations can use this small helper to keep ADR-0010/0012
//! forced-refresh semantics consistent: a successful refresh replaces the cache,
//! while a failed forced refresh may return old cache only with explicit stale
//! diagnostics.

use getter_storage::{CacheDb, ProviderResponseUpsert, StorageError, StoredProviderResponse};
use serde_json::Value;

pub const CACHE_REFRESH_FAILED: &str = "cache.refresh_failed";
pub const USED_STALE_CACHE: &str = "used_stale_cache";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCacheMode {
    UseCached,
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
}

pub fn read_or_refresh_provider_response<F>(
    db: &CacheDb,
    request: ProviderCacheRequest<'_>,
    refresh: F,
) -> Result<ProviderCacheResult, ProviderCacheOperationError>
where
    F: FnOnce() -> Result<Value, String>,
{
    let cached = db.provider_response(request.cache_key)?;
    if request.mode == ProviderCacheMode::UseCached {
        if let Some(response) = cached {
            return Ok(ProviderCacheResult {
                response,
                source: ProviderCacheSource::Cache,
                diagnostics: Vec::new(),
            });
        }
    }

    match refresh() {
        Ok(response_json) => {
            let response = db.upsert_provider_response(&ProviderResponseUpsert {
                cache_key: request.cache_key.to_owned(),
                provider: request.provider.to_owned(),
                response_json,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;

    #[test]
    fn uses_cached_provider_response_without_refreshing() {
        let db = CacheDb::open_in_memory().unwrap();
        db.upsert_provider_response(&ProviderResponseUpsert {
            cache_key: "fdroid:official:index".to_owned(),
            provider: "fdroid".to_owned(),
            response_json: json!({ "revision": "cached" }),
        })
        .unwrap();
        let refreshed = Cell::new(false);

        let result = read_or_refresh_provider_response(
            &db,
            ProviderCacheRequest {
                cache_key: "fdroid:official:index",
                provider: "fdroid",
                mode: ProviderCacheMode::UseCached,
            },
            || {
                refreshed.set(true);
                Ok(json!({ "revision": "fresh" }))
            },
        )
        .unwrap();

        assert!(!refreshed.get());
        assert_eq!(result.source, ProviderCacheSource::Cache);
        assert_eq!(result.response.response_json["revision"], "cached");
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn cache_miss_refreshes_and_stores_provider_response() {
        let db = CacheDb::open_in_memory().unwrap();

        let result = read_or_refresh_provider_response(
            &db,
            ProviderCacheRequest {
                cache_key: "github:f-droid/fdroidclient:releases",
                provider: "github",
                mode: ProviderCacheMode::UseCached,
            },
            || Ok(json!({ "etag": "fresh" })),
        )
        .unwrap();

        assert_eq!(result.source, ProviderCacheSource::Refreshed);
        assert_eq!(result.response.response_json["etag"], "fresh");
        assert_eq!(
            db.provider_response("github:f-droid/fdroidclient:releases")
                .unwrap()
                .unwrap()
                .response_json["etag"],
            "fresh"
        );
    }

    #[test]
    fn forced_refresh_replaces_cached_provider_response() {
        let db = CacheDb::open_in_memory().unwrap();
        db.upsert_provider_response(&ProviderResponseUpsert {
            cache_key: "fdroid:official:index".to_owned(),
            provider: "fdroid".to_owned(),
            response_json: json!({ "revision": "old" }),
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
