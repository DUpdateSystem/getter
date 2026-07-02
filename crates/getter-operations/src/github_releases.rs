//! GitHub release cache/query operations.
//!
//! Getter owns both fixture-backed and live GitHub REST release refreshes,
//! runs them through the shared provider cache, and normalizes release assets
//! into getter update candidates without moving provider parsing, transport
//! controls, downloader, or installer logic into Flutter/Kotlin.

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
use std::time::Duration;

pub const GITHUB_PROVIDER_ID: &str = "github";
pub const GITHUB_RELEASES_CACHE_VERSION: &str = "github-releases-v1";
pub const DEFAULT_GITHUB_API_BASE_URL: &str = "https://api.github.com";
pub const GITHUB_ASSET_NOT_FOUND: &str = "provider.github.asset_not_found";
const GITHUB_RELEASE_TRANSPORT_TIMEOUT: Duration = Duration::from_secs(30);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GithubReleaseTransportRequest<'a> {
    pub api_base_url: &'a str,
    pub owner: &'a str,
    pub repo: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubReleaseTransportResponse {
    pub body: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GithubReleaseTransportError {
    #[error("GitHub release transport failed: {0}")]
    Transport(String),
    #[error("GitHub release transport returned HTTP {status}: {message}")]
    HttpStatus { status: u16, message: String },
}

pub trait GithubReleaseTransport {
    fn fetch_releases(
        &self,
        request: &GithubReleaseTransportRequest<'_>,
    ) -> Result<GithubReleaseTransportResponse, GithubReleaseTransportError>;
}

pub struct UreqGithubReleaseTransport {
    agent: ureq::Agent,
}

impl Default for UreqGithubReleaseTransport {
    fn default() -> Self {
        #[cfg(feature = "rustls-platform-verifier")]
        {
            return Self {
                agent: ureq::Agent::config_builder()
                    .timeout_global(Some(GITHUB_RELEASE_TRANSPORT_TIMEOUT))
                    .tls_config(
                        ureq::tls::TlsConfig::builder()
                            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                            .build(),
                    )
                    .build()
                    .new_agent(),
            };
        }

        #[cfg(not(feature = "rustls-platform-verifier"))]
        {
            Self {
                agent: ureq::Agent::config_builder()
                    .timeout_global(Some(GITHUB_RELEASE_TRANSPORT_TIMEOUT))
                    .build()
                    .new_agent(),
            }
        }
    }
}

impl UreqGithubReleaseTransport {
    pub fn new() -> Self {
        Self::default()
    }
}

impl GithubReleaseTransport for UreqGithubReleaseTransport {
    fn fetch_releases(
        &self,
        request: &GithubReleaseTransportRequest<'_>,
    ) -> Result<GithubReleaseTransportResponse, GithubReleaseTransportError> {
        let url = github_releases_url(request.api_base_url, request.owner, request.repo);
        let mut response = self
            .agent
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "UpgradeAll-getter")
            .call()
            .map_err(github_transport_error)?;
        let etag = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let last_modified = response
            .headers()
            .get("last-modified")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|source| GithubReleaseTransportError::Transport(source.to_string()))?;
        Ok(GithubReleaseTransportResponse {
            body,
            etag,
            last_modified,
        })
    }
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
    read_or_refresh_github_releases_with_refresh(db, config, mode, || {
        let body = refresh_json()?;
        github_release_refresh_from_body(body, json!({}))
    })
}

pub fn read_or_refresh_github_releases_from_default_transport(
    db: &CacheDb,
    config: GithubReleaseConfig,
    mode: ProviderCacheMode,
) -> Result<GithubReleaseResult, GithubReleaseOperationError> {
    read_or_refresh_github_releases_from_transport(
        db,
        config,
        mode,
        &UreqGithubReleaseTransport::new(),
    )
}

pub fn read_or_refresh_github_releases_from_transport<T>(
    db: &CacheDb,
    config: GithubReleaseConfig,
    mode: ProviderCacheMode,
    transport: &T,
) -> Result<GithubReleaseResult, GithubReleaseOperationError>
where
    T: GithubReleaseTransport + ?Sized,
{
    read_or_refresh_github_releases_from_transport_checked(db, config, mode, transport, |_| Ok(()))
}

pub fn read_or_refresh_github_releases_from_transport_checked<T, F>(
    db: &CacheDb,
    config: GithubReleaseConfig,
    mode: ProviderCacheMode,
    transport: &T,
    validate_body: F,
) -> Result<GithubReleaseResult, GithubReleaseOperationError>
where
    T: GithubReleaseTransport + ?Sized,
    F: FnOnce(&str) -> Result<(), String>,
{
    let refresh_config = config.clone();
    read_or_refresh_github_releases_with_refresh(db, config, mode, || {
        let request = GithubReleaseTransportRequest {
            api_base_url: &refresh_config.api_base_url,
            owner: &refresh_config.owner,
            repo: &refresh_config.repo,
        };
        let response = transport
            .fetch_releases(&request)
            .map_err(|source| source.to_string())?;
        validate_body(&response.body)?;
        let freshness_json = json!({
            "etag": response.etag,
            "last_modified": response.last_modified,
        });
        github_release_refresh_from_body(response.body, freshness_json)
    })
}

fn read_or_refresh_github_releases_with_refresh<F>(
    db: &CacheDb,
    config: GithubReleaseConfig,
    mode: ProviderCacheMode,
    refresh: F,
) -> Result<GithubReleaseResult, GithubReleaseOperationError>
where
    F: FnOnce() -> Result<ProviderResponseRefresh, String>,
{
    let cache_key = config.cache_key();
    let cache_result = read_or_refresh_provider_response_with_provenance(
        db,
        ProviderCacheRequest {
            cache_key: &cache_key,
            provider: GITHUB_PROVIDER_ID,
            mode,
        },
        refresh,
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

fn github_releases_url(api_base_url: &str, owner: &str, repo: &str) -> String {
    format!(
        "{}/repos/{owner}/{repo}/releases",
        api_base_url.trim_end_matches('/')
    )
}

fn github_transport_error(source: ureq::Error) -> GithubReleaseTransportError {
    match source {
        ureq::Error::StatusCode(status) => GithubReleaseTransportError::HttpStatus {
            status,
            message: github_http_status_message(status).to_owned(),
        },
        other => GithubReleaseTransportError::Transport(other.to_string()),
    }
}

fn github_http_status_message(status: u16) -> &'static str {
    match status {
        403 | 429 => "GitHub API rate limit or access restriction",
        404 => "GitHub repository or release endpoint was not found",
        401 => "GitHub API authentication is required",
        _ => "GitHub API request failed",
    }
}

fn github_release_refresh_from_body(
    body: String,
    freshness_json: Value,
) -> Result<ProviderResponseRefresh, String> {
    let source_digest = source_response_sha512(body.as_bytes());
    let releases = parse_github_releases_json(&body).map_err(|source| source.to_string())?;
    let response_json = serde_json::to_value(releases).map_err(|source| source.to_string())?;
    Ok(ProviderResponseRefresh {
        response_json,
        source_response_sha512: vec![source_digest],
        freshness_json,
    })
}

pub fn github_releases_json(
    db: &CacheDb,
    request_json: &str,
) -> Result<Value, GithubReleaseOperationError> {
    github_releases_json_with_transport(db, request_json, &UreqGithubReleaseTransport::new())
}

pub fn github_releases_json_with_transport<T>(
    db: &CacheDb,
    request_json: &str,
    transport: &T,
) -> Result<Value, GithubReleaseOperationError>
where
    T: GithubReleaseTransport + ?Sized,
{
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
    let result = if let Some(releases_json) = request.releases_json {
        read_or_refresh_github_releases(db, config, mode, || Ok(releases_json))
    } else {
        read_or_refresh_github_releases_from_transport(db, config, mode, transport)
    }?;
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
    use crate::provider_cache::{source_response_sha512, CACHE_REFRESH_FAILED, USED_STALE_CACHE};
    use getter_storage::ProviderResponseUpsert;
    use serde_json::json;
    use std::cell::{Cell, RefCell};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

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
    fn cache_miss_fetches_live_github_releases_through_transport() {
        let db = CacheDb::open_in_memory().unwrap();
        let config = GithubReleaseConfig {
            api_base_url: "https://api.github.test/".to_owned(),
            owner: "DUpdateSystem".to_owned(),
            repo: "UpgradeAll".to_owned(),
        };
        let cache_key = config.cache_key();
        let transport = MockGithubReleaseTransport {
            response: RefCell::new(Ok(GithubReleaseTransportResponse {
                body: GITHUB_RELEASES_FIXTURE.to_owned(),
                etag: Some("W/\"release-snapshot\"".to_owned()),
                last_modified: Some("Wed, 01 Jul 2026 00:00:00 GMT".to_owned()),
            })),
            requests: RefCell::new(Vec::new()),
        };

        let result = read_or_refresh_github_releases_from_transport(
            &db,
            config,
            ProviderCacheMode::UseCached,
            &transport,
        )
        .unwrap();

        assert_eq!(transport.requests.borrow().len(), 1);
        let request = &transport.requests.borrow()[0];
        assert_eq!(request.api_base_url, "https://api.github.test/");
        assert_eq!(request.owner, "DUpdateSystem");
        assert_eq!(request.repo, "UpgradeAll");
        assert_eq!(result.source, ProviderCacheSource::Refreshed);
        assert_eq!(result.releases[0].tag_name, "v1.2.0");
        assert_eq!(
            result.source_response_sha512,
            vec![source_response_sha512(GITHUB_RELEASES_FIXTURE.as_bytes())]
        );
        let cached = db.provider_response(&cache_key).unwrap().unwrap();
        assert_eq!(cached.provider, GITHUB_PROVIDER_ID);
        assert_eq!(cached.response_json[0]["tag_name"], "v1.2.0");
        assert_eq!(cached.freshness_json["etag"], "W/\"release-snapshot\"");
        assert_eq!(
            cached.freshness_json["last_modified"],
            "Wed, 01 Jul 2026 00:00:00 GMT"
        );
    }

    #[test]
    fn default_transport_fetches_releases_from_mock_http_server() {
        let db = CacheDb::open_in_memory().unwrap();
        let (base_url, handle) = serve_one_github_response(
            200,
            &[
                ("ETag", "W/\"mock-releases\""),
                ("Last-Modified", "Wed, 01 Jul 2026 00:00:00 GMT"),
                ("Content-Type", "application/json"),
            ],
            GITHUB_RELEASES_FIXTURE,
        );
        let config = GithubReleaseConfig {
            api_base_url: base_url,
            owner: "DUpdateSystem".to_owned(),
            repo: "UpgradeAll".to_owned(),
        };

        let result = read_or_refresh_github_releases_from_default_transport(
            &db,
            config,
            ProviderCacheMode::UseCached,
        )
        .unwrap();
        let request = handle.join().unwrap();

        assert!(request.starts_with("GET /repos/DUpdateSystem/UpgradeAll/releases HTTP/1.1"));
        assert!(request.contains("accept: application/vnd.github+json"));
        assert!(request.contains("x-github-api-version: 2022-11-28"));
        assert!(request.contains("user-agent: UpgradeAll-getter"));
        assert_eq!(result.source, ProviderCacheSource::Refreshed);
        assert_eq!(result.releases[0].tag_name, "v1.2.0");
        assert_eq!(result.source_response_sha512.len(), 1);
    }

    #[test]
    fn transport_cache_hit_does_not_call_live_refresh() {
        let db = CacheDb::open_in_memory().unwrap();
        let config = config();
        read_or_refresh_github_releases(&db, config.clone(), ProviderCacheMode::UseCached, || {
            Ok(GITHUB_RELEASES_FIXTURE.to_owned())
        })
        .unwrap();
        let transport = MockGithubReleaseTransport {
            response: RefCell::new(Err(GithubReleaseTransportError::Transport(
                "transport should not run".to_owned(),
            ))),
            requests: RefCell::new(Vec::new()),
        };

        let result = read_or_refresh_github_releases_from_transport(
            &db,
            config,
            ProviderCacheMode::UseCached,
            &transport,
        )
        .unwrap();

        assert_eq!(result.source, ProviderCacheSource::Cache);
        assert_eq!(result.releases[0].tag_name, "v1.2.0");
        assert!(transport.requests.borrow().is_empty());
    }

    #[test]
    fn transport_refresh_failure_without_stale_cache_is_provider_error() {
        let db = CacheDb::open_in_memory().unwrap();
        let transport = MockGithubReleaseTransport {
            response: RefCell::new(Err(GithubReleaseTransportError::HttpStatus {
                status: 429,
                message: "GitHub API rate limit or access restriction".to_owned(),
            })),
            requests: RefCell::new(Vec::new()),
        };

        let error = read_or_refresh_github_releases_from_transport(
            &db,
            config(),
            ProviderCacheMode::ForceRefresh,
            &transport,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            GithubReleaseOperationError::Cache(ProviderCacheOperationError::RefreshFailed(_))
        ));
        assert!(error.to_string().contains("HTTP 429"));
    }

    #[test]
    fn forced_transport_refresh_failure_returns_stale_cache_with_diagnostics() {
        let db = CacheDb::open_in_memory().unwrap();
        let config = config();
        let cache_key = config.cache_key();
        db.upsert_provider_response(&ProviderResponseUpsert {
            cache_key,
            provider: GITHUB_PROVIDER_ID.to_owned(),
            response_json: serde_json::from_str(GITHUB_RELEASES_FIXTURE).unwrap(),
            source_response_sha512: vec![source_response_sha512(
                GITHUB_RELEASES_FIXTURE.as_bytes(),
            )],
            provenance_schema_version: Some(
                crate::provider_cache::PROVIDER_RESPONSE_PROVENANCE_SCHEMA_V1.to_owned(),
            ),
            freshness_json: serde_json::json!({}),
        })
        .unwrap();
        let transport = MockGithubReleaseTransport {
            response: RefCell::new(Err(GithubReleaseTransportError::HttpStatus {
                status: 429,
                message: "GitHub API rate limit or access restriction".to_owned(),
            })),
            requests: RefCell::new(Vec::new()),
        };

        let result = read_or_refresh_github_releases_from_transport(
            &db,
            config,
            ProviderCacheMode::ForceRefresh,
            &transport,
        )
        .unwrap();

        assert_eq!(result.source, ProviderCacheSource::Stale);
        assert_eq!(result.releases[0].tag_name, "v1.2.0");
        let codes: Vec<_> = result
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect();
        assert_eq!(codes, vec![CACHE_REFRESH_FAILED, USED_STALE_CACHE]);
        assert!(result.diagnostics[0]
            .message
            .contains("HTTP 429: GitHub API rate limit"));
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
    fn json_operation_without_fixture_uses_live_transport() {
        let db = CacheDb::open_in_memory().unwrap();
        let transport = MockGithubReleaseTransport {
            response: RefCell::new(Ok(GithubReleaseTransportResponse {
                body: GITHUB_RELEASES_FIXTURE.to_owned(),
                etag: Some("W/\"json-live\"".to_owned()),
                last_modified: None,
            })),
            requests: RefCell::new(Vec::new()),
        };
        let request = json!({
            "owner": "example",
            "repo": "app",
            "asset": { "include": "\\.apk$", "exclude": "debug" }
        });

        let response =
            github_releases_json_with_transport(&db, &request.to_string(), &transport).unwrap();

        assert_eq!(transport.requests.borrow().len(), 1);
        assert_eq!(response["source"], "refreshed");
        assert_eq!(response["candidates"][0]["version"], "v1.2.0");
        assert_eq!(response["diagnostics"], json!([]));
        let cache_key = response["cache_key"].as_str().unwrap();
        let cached = db.provider_response(cache_key).unwrap().unwrap();
        assert_eq!(cached.freshness_json["etag"], "W/\"json-live\"");
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

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RecordedGithubReleaseRequest {
        api_base_url: String,
        owner: String,
        repo: String,
    }

    struct MockGithubReleaseTransport {
        response: RefCell<Result<GithubReleaseTransportResponse, GithubReleaseTransportError>>,
        requests: RefCell<Vec<RecordedGithubReleaseRequest>>,
    }

    impl GithubReleaseTransport for MockGithubReleaseTransport {
        fn fetch_releases(
            &self,
            request: &GithubReleaseTransportRequest<'_>,
        ) -> Result<GithubReleaseTransportResponse, GithubReleaseTransportError> {
            self.requests
                .borrow_mut()
                .push(RecordedGithubReleaseRequest {
                    api_base_url: request.api_base_url.to_owned(),
                    owner: request.owner.to_owned(),
                    repo: request.repo.to_owned(),
                });
            self.response.borrow().clone()
        }
    }

    fn serve_one_github_response(
        status: u16,
        headers: &[(&str, &str)],
        body: &'static str,
    ) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let headers = headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect::<Vec<_>>();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0; 4096];
            let bytes = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..bytes]).into_owned();
            let reason = if status == 200 { "OK" } else { "Error" };
            write!(
                stream,
                "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            )
            .unwrap();
            for (name, value) in headers {
                write!(stream, "{name}: {value}\r\n").unwrap();
            }
            write!(stream, "\r\n{body}").unwrap();
            request
        });
        (format!("http://{address}"), handle)
    }
}
