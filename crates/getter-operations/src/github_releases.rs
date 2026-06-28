//! Fixture-backed GitHub release cache/query operations.
//!
//! This is the first getter-owned GitHub provider slice. It parses controlled
//! GitHub REST release fixtures, runs them through the shared provider cache,
//! and normalizes release assets into getter update candidates without adding
//! live HTTP, Flutter/Kotlin provider parsing, downloader, or installer logic.

use crate::provider_cache::{
    read_or_refresh_provider_response_with_provenance, source_response_sha512,
    ProviderCacheDiagnostic, ProviderCacheMode, ProviderCacheOperationError, ProviderCacheRequest,
    ProviderCacheSource, ProviderResponseRefresh,
};
use getter_providers::{
    github_release_update_candidates, parse_github_releases_json, GithubAssetFilter,
    GithubProviderError, GithubRelease, GithubReleaseCandidateOptions,
};
use getter_storage::{CacheDb, StorageError};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha512};

pub const GITHUB_PROVIDER_ID: &str = "github";
pub const GITHUB_RELEASES_CACHE_VERSION: &str = "github-releases-v1";
pub const DEFAULT_GITHUB_API_BASE_URL: &str = "https://api.github.com";
pub const GITHUB_ASSET_NOT_FOUND: &str = "provider.github.asset_not_found";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubReleaseConfig {
    pub api_base_url: String,
    pub owner: String,
    pub repo: String,
}

impl GithubReleaseConfig {
    pub fn cache_key(&self) -> String {
        format!(
            "{GITHUB_PROVIDER_ID}:{GITHUB_RELEASES_CACHE_VERSION}:releases:{}:{}/{}",
            digest_hex(&self.api_base_url),
            self.owner,
            self.repo,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubReleaseResult {
    pub config: GithubReleaseConfig,
    pub cache_key: String,
    pub releases: Vec<GithubRelease>,
    pub source: ProviderCacheSource,
    pub source_response_sha512: Vec<String>,
    pub provenance_schema_version: Option<String>,
    pub diagnostics: Vec<ProviderCacheDiagnostic>,
}

#[derive(Debug, thiserror::Error)]
pub enum GithubReleaseOperationError {
    #[error("provider cache operation failed: {0}")]
    Cache(#[from] ProviderCacheOperationError),
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("GitHub provider parse failed: {0}")]
    Provider(#[from] GithubProviderError),
    #[error("GitHub provider serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid GitHub release request: {0}")]
    InvalidRequest(String),
}

pub fn read_or_refresh_github_releases<F>(
    db: &CacheDb,
    config: GithubReleaseConfig,
    mode: ProviderCacheMode,
    refresh_json: F,
) -> Result<GithubReleaseResult, GithubReleaseOperationError>
where
    F: FnOnce() -> Result<String, String>,
{
    let cache_key = config.cache_key();
    let cache_result = read_or_refresh_provider_response_with_provenance(
        db,
        ProviderCacheRequest {
            cache_key: &cache_key,
            provider: GITHUB_PROVIDER_ID,
            mode,
        },
        || {
            let json = refresh_json()?;
            let source_digest = source_response_sha512(json.as_bytes());
            let releases =
                parse_github_releases_json(&json).map_err(|source| source.to_string())?;
            let response_json =
                serde_json::to_value(releases).map_err(|source| source.to_string())?;
            Ok(ProviderResponseRefresh {
                response_json,
                source_response_sha512: vec![source_digest],
                freshness_json: serde_json::json!({}),
            })
        },
    )?;
    let response = cache_result.response;
    let releases = serde_json::from_value(response.response_json)?;

    Ok(GithubReleaseResult {
        config,
        cache_key,
        releases,
        source: cache_result.source,
        source_response_sha512: response.source_response_sha512,
        provenance_schema_version: response.provenance_schema_version,
        diagnostics: cache_result.diagnostics,
    })
}

pub fn github_releases_json(
    db: &CacheDb,
    request_json: &str,
) -> Result<Value, GithubReleaseOperationError> {
    let request: GithubReleasesJsonRequest = serde_json::from_str(request_json)?;
    let owner = required_non_empty(request.owner, "owner")?;
    let repo = required_non_empty(request.repo, "repo")?;
    let config = GithubReleaseConfig {
        api_base_url: request
            .api_base_url
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_GITHUB_API_BASE_URL.to_owned()),
        owner,
        repo,
    };
    let mode = match request.mode.as_deref() {
        Some("force_refresh") => ProviderCacheMode::ForceRefresh,
        Some("use_cached") | None => ProviderCacheMode::UseCached,
        Some(other) => {
            return Err(GithubReleaseOperationError::InvalidRequest(format!(
                "unknown mode '{other}'"
            )))
        }
    };
    let asset_filter = request.asset.unwrap_or_default();
    let options = GithubReleaseCandidateOptions {
        include_prereleases: request.include_prereleases,
        asset_filter: GithubAssetFilter {
            include: asset_filter.include,
            exclude: asset_filter.exclude,
        },
    };
    let result = read_or_refresh_github_releases(db, config, mode, || {
        request.releases_json.ok_or_else(|| {
            "fixture-backed GitHub release refresh requires releases_json".to_owned()
        })
    })?;
    let candidates = github_release_update_candidates(&result.releases, &options)?;
    let mut diagnostics = result
        .diagnostics
        .iter()
        .map(diagnostic_json)
        .collect::<Vec<_>>();
    if candidates.is_empty() && github_eligible_release_count(&result.releases, &options) > 0 {
        diagnostics.push(json!({
            "code": GITHUB_ASSET_NOT_FOUND,
            "message": "no GitHub release assets matched the requested filters",
            "provider": GITHUB_PROVIDER_ID,
            "owner": result.config.owner,
            "repo": result.config.repo,
            "cache_key": result.cache_key,
        }));
    }

    Ok(json!({
        "operation": "github.releases",
        "provider": GITHUB_PROVIDER_ID,
        "api_base_url": result.config.api_base_url,
        "owner": result.config.owner,
        "repo": result.config.repo,
        "cache_key": result.cache_key,
        "source": provider_source_json(result.source),
        "releases": result.releases,
        "candidates": candidates,
        "diagnostics": diagnostics,
    }))
}

#[derive(Debug, Deserialize)]
struct GithubReleasesJsonRequest {
    #[serde(default)]
    api_base_url: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    releases_json: Option<String>,
    #[serde(default)]
    include_prereleases: bool,
    #[serde(default)]
    asset: Option<GithubAssetFilterRequest>,
}

#[derive(Debug, Default, Deserialize)]
struct GithubAssetFilterRequest {
    #[serde(default)]
    include: Option<String>,
    #[serde(default)]
    exclude: Option<String>,
}

fn required_non_empty(
    value: Option<String>,
    field: &'static str,
) -> Result<String, GithubReleaseOperationError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| GithubReleaseOperationError::InvalidRequest(format!("missing {field}")))
}

fn github_eligible_release_count(
    releases: &[GithubRelease],
    options: &GithubReleaseCandidateOptions,
) -> usize {
    releases
        .iter()
        .filter(|release| !release.draft)
        .filter(|release| options.include_prereleases || !release.prerelease)
        .count()
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
    use getter_storage::ProviderResponseUpsert;
    use serde_json::json;
    use std::cell::Cell;

    const GITHUB_RELEASES_FIXTURE: &str = r#"[
  {
    "tag_name": "v1.2.0",
    "name": "Release 1.2.0",
    "body": "Release notes",
    "draft": false,
    "prerelease": false,
    "published_at": "2026-06-01T00:00:00Z",
    "assets": [
      {
        "name": "app-release.apk",
        "content_type": "application/vnd.android.package-archive",
        "size": 1234,
        "browser_download_url": "https://github.com/example/app/releases/download/v1.2.0/app-release.apk",
        "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      },
      {
        "name": "app-debug.apk",
        "content_type": "application/vnd.android.package-archive",
        "size": 2345,
        "browser_download_url": "https://github.com/example/app/releases/download/v1.2.0/app-debug.apk"
      }
    ]
  }
]"#;

    const UPDATED_RELEASES_FIXTURE: &str = r#"[
  {
    "tag_name": "v1.3.0",
    "draft": false,
    "prerelease": false,
    "assets": [
      {
        "name": "app-release.apk",
        "size": 3456,
        "browser_download_url": "https://github.com/example/app/releases/download/v1.3.0/app-release.apk"
      }
    ]
  }
]"#;

    fn config() -> GithubReleaseConfig {
        GithubReleaseConfig {
            api_base_url: DEFAULT_GITHUB_API_BASE_URL.to_owned(),
            owner: "example".to_owned(),
            repo: "app".to_owned(),
        }
    }

    #[test]
    fn cache_miss_parses_and_stores_fixture_releases() {
        let db = CacheDb::open_in_memory().unwrap();
        let config = config();
        let cache_key = config.cache_key();

        let result =
            read_or_refresh_github_releases(&db, config, ProviderCacheMode::UseCached, || {
                Ok(GITHUB_RELEASES_FIXTURE.to_owned())
            })
            .unwrap();

        assert_eq!(result.source, ProviderCacheSource::Refreshed);
        assert_eq!(result.releases[0].tag_name, "v1.2.0");
        let cached = db.provider_response(&cache_key).unwrap().unwrap();
        assert_eq!(cached.provider, GITHUB_PROVIDER_ID);
        assert_eq!(cached.response_json[0]["tag_name"], "v1.2.0");
    }

    #[test]
    fn cache_hit_avoids_refreshing_fixture_releases() {
        let db = CacheDb::open_in_memory().unwrap();
        read_or_refresh_github_releases(&db, config(), ProviderCacheMode::UseCached, || {
            Ok(GITHUB_RELEASES_FIXTURE.to_owned())
        })
        .unwrap();
        let refreshed = Cell::new(false);

        let result =
            read_or_refresh_github_releases(&db, config(), ProviderCacheMode::UseCached, || {
                refreshed.set(true);
                Ok(UPDATED_RELEASES_FIXTURE.to_owned())
            })
            .unwrap();

        assert!(!refreshed.get());
        assert_eq!(result.source, ProviderCacheSource::Cache);
        assert_eq!(result.releases[0].tag_name, "v1.2.0");
    }

    #[test]
    fn forced_refresh_replaces_cached_fixture_releases() {
        let db = CacheDb::open_in_memory().unwrap();
        read_or_refresh_github_releases(&db, config(), ProviderCacheMode::UseCached, || {
            Ok(GITHUB_RELEASES_FIXTURE.to_owned())
        })
        .unwrap();

        let result =
            read_or_refresh_github_releases(&db, config(), ProviderCacheMode::ForceRefresh, || {
                Ok(UPDATED_RELEASES_FIXTURE.to_owned())
            })
            .unwrap();

        assert_eq!(result.source, ProviderCacheSource::Refreshed);
        assert_eq!(result.releases[0].tag_name, "v1.3.0");
    }

    #[test]
    fn forced_refresh_failure_returns_stale_cache_with_diagnostics() {
        let db = CacheDb::open_in_memory().unwrap();
        let config = config();
        let cache_key = config.cache_key();
        db.upsert_provider_response(&ProviderResponseUpsert {
            cache_key,
            provider: GITHUB_PROVIDER_ID.to_owned(),
            response_json: serde_json::from_str(GITHUB_RELEASES_FIXTURE).unwrap(),
            source_response_sha512: Vec::new(),
            provenance_schema_version: None,
            freshness_json: serde_json::json!({}),
        })
        .unwrap();

        let result =
            read_or_refresh_github_releases(&db, config, ProviderCacheMode::ForceRefresh, || {
                Err("HTTP 503".to_owned())
            })
            .unwrap();

        assert_eq!(result.source, ProviderCacheSource::Stale);
        assert_eq!(result.releases[0].tag_name, "v1.2.0");
        let codes: Vec<_> = result
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect();
        assert_eq!(codes, vec![CACHE_REFRESH_FAILED, USED_STALE_CACHE]);
    }

    #[test]
    fn json_operation_returns_candidates_and_asset_diagnostics() {
        let db = CacheDb::open_in_memory().unwrap();
        let request = json!({
            "owner": "example",
            "repo": "app",
            "releases_json": GITHUB_RELEASES_FIXTURE,
            "asset": { "include": "\\.apk$", "exclude": "debug" }
        });

        let response = github_releases_json(&db, &request.to_string()).unwrap();

        assert_eq!(response["operation"], "github.releases");
        assert_eq!(response["provider"], "github");
        assert_eq!(response["source"], "refreshed");
        assert_eq!(response["candidates"][0]["version"], "v1.2.0");
        assert_eq!(
            response["candidates"][0]["artifacts"][0]["name"],
            "app-release.apk"
        );
        assert!(response["diagnostics"].as_array().unwrap().is_empty());
    }

    #[test]
    fn json_operation_reports_asset_not_found_diagnostic() {
        let db = CacheDb::open_in_memory().unwrap();
        let request = json!({
            "owner": "example",
            "repo": "app",
            "releases_json": GITHUB_RELEASES_FIXTURE,
            "asset": { "include": "\\.aab$" }
        });

        let response = github_releases_json(&db, &request.to_string()).unwrap();

        assert!(response["candidates"].as_array().unwrap().is_empty());
        assert_eq!(response["diagnostics"][0]["code"], GITHUB_ASSET_NOT_FOUND);
    }

    #[test]
    fn malformed_fixture_json_reports_provider_error() {
        let db = CacheDb::open_in_memory().unwrap();
        let request = json!({
            "owner": "example",
            "repo": "app",
            "releases_json": "not-json"
        });

        let error = github_releases_json(&db, &request.to_string()).unwrap_err();

        assert!(matches!(
            error,
            GithubReleaseOperationError::Cache(ProviderCacheOperationError::RefreshFailed(_))
        ));
    }
}
