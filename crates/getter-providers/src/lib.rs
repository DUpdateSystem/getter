//! Provider executor scaffolding for the UpgradeAll getter rewrite.
//!
//! Real network/provider execution is intentionally deferred to later slices.
//! The current provider code is fixture-backed and keeps parsing/normalization in
//! Rust getter so Flutter and Android adapter glue do not learn provider formats.

pub use getter_core as core;

use getter_core::{ResolvedPackage, UpdateArtifact, UpdateCandidate};
use roxmltree::{Document, Node};
use serde::{Deserialize, Serialize};

/// Mock provider that returns the static `updates` candidates materialized from
/// a resolved Lua package table.
///
/// This is development scaffolding, not the final live provider model. Keeping
/// it behind a provider-shaped boundary prevents operation code from treating
/// `package.updates` as the product update-check architecture.
#[derive(Debug, Default, Clone, Copy)]
pub struct StaticPackageUpdatesProvider;

impl StaticPackageUpdatesProvider {
    pub fn check_updates(self, package: &ResolvedPackage) -> Vec<UpdateCandidate> {
        package.updates.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FdroidCatalog {
    pub endpoint: FdroidEndpoint,
    pub apps: Vec<FdroidApp>,
}

impl FdroidCatalog {
    pub fn app(&self, package_name: &str) -> Option<&FdroidApp> {
        self.apps
            .iter()
            .find(|app| app.package_name == package_name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FdroidEndpoint {
    pub name: Option<String>,
    pub url: Option<String>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FdroidApp {
    pub package_name: String,
    pub name: Option<String>,
    pub summary: Option<String>,
    pub packages: Vec<FdroidRelease>,
}

impl FdroidApp {
    pub fn update_candidates(&self, endpoint: &FdroidEndpoint) -> Vec<UpdateCandidate> {
        self.packages
            .iter()
            .map(|release| release.update_candidate(endpoint))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FdroidRelease {
    pub version: String,
    pub version_code: Option<i64>,
    pub apk_name: String,
    pub sha256: Option<String>,
    pub size: Option<u64>,
}

impl FdroidRelease {
    fn update_candidate(&self, endpoint: &FdroidEndpoint) -> UpdateCandidate {
        UpdateCandidate {
            version: self.version.clone(),
            version_code: self.version_code,
            channel: None,
            source: Some("fdroid".to_owned()),
            artifacts: vec![UpdateArtifact {
                name: self.apk_name.clone(),
                url: endpoint
                    .url
                    .as_deref()
                    .map(|base| fdroid_artifact_url(base, &self.apk_name))
                    .unwrap_or_else(|| self.apk_name.clone()),
                file_name: Some(self.apk_name.clone()),
                sha256: self.sha256.clone(),
                size: self.size,
            }],
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FdroidCatalogError {
    #[error("failed to parse F-Droid catalog XML: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("F-Droid catalog is missing required field {field} for {context}")]
    MissingField {
        context: String,
        field: &'static str,
    },
    #[error("F-Droid catalog field {field} for {context} is invalid: {value}")]
    InvalidField {
        context: String,
        field: &'static str,
        value: String,
    },
}

pub fn parse_fdroid_index_xml(xml: &str) -> Result<FdroidCatalog, FdroidCatalogError> {
    let document = Document::parse(xml)?;
    let root = document.root_element();
    let repo = child_element(root, "repo");
    let endpoint = FdroidEndpoint {
        name: repo
            .and_then(|node| node.attribute("name"))
            .map(str::to_owned),
        url: repo
            .and_then(|node| node.attribute("url"))
            .map(str::to_owned),
        timestamp: repo
            .and_then(|node| node.attribute("timestamp"))
            .map(str::to_owned),
    };
    let apps = root
        .children()
        .filter(|node| node.has_tag_name("application"))
        .map(parse_fdroid_app)
        .collect::<Result<_, _>>()?;

    Ok(FdroidCatalog { endpoint, apps })
}

fn parse_fdroid_app(app: Node<'_, '_>) -> Result<FdroidApp, FdroidCatalogError> {
    let package_name = app
        .attribute("id")
        .map(str::to_owned)
        .or_else(|| child_text(app, "id"))
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| FdroidCatalogError::MissingField {
            context: "application".to_owned(),
            field: "id",
        })?;
    let packages = app
        .children()
        .filter(|node| node.has_tag_name("package"))
        .map(|package| parse_fdroid_release(&package_name, package))
        .collect::<Result<_, _>>()?;

    Ok(FdroidApp {
        package_name,
        name: child_text(app, "name"),
        summary: child_text(app, "summary"),
        packages,
    })
}

fn parse_fdroid_release(
    package_name: &str,
    package: Node<'_, '_>,
) -> Result<FdroidRelease, FdroidCatalogError> {
    let version = required_child_text(package, package_name, "version")?;
    let apk_name = required_child_text(package, package_name, "apkname")?;
    let version_code = child_text(package, "versioncode")
        .map(|value| parse_i64(package_name, "versioncode", &value))
        .transpose()?;
    let size = child_text(package, "size")
        .map(|value| parse_u64(package_name, "size", &value))
        .transpose()?;
    let sha256 = package
        .children()
        .find(|node| node.has_tag_name("hash") && node.attribute("type") == Some("sha256"))
        .and_then(|node| node.text())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);

    Ok(FdroidRelease {
        version,
        version_code,
        apk_name,
        sha256,
        size,
    })
}

fn required_child_text(
    parent: Node<'_, '_>,
    context: &str,
    field: &'static str,
) -> Result<String, FdroidCatalogError> {
    child_text(parent, field).ok_or_else(|| FdroidCatalogError::MissingField {
        context: context.to_owned(),
        field,
    })
}

fn child_text(parent: Node<'_, '_>, tag: &str) -> Option<String> {
    child_element(parent, tag)
        .and_then(|node| node.text())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn child_element<'a>(parent: Node<'a, 'a>, tag: &str) -> Option<Node<'a, 'a>> {
    parent.children().find(|node| node.has_tag_name(tag))
}

fn parse_i64(context: &str, field: &'static str, value: &str) -> Result<i64, FdroidCatalogError> {
    value.parse().map_err(|_| FdroidCatalogError::InvalidField {
        context: context.to_owned(),
        field,
        value: value.to_owned(),
    })
}

fn parse_u64(context: &str, field: &'static str, value: &str) -> Result<u64, FdroidCatalogError> {
    value.parse().map_err(|_| FdroidCatalogError::InvalidField {
        context: context.to_owned(),
        field,
        value: value.to_owned(),
    })
}

fn fdroid_artifact_url(base: &str, apk_name: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), apk_name)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubRelease {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub assets: Vec<GithubReleaseAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GithubReleaseCandidateOptions {
    pub include_prereleases: bool,
    pub asset_filter: GithubAssetFilter,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GithubAssetFilter {
    pub include: Option<String>,
    pub exclude: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum GithubProviderError {
    #[error("failed to parse GitHub releases JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid GitHub asset {filter_kind} regex '{pattern}': {source}")]
    AssetFilterRegex {
        filter_kind: &'static str,
        pattern: String,
        #[source]
        source: regex::Error,
    },
}

pub fn parse_github_releases_json(json: &str) -> Result<Vec<GithubRelease>, GithubProviderError> {
    Ok(serde_json::from_str(json)?)
}

pub fn github_release_update_candidates(
    releases: &[GithubRelease],
    options: &GithubReleaseCandidateOptions,
) -> Result<Vec<UpdateCandidate>, GithubProviderError> {
    let filter = CompiledGithubAssetFilter::new(&options.asset_filter)?;
    Ok(releases
        .iter()
        .filter(|release| !release.draft)
        .filter(|release| options.include_prereleases || !release.prerelease)
        .filter_map(|release| github_release_update_candidate(release, &filter))
        .collect())
}

fn github_release_update_candidate(
    release: &GithubRelease,
    filter: &CompiledGithubAssetFilter,
) -> Option<UpdateCandidate> {
    let artifacts = release
        .assets
        .iter()
        .filter(|asset| filter.matches(asset))
        .map(github_asset_update_artifact)
        .collect::<Vec<_>>();
    if artifacts.is_empty() {
        return None;
    }

    Some(UpdateCandidate {
        version: release.tag_name.clone(),
        version_code: None,
        channel: release.prerelease.then(|| "prerelease".to_owned()),
        source: Some("github".to_owned()),
        artifacts,
    })
}

fn github_asset_update_artifact(asset: &GithubReleaseAsset) -> UpdateArtifact {
    UpdateArtifact {
        name: asset.name.clone(),
        url: asset.browser_download_url.clone(),
        file_name: Some(asset.name.clone()),
        sha256: github_asset_sha256(asset.digest.as_deref()),
        size: asset.size,
    }
}

fn github_asset_sha256(digest: Option<&str>) -> Option<String> {
    digest
        .and_then(|value| value.strip_prefix("sha256:"))
        .filter(|value| {
            value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit())
        })
        .map(str::to_owned)
}

struct CompiledGithubAssetFilter {
    include: Option<regex::Regex>,
    exclude: Option<regex::Regex>,
}

impl CompiledGithubAssetFilter {
    fn new(filter: &GithubAssetFilter) -> Result<Self, GithubProviderError> {
        Ok(Self {
            include: compile_github_asset_regex("include", filter.include.as_deref())?,
            exclude: compile_github_asset_regex("exclude", filter.exclude.as_deref())?,
        })
    }

    fn matches(&self, asset: &GithubReleaseAsset) -> bool {
        self.include
            .as_ref()
            .is_none_or(|include| include.is_match(&asset.name))
            && !self
                .exclude
                .as_ref()
                .is_some_and(|exclude| exclude.is_match(&asset.name))
    }
}

fn compile_github_asset_regex(
    filter_kind: &'static str,
    pattern: Option<&str>,
) -> Result<Option<regex::Regex>, GithubProviderError> {
    pattern
        .map(|pattern| {
            regex::Regex::new(pattern).map_err(|source| GithubProviderError::AssetFilterRegex {
                filter_kind,
                pattern: pattern.to_owned(),
                source,
            })
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use getter_core::PackagePermissions;

    const FDROID_FIXTURE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<fdroid>
  <repo name="F-Droid" timestamp="1700000000" url="https://f-droid.org/repo" />
  <application id="org.fdroid.fdroid">
    <id>org.fdroid.fdroid</id>
    <name>F-Droid</name>
    <summary>App repository client</summary>
    <package>
      <version>1.20.0</version>
      <versioncode>1020000</versioncode>
      <apkname>org.fdroid.fdroid_1020000.apk</apkname>
      <hash type="sha256">aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</hash>
      <size>1234567</size>
    </package>
    <package>
      <version>1.19.0</version>
      <versioncode>1019000</versioncode>
      <apkname>org.fdroid.fdroid_1019000.apk</apkname>
      <hash type="sha256">bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb</hash>
      <size>1111111</size>
    </package>
  </application>
</fdroid>
"#;

    #[test]
    fn static_provider_returns_package_declared_update_candidates() {
        let package = ResolvedPackage {
            id: "android/org.fdroid.fdroid".parse().unwrap(),
            name: "F-Droid".to_owned(),
            repository: "official".parse().unwrap(),
            installed: Vec::new(),
            permissions: PackagePermissions::default(),
            source_priority: Vec::new(),
            updates: vec![UpdateCandidate {
                version: "1.2.0".to_owned(),
                version_code: None,
                channel: Some("stable".to_owned()),
                source: Some("fixture".to_owned()),
                artifacts: Vec::new(),
            }],
        };

        let candidates = StaticPackageUpdatesProvider.check_updates(&package);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].version, "1.2.0");
        assert_eq!(candidates[0].source.as_deref(), Some("fixture"));
    }

    #[test]
    fn parses_fdroid_index_xml_into_catalog_facts() {
        let catalog = parse_fdroid_index_xml(FDROID_FIXTURE).unwrap();

        assert_eq!(catalog.endpoint.name.as_deref(), Some("F-Droid"));
        assert_eq!(
            catalog.endpoint.url.as_deref(),
            Some("https://f-droid.org/repo")
        );
        let app = catalog.app("org.fdroid.fdroid").unwrap();
        assert_eq!(app.name.as_deref(), Some("F-Droid"));
        assert_eq!(app.summary.as_deref(), Some("App repository client"));
        assert_eq!(app.packages.len(), 2);
        assert_eq!(app.packages[0].version, "1.20.0");
        assert_eq!(app.packages[0].version_code, Some(1020000));
        assert_eq!(
            app.packages[0].sha256.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(app.packages[0].size, Some(1234567));
    }

    #[test]
    fn normalizes_fdroid_releases_to_update_candidates() {
        let catalog = parse_fdroid_index_xml(FDROID_FIXTURE).unwrap();
        let app = catalog.app("org.fdroid.fdroid").unwrap();

        let candidates = app.update_candidates(&catalog.endpoint);

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].version, "1.20.0");
        assert_eq!(candidates[0].version_code, Some(1020000));
        assert_eq!(candidates[0].source.as_deref(), Some("fdroid"));
        let artifact = &candidates[0].artifacts[0];
        assert_eq!(artifact.name, "org.fdroid.fdroid_1020000.apk");
        assert_eq!(
            artifact.url,
            "https://f-droid.org/repo/org.fdroid.fdroid_1020000.apk"
        );
        assert_eq!(
            artifact.file_name.as_deref(),
            Some("org.fdroid.fdroid_1020000.apk")
        );
        assert_eq!(
            artifact.sha256.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(artifact.size, Some(1234567));
    }

    #[test]
    fn malformed_fdroid_index_reports_parse_error() {
        let error = parse_fdroid_index_xml("<fdroid><application></fdroid>").unwrap_err();

        assert!(matches!(error, FdroidCatalogError::Xml(_)));
    }

    #[test]
    fn parses_github_releases_json_into_provider_facts() {
        let releases = parse_github_releases_json(GITHUB_RELEASES_FIXTURE).unwrap();

        assert_eq!(releases.len(), 3);
        let release = &releases[0];
        assert_eq!(release.tag_name, "v1.2.0");
        assert_eq!(release.name.as_deref(), Some("Release 1.2.0"));
        assert_eq!(release.body.as_deref(), Some("Release notes"));
        assert!(!release.draft);
        assert!(!release.prerelease);
        assert_eq!(
            release.published_at.as_deref(),
            Some("2026-06-01T00:00:00Z")
        );
        assert_eq!(release.assets.len(), 3);
        let asset = &release.assets[0];
        assert_eq!(asset.name, "app-release.apk");
        assert_eq!(
            asset.content_type.as_deref(),
            Some("application/vnd.android.package-archive")
        );
        assert_eq!(asset.size, Some(1234));
        assert_eq!(
            asset.browser_download_url,
            "https://github.com/example/app/releases/download/v1.2.0/app-release.apk"
        );
        assert_eq!(
            asset.digest.as_deref(),
            Some("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
    }

    #[test]
    fn normalizes_github_release_assets_to_update_candidates_with_filters() {
        let releases = parse_github_releases_json(GITHUB_RELEASES_FIXTURE).unwrap();
        let options = GithubReleaseCandidateOptions {
            asset_filter: GithubAssetFilter {
                include: Some(r"\.apk$".to_owned()),
                exclude: Some("debug".to_owned()),
            },
            ..Default::default()
        };

        let candidates = github_release_update_candidates(&releases, &options).unwrap();

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].version, "v1.2.0");
        assert_eq!(candidates[0].source.as_deref(), Some("github"));
        assert_eq!(candidates[0].artifacts.len(), 1);
        let artifact = &candidates[0].artifacts[0];
        assert_eq!(artifact.name, "app-release.apk");
        assert_eq!(
            artifact.url,
            "https://github.com/example/app/releases/download/v1.2.0/app-release.apk"
        );
        assert_eq!(artifact.file_name.as_deref(), Some("app-release.apk"));
        assert_eq!(
            artifact.sha256.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(artifact.size, Some(1234));
    }

    #[test]
    fn ignores_unrecognized_github_asset_digests() {
        let releases = parse_github_releases_json(GITHUB_INVALID_DIGEST_FIXTURE).unwrap();

        let candidates = github_release_update_candidates(
            &releases,
            &GithubReleaseCandidateOptions {
                asset_filter: GithubAssetFilter {
                    include: Some(r"\.apk$".to_owned()),
                    exclude: None,
                },
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(candidates[0].artifacts[0].sha256, None);
    }

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
      },
      {
        "name": "notes.txt",
        "content_type": "text/plain",
        "size": 345,
        "browser_download_url": "https://github.com/example/app/releases/download/v1.2.0/notes.txt"
      }
    ]
  },
  {
    "tag_name": "v1.3.0-beta1",
    "draft": false,
    "prerelease": true,
    "assets": [
      {
        "name": "app-beta.apk",
        "size": 456,
        "browser_download_url": "https://github.com/example/app/releases/download/v1.3.0-beta1/app-beta.apk"
      }
    ]
  },
  {
    "tag_name": "v1.4.0-draft",
    "draft": true,
    "prerelease": false,
    "assets": [
      {
        "name": "app-draft.apk",
        "size": 567,
        "browser_download_url": "https://github.com/example/app/releases/download/v1.4.0-draft/app-draft.apk"
      }
    ]
  }
]"#;

    const GITHUB_INVALID_DIGEST_FIXTURE: &str = r#"[
  {
    "tag_name": "v1.2.0",
    "draft": false,
    "prerelease": false,
    "assets": [
      {
        "name": "app-release.apk",
        "size": 1234,
        "browser_download_url": "https://github.com/example/app/releases/download/v1.2.0/app-release.apk",
        "digest": "sha256:not-a-valid-sha256"
      }
    ]
  }
]"#;
}
