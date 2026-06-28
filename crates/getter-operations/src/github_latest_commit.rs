//! Fixture-backed GitHub latest-commit cache/query operations.
//!
//! This ADR-0012 provider slice models latest-commit checks as live/floating
//! provider facts. It parses controlled GitHub commit fixtures through the
//! shared provider cache without producing ordinary release candidates, runtime
//! actions, downloads, installer handoffs, or Flutter/Kotlin provider parsing.

use crate::provider_cache::{
    read_or_refresh_provider_response_with_provenance, source_response_sha512,
    ProviderCacheDiagnostic, ProviderCacheMode, ProviderCacheOperationError, ProviderCacheRequest,
    ProviderCacheSource, ProviderResponseRefresh,
};
use getter_providers::{
    github_latest_commit_live_revision, parse_github_commit_json, GithubCommit, GithubLiveRevision,
    GithubProviderError,
};
use getter_storage::{CacheDb, StorageError};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha512};

pub const GITHUB_PROVIDER_ID: &str = "github";
pub const GITHUB_LATEST_COMMIT_CACHE_VERSION: &str = "github-latest-commit-v1";
pub const DEFAULT_GITHUB_API_BASE_URL: &str = "https://api.github.com";
pub const DEFAULT_GITHUB_REF: &str = "HEAD";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubLatestCommitConfig {
    pub api_base_url: String,
    pub owner: String,
    pub repo: String,
    pub reference: String,
}

impl GithubLatestCommitConfig {
    pub fn cache_key(&self) -> String {
        format!(
            "{GITHUB_PROVIDER_ID}:{GITHUB_LATEST_COMMIT_CACHE_VERSION}:latest_commit:{}:{}/{}/{}",
            digest_hex(&self.api_base_url),
            self.owner,
            self.repo,
            self.reference,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubLatestCommitResult {
    pub config: GithubLatestCommitConfig,
    pub cache_key: String,
    pub commit: GithubCommit,
    pub live_revision: GithubLiveRevision,
    pub source: ProviderCacheSource,
    pub source_response_sha512: Vec<String>,
    pub provenance_schema_version: Option<String>,
    pub diagnostics: Vec<ProviderCacheDiagnostic>,
}

#[derive(Debug, thiserror::Error)]
pub enum GithubLatestCommitOperationError {
    #[error("provider cache operation failed: {0}")]
    Cache(#[from] ProviderCacheOperationError),
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("GitHub provider parse failed: {0}")]
    Provider(#[from] GithubProviderError),
    #[error("GitHub provider serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid GitHub latest-commit request: {0}")]
    InvalidRequest(String),
}

pub fn read_or_refresh_github_latest_commit<F>(
    db: &CacheDb,
    config: GithubLatestCommitConfig,
    mode: ProviderCacheMode,
    refresh_json: F,
) -> Result<GithubLatestCommitResult, GithubLatestCommitOperationError>
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
            let commit = parse_github_commit_json(&json).map_err(|source| source.to_string())?;
            github_latest_commit_live_revision(&commit).map_err(|source| source.to_string())?;
            let response_json =
                serde_json::to_value(commit).map_err(|source| source.to_string())?;
            Ok(ProviderResponseRefresh {
                response_json,
                source_response_sha512: vec![source_digest],
                freshness_json: serde_json::json!({}),
            })
        },
    )?;
    let response = cache_result.response;
    let commit = serde_json::from_value(response.response_json)?;
    let live_revision = github_latest_commit_live_revision(&commit)?;

    Ok(GithubLatestCommitResult {
        config,
        cache_key,
        commit,
        live_revision,
        source: cache_result.source,
        source_response_sha512: response.source_response_sha512,
        provenance_schema_version: response.provenance_schema_version,
        diagnostics: cache_result.diagnostics,
    })
}

pub fn github_latest_commit_json(
    db: &CacheDb,
    request_json: &str,
) -> Result<Value, GithubLatestCommitOperationError> {
    let request: GithubLatestCommitJsonRequest = serde_json::from_str(request_json)?;
    let owner = required_non_empty(request.owner, "owner")?;
    let repo = required_non_empty(request.repo, "repo")?;
    let reference = request
        .reference
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_GITHUB_REF.to_owned());
    let config = GithubLatestCommitConfig {
        api_base_url: request
            .api_base_url
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_GITHUB_API_BASE_URL.to_owned()),
        owner,
        repo,
        reference,
    };
    let mode = match request.mode.as_deref() {
        Some("force_refresh") => ProviderCacheMode::ForceRefresh,
        Some("use_cached") | None => ProviderCacheMode::UseCached,
        Some(other) => {
            return Err(GithubLatestCommitOperationError::InvalidRequest(format!(
                "unknown mode '{other}'"
            )))
        }
    };
    let result = read_or_refresh_github_latest_commit(db, config, mode, || {
        request.commit_json.ok_or_else(|| {
            "fixture-backed GitHub latest-commit refresh requires commit_json".to_owned()
        })
    })?;

    Ok(json!({
        "operation": "github.latest_commit",
        "provider": GITHUB_PROVIDER_ID,
        "api_base_url": result.config.api_base_url,
        "owner": result.config.owner,
        "repo": result.config.repo,
        "ref": result.config.reference,
        "cache_key": result.cache_key,
        "source": provider_source_json(result.source),
        "live": true,
        "version": result.live_revision.version,
        "revision": result.live_revision.revision,
        "latest_commit": result.live_revision,
        "commit": result.commit,
        "diagnostics": result.diagnostics.iter().map(diagnostic_json).collect::<Vec<_>>(),
    }))
}

#[derive(Debug, Deserialize)]
struct GithubLatestCommitJsonRequest {
    #[serde(default)]
    api_base_url: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    repo: Option<String>,
    #[serde(default, rename = "ref")]
    reference: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    commit_json: Option<String>,
}

fn required_non_empty(
    value: Option<String>,
    field: &'static str,
) -> Result<String, GithubLatestCommitOperationError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| GithubLatestCommitOperationError::InvalidRequest(format!("missing {field}")))
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
    use std::cell::Cell;

    const GITHUB_COMMIT_FIXTURE: &str = r#"{
  "sha": "0123456789abcdef0123456789abcdef01234567",
  "html_url": "https://github.com/DUpdateSystem/UpgradeAll/commit/0123456789abcdef0123456789abcdef01234567",
  "commit": {
    "message": "Update app metadata",
    "author": {
      "date": "2026-06-02T03:04:05Z"
    },
    "committer": {
      "date": "2026-06-02T04:05:06Z"
    }
  }
}"#;

    const UPDATED_COMMIT_FIXTURE: &str = r#"{
  "sha": "fedcba9876543210fedcba9876543210fedcba98",
  "html_url": "https://github.com/DUpdateSystem/UpgradeAll/commit/fedcba9876543210fedcba9876543210fedcba98",
  "commit": {
    "message": "Refresh generated packages",
    "committer": {
      "date": "2026-06-03T03:04:05Z"
    }
  }
}"#;

    const EMPTY_SHA_COMMIT_FIXTURE: &str = r#"{
  "sha": "   ",
  "commit": {
    "message": "Broken fixture"
  }
}"#;

    fn config() -> GithubLatestCommitConfig {
        GithubLatestCommitConfig {
            api_base_url: DEFAULT_GITHUB_API_BASE_URL.to_owned(),
            owner: "DUpdateSystem".to_owned(),
            repo: "UpgradeAll".to_owned(),
            reference: DEFAULT_GITHUB_REF.to_owned(),
        }
    }

    #[test]
    fn cache_key_identifies_latest_commit_request_and_ref() {
        let main = config();
        let mut branch = config();
        branch.reference = "main".to_owned();
        let releases = crate::github_releases::GithubReleaseConfig {
            api_base_url: crate::github_releases::DEFAULT_GITHUB_API_BASE_URL.to_owned(),
            owner: main.owner.clone(),
            repo: main.repo.clone(),
        };

        assert!(main
            .cache_key()
            .contains(GITHUB_LATEST_COMMIT_CACHE_VERSION));
        assert!(main.cache_key().contains(":latest_commit:"));
        assert_ne!(main.cache_key(), branch.cache_key());
        assert_ne!(main.cache_key(), releases.cache_key());
    }

    #[test]
    fn cache_miss_parses_and_stores_fixture_commit_as_live_revision() {
        let db = CacheDb::open_in_memory().unwrap();
        let config = config();
        let cache_key = config.cache_key();

        let result =
            read_or_refresh_github_latest_commit(&db, config, ProviderCacheMode::UseCached, || {
                Ok(GITHUB_COMMIT_FIXTURE.to_owned())
            })
            .unwrap();

        assert_eq!(result.source, ProviderCacheSource::Refreshed);
        assert!(result.live_revision.live);
        assert_eq!(
            result.live_revision.revision,
            "0123456789abcdef0123456789abcdef01234567"
        );
        assert_eq!(
            result.live_revision.published_at.as_deref(),
            Some("2026-06-02T03:04:05Z")
        );
        let cached = db.provider_response(&cache_key).unwrap().unwrap();
        assert_eq!(cached.provider, GITHUB_PROVIDER_ID);
        assert_eq!(
            cached.response_json["sha"],
            "0123456789abcdef0123456789abcdef01234567"
        );
    }

    #[test]
    fn cache_hit_avoids_refreshing_fixture_commit() {
        let db = CacheDb::open_in_memory().unwrap();
        read_or_refresh_github_latest_commit(&db, config(), ProviderCacheMode::UseCached, || {
            Ok(GITHUB_COMMIT_FIXTURE.to_owned())
        })
        .unwrap();
        let refreshed = Cell::new(false);

        let result = read_or_refresh_github_latest_commit(
            &db,
            config(),
            ProviderCacheMode::UseCached,
            || {
                refreshed.set(true);
                Ok(UPDATED_COMMIT_FIXTURE.to_owned())
            },
        )
        .unwrap();

        assert!(!refreshed.get());
        assert_eq!(result.source, ProviderCacheSource::Cache);
        assert_eq!(
            result.live_revision.revision,
            "0123456789abcdef0123456789abcdef01234567"
        );
    }

    #[test]
    fn forced_refresh_replaces_cached_fixture_commit() {
        let db = CacheDb::open_in_memory().unwrap();
        read_or_refresh_github_latest_commit(&db, config(), ProviderCacheMode::UseCached, || {
            Ok(GITHUB_COMMIT_FIXTURE.to_owned())
        })
        .unwrap();

        let result = read_or_refresh_github_latest_commit(
            &db,
            config(),
            ProviderCacheMode::ForceRefresh,
            || Ok(UPDATED_COMMIT_FIXTURE.to_owned()),
        )
        .unwrap();

        assert_eq!(result.source, ProviderCacheSource::Refreshed);
        assert_eq!(
            result.live_revision.revision,
            "fedcba9876543210fedcba9876543210fedcba98"
        );
    }

    #[test]
    fn forced_refresh_failure_returns_stale_commit_with_diagnostics() {
        let db = CacheDb::open_in_memory().unwrap();
        read_or_refresh_github_latest_commit(&db, config(), ProviderCacheMode::UseCached, || {
            Ok(GITHUB_COMMIT_FIXTURE.to_owned())
        })
        .unwrap();

        let result = read_or_refresh_github_latest_commit(
            &db,
            config(),
            ProviderCacheMode::ForceRefresh,
            || Err("HTTP 503".to_owned()),
        )
        .unwrap();

        assert_eq!(result.source, ProviderCacheSource::Stale);
        assert_eq!(
            result.live_revision.revision,
            "0123456789abcdef0123456789abcdef01234567"
        );
        let codes: Vec<_> = result
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect();
        assert_eq!(codes, vec![CACHE_REFRESH_FAILED, USED_STALE_CACHE]);
    }

    #[test]
    fn cache_miss_with_empty_sha_is_an_error_and_does_not_store_cache() {
        let db = CacheDb::open_in_memory().unwrap();
        let config = config();
        let cache_key = config.cache_key();

        let error =
            read_or_refresh_github_latest_commit(&db, config, ProviderCacheMode::UseCached, || {
                Ok(EMPTY_SHA_COMMIT_FIXTURE.to_owned())
            })
            .unwrap_err();

        assert!(matches!(
            error,
            GithubLatestCommitOperationError::Cache(ProviderCacheOperationError::RefreshFailed(_))
        ));
        assert!(db.provider_response(&cache_key).unwrap().is_none());
    }

    #[test]
    fn forced_refresh_with_empty_sha_returns_stale_commit_with_diagnostics() {
        let db = CacheDb::open_in_memory().unwrap();
        read_or_refresh_github_latest_commit(&db, config(), ProviderCacheMode::UseCached, || {
            Ok(GITHUB_COMMIT_FIXTURE.to_owned())
        })
        .unwrap();

        let result = read_or_refresh_github_latest_commit(
            &db,
            config(),
            ProviderCacheMode::ForceRefresh,
            || Ok(EMPTY_SHA_COMMIT_FIXTURE.to_owned()),
        )
        .unwrap();

        assert_eq!(result.source, ProviderCacheSource::Stale);
        assert_eq!(
            result.live_revision.revision,
            "0123456789abcdef0123456789abcdef01234567"
        );
        let codes: Vec<_> = result
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect();
        assert_eq!(codes, vec![CACHE_REFRESH_FAILED, USED_STALE_CACHE]);
    }

    #[test]
    fn json_operation_returns_live_revision_from_cached_commit() {
        let db = CacheDb::open_in_memory().unwrap();
        let first = github_latest_commit_json(
            &db,
            &json!({
                "owner": "DUpdateSystem",
                "repo": "UpgradeAll",
                "commit_json": GITHUB_COMMIT_FIXTURE
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(first["source"], "refreshed");

        let second = github_latest_commit_json(
            &db,
            &json!({
                "owner": "DUpdateSystem",
                "repo": "UpgradeAll"
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(second["operation"], "github.latest_commit");
        assert_eq!(second["ref"], DEFAULT_GITHUB_REF);
        assert_eq!(second["source"], "cache");
        assert_eq!(second["live"], true);
        assert!(second.get("candidates").is_none());
        assert!(second.get("selected_update").is_none());
        assert!(second.get("artifacts").is_none());
        assert!(second.get("actions").is_none());
        assert!(second.get("action_id").is_none());
        assert_eq!(
            second["revision"],
            "0123456789abcdef0123456789abcdef01234567"
        );
        assert_eq!(
            second["latest_commit"]["published_at"],
            "2026-06-02T03:04:05Z"
        );
        assert!(second["latest_commit"]["live"].as_bool().unwrap());
    }
}
