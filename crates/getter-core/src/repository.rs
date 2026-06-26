//! Repository layout loading for Lua package repositories.

use crate::{PackageId, PackageIdError, RepositoryId, RepositoryIdError, RepositoryPriority};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const REPO_API_VERSION_V1: &str = "getter.repo.v1";
pub const MAIN_DB_FILE: &str = "main.db";
pub const CACHE_DB_FILE: &str = "cache.db";
pub const REPOSITORY_ROOT_DIR: &str = "repo";
pub const RUNTIME_CONFIG_DIR: &str = "rc";
pub const REPOSITORY_ROOT_METADATA_FILE: &str = "metadata.jsonc";
pub const REPOSITORY_ROOT_METADATA_VERSION: u32 = 1;
pub const LOCAL_REPOSITORY_ALIAS: &str = "local";
pub const DEFAULT_GENERATED_REPOSITORY_ALIAS: &str = "autogen";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryLayout {
    pub root: PathBuf,
    pub metadata: RepositoryMetadata,
    pub packages_dir: PathBuf,
    pub lib_dir: PathBuf,
    pub templates_dir: PathBuf,
    pub packages: Vec<PackageFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryMetadata {
    pub id: RepositoryId,
    pub name: String,
    pub priority: RepositoryPriority,
    pub api_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetterDataDirLayout {
    pub root: PathBuf,
    pub main_db: PathBuf,
    pub cache_db: PathBuf,
    pub repository_root: PathBuf,
    pub runtime_config_root: PathBuf,
}

impl GetterDataDirLayout {
    pub fn new(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        Self {
            main_db: root.join(MAIN_DB_FILE),
            cache_db: root.join(CACHE_DB_FILE),
            repository_root: root.join(REPOSITORY_ROOT_DIR),
            runtime_config_root: root.join(RUNTIME_CONFIG_DIR),
            root,
        }
    }

    pub fn repository_path(&self, alias: &RepositoryId) -> PathBuf {
        self.repository_root.join(alias.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRootConfig {
    pub generated_repository: RepositoryId,
    priority: HashMap<String, RepositoryPriority>,
}

impl RepositoryRootConfig {
    pub fn load(repository_root: impl AsRef<Path>) -> Result<Self, RepositoryLoadError> {
        let path = repository_root.as_ref().join(REPOSITORY_ROOT_METADATA_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = fs::read(&path).map_err(|source| RepositoryLoadError::ReadRootMetadata {
            path: path.clone(),
            source,
        })?;
        let raw: RawRepositoryRootConfig = serde_json::from_reader(
            json_comments::StripComments::new(bytes.as_slice()),
        )
        .map_err(|source| RepositoryLoadError::ParseRootMetadata {
            path: path.clone(),
            source,
        })?;
        if raw.version != REPOSITORY_ROOT_METADATA_VERSION {
            return Err(RepositoryLoadError::UnsupportedRootMetadataVersion {
                path,
                found: raw.version,
                expected: REPOSITORY_ROOT_METADATA_VERSION,
            });
        }
        let priority = raw
            .priority
            .into_iter()
            .map(|(alias, value)| (alias, RepositoryPriority::new(value)))
            .collect();
        Ok(Self {
            generated_repository: RepositoryId::new(
                raw.generated_repository
                    .unwrap_or_else(|| DEFAULT_GENERATED_REPOSITORY_ALIAS.to_owned()),
            )?,
            priority,
        })
    }

    pub fn priority_for(&self, alias: &RepositoryId) -> RepositoryPriority {
        self.priority
            .get(alias.as_str())
            .copied()
            .unwrap_or_else(|| default_repository_priority(alias.as_str()))
    }
}

impl Default for RepositoryRootConfig {
    fn default() -> Self {
        Self {
            generated_repository: RepositoryId::new(DEFAULT_GENERATED_REPOSITORY_ALIAS)
                .expect("default generated repository alias is valid"),
            priority: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryAliasEntry {
    pub alias: RepositoryId,
    pub path: PathBuf,
    pub priority: RepositoryPriority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRootLayout {
    pub root: PathBuf,
    pub config: RepositoryRootConfig,
    pub repositories: Vec<RepositoryAliasEntry>,
}

impl RepositoryRootLayout {
    pub fn load(root: impl AsRef<Path>) -> Result<Self, RepositoryLoadError> {
        let root = root.as_ref().to_path_buf();
        let config = RepositoryRootConfig::load(&root)?;
        let mut repositories = Vec::new();
        match fs::read_dir(&root) {
            Ok(entries) => {
                for entry in entries {
                    let entry =
                        entry.map_err(|source| RepositoryLoadError::ReadRepositoryRoot {
                            path: root.clone(),
                            source,
                        })?;
                    let file_type = entry.file_type().map_err(|source| {
                        RepositoryLoadError::ReadRepositoryRoot {
                            path: root.clone(),
                            source,
                        }
                    })?;
                    if !file_type.is_dir() {
                        continue;
                    }
                    let alias_text = entry.file_name().to_string_lossy().into_owned();
                    let alias = RepositoryId::new(alias_text)?;
                    let priority = config.priority_for(&alias);
                    repositories.push(RepositoryAliasEntry {
                        alias,
                        path: entry.path(),
                        priority,
                    });
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(RepositoryLoadError::ReadRepositoryRoot {
                    path: root.clone(),
                    source,
                })
            }
        }
        repositories.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.alias.as_str().cmp(right.alias.as_str()))
        });
        Ok(Self {
            root,
            config,
            repositories,
        })
    }
}

pub fn default_repository_priority(alias: &str) -> RepositoryPriority {
    match alias {
        LOCAL_REPOSITORY_ALIAS => RepositoryPriority::LOCAL,
        DEFAULT_GENERATED_REPOSITORY_ALIAS => RepositoryPriority::GENERATED_FALLBACK,
        _ => RepositoryPriority::DEFAULT,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeneratedRepositoryTarget {
    CreateDefault { alias: RepositoryId, path: PathBuf },
    Existing { alias: RepositoryId, path: PathBuf },
}

pub fn generated_repository_target(
    data_dir: impl AsRef<Path>,
) -> Result<GeneratedRepositoryTarget, RepositoryLoadError> {
    let layout = GetterDataDirLayout::new(data_dir);
    let config = RepositoryRootConfig::load(&layout.repository_root)?;
    let path = layout.repository_path(&config.generated_repository);
    if config.generated_repository.as_str() == DEFAULT_GENERATED_REPOSITORY_ALIAS {
        Ok(GeneratedRepositoryTarget::CreateDefault {
            alias: config.generated_repository,
            path,
        })
    } else if path.is_dir() {
        Ok(GeneratedRepositoryTarget::Existing {
            alias: config.generated_repository,
            path,
        })
    } else {
        Err(RepositoryLoadError::MissingGeneratedRepository {
            alias: config.generated_repository,
            path,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageFile {
    pub id: PackageId,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryPackageCacheKey {
    pub repository_id: RepositoryId,
    pub package_id: PackageId,
    pub api_version: String,
    pub package_file_hash: String,
}

#[derive(Debug, Deserialize)]
struct RawRepositoryRootConfig {
    version: u32,
    #[serde(default)]
    generated_repository: Option<String>,
    #[serde(default)]
    priority: HashMap<String, i32>,
}

#[derive(Debug, thiserror::Error)]
pub enum RepositoryLoadError {
    #[error("failed to read repository root metadata at {path}: {source}")]
    ReadRootMetadata {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse repository root metadata at {path}: {source}")]
    ParseRootMetadata {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("unsupported repository root metadata version {found} at {path}; expected {expected}")]
    UnsupportedRootMetadataVersion {
        path: PathBuf,
        found: u32,
        expected: u32,
    },
    #[error("failed to read repository root {path}: {source}")]
    ReadRepositoryRoot {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("configured generated repository '{alias}' does not exist at {path}")]
    MissingGeneratedRepository { alias: RepositoryId, path: PathBuf },
    #[error("failed to read repo.toml at {path}: {source}")]
    ReadRepoToml {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse repo.toml at {path}: {source}")]
    ParseRepoToml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid repository id in repo.toml: {0}")]
    RepositoryId(#[from] RepositoryIdError),
    #[error("unsupported repository api_version '{0}'")]
    UnsupportedApiVersion(String),
    #[error("repository path {path} is missing required directory '{directory}'")]
    MissingDirectory {
        path: PathBuf,
        directory: &'static str,
    },
    #[error("failed to read packages directory {path}: {source}")]
    ReadPackagesDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid package path {path}: {reason}")]
    InvalidPackagePath { path: PathBuf, reason: String },
    #[error("invalid package id derived from {path}: {source}")]
    PackageId {
        path: PathBuf,
        #[source]
        source: PackageIdError,
    },
    #[error("failed to hash package file {path}: {source}")]
    HashPackageFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl RepositoryLayout {
    pub fn load(root: impl AsRef<Path>) -> Result<Self, RepositoryLoadError> {
        let root = root.as_ref().to_path_buf();
        let repo_toml_path = root.join("repo.toml");
        let raw = fs::read_to_string(&repo_toml_path).map_err(|source| {
            RepositoryLoadError::ReadRepoToml {
                path: repo_toml_path.clone(),
                source,
            }
        })?;
        let raw_metadata: RawRepositoryMetadata =
            toml::from_str(&raw).map_err(|source| RepositoryLoadError::ParseRepoToml {
                path: repo_toml_path.clone(),
                source,
            })?;
        let api_version = raw_metadata.api_version;
        if api_version != REPO_API_VERSION_V1 {
            return Err(RepositoryLoadError::UnsupportedApiVersion(api_version));
        }
        let metadata = RepositoryMetadata {
            id: RepositoryId::new(raw_metadata.id)?,
            name: raw_metadata.name,
            priority: RepositoryPriority::new(raw_metadata.priority.unwrap_or_default()),
            api_version,
        };

        let packages_dir = root.join("packages");
        let lib_dir = root.join("lib");
        let templates_dir = root.join("templates");
        require_dir(&root, &packages_dir, "packages")?;
        require_dir(&root, &lib_dir, "lib")?;
        require_dir(&root, &templates_dir, "templates")?;

        let mut packages = Vec::new();
        collect_package_files(&packages_dir, &packages_dir, &mut packages)?;
        packages.sort_by_key(|package| package.id.to_string());

        Ok(Self {
            root,
            metadata,
            packages_dir,
            lib_dir,
            templates_dir,
            packages,
        })
    }

    pub fn package_file(&self, id: &PackageId) -> Option<&PackageFile> {
        self.packages.iter().find(|package| &package.id == id)
    }
}

#[derive(Debug, Deserialize)]
struct RawRepositoryMetadata {
    id: String,
    name: String,
    #[serde(default)]
    priority: Option<i32>,
    api_version: String,
}

fn require_dir(
    root: &Path,
    path: &Path,
    directory: &'static str,
) -> Result<(), RepositoryLoadError> {
    if path.is_dir() {
        Ok(())
    } else {
        Err(RepositoryLoadError::MissingDirectory {
            path: root.to_path_buf(),
            directory,
        })
    }
}

fn collect_package_files(
    packages_root: &Path,
    current: &Path,
    out: &mut Vec<PackageFile>,
) -> Result<(), RepositoryLoadError> {
    for entry in fs::read_dir(current).map_err(|source| RepositoryLoadError::ReadPackagesDir {
        path: current.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| RepositoryLoadError::ReadPackagesDir {
            path: current.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type =
            entry
                .file_type()
                .map_err(|source| RepositoryLoadError::ReadPackagesDir {
                    path: current.to_path_buf(),
                    source,
                })?;
        if file_type.is_dir() {
            collect_package_files(packages_root, &path, out)?;
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "lua") {
            let id = package_id_from_path(packages_root, &path)?;
            out.push(PackageFile { id, path });
        }
    }
    Ok(())
}

pub fn package_id_from_path(
    packages_root: impl AsRef<Path>,
    path: impl AsRef<Path>,
) -> Result<PackageId, RepositoryLoadError> {
    let packages_root = packages_root.as_ref();
    let path = path.as_ref();
    let relative =
        path.strip_prefix(packages_root)
            .map_err(|_| RepositoryLoadError::InvalidPackagePath {
                path: path.to_path_buf(),
                reason: format!("path is not under {}", packages_root.display()),
            })?;
    if relative.extension().is_none_or(|ext| ext != "lua") {
        return Err(RepositoryLoadError::InvalidPackagePath {
            path: path.to_path_buf(),
            reason: "package file must have .lua extension".to_owned(),
        });
    }
    let without_extension = relative.with_extension("");
    let mut parts = without_extension.components();
    let kind = parts
        .next()
        .ok_or_else(|| RepositoryLoadError::InvalidPackagePath {
            path: path.to_path_buf(),
            reason: "missing package kind directory".to_owned(),
        })?
        .as_os_str()
        .to_string_lossy()
        .into_owned();
    let name_path: PathBuf = parts.collect();
    let name = name_path
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    if name.is_empty() {
        return Err(RepositoryLoadError::InvalidPackagePath {
            path: path.to_path_buf(),
            reason: "missing package name".to_owned(),
        });
    }
    format!("{kind}/{name}")
        .parse()
        .map_err(|source| RepositoryLoadError::PackageId {
            path: path.to_path_buf(),
            source,
        })
}

pub fn package_cache_key(
    repository: &RepositoryLayout,
    package: &PackageFile,
) -> Result<RepositoryPackageCacheKey, RepositoryLoadError> {
    Ok(RepositoryPackageCacheKey {
        repository_id: repository.metadata.id.clone(),
        package_id: package.id.clone(),
        api_version: repository.metadata.api_version.clone(),
        package_file_hash: package_file_content_hash(&package.path)?,
    })
}

pub fn package_file_content_hash(path: impl AsRef<Path>) -> Result<String, RepositoryLoadError> {
    let path = path.as_ref();
    let bytes = fs::read(path).map_err(|source| RepositoryLoadError::HashPackageFile {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(format!("{:016x}", fnv1a64(&bytes)))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn highest_priority<T, F>(items: &[T], priority: F) -> Option<&T>
where
    F: Fn(&T) -> RepositoryPriority,
{
    items.iter().max_by_key(|item| priority(item))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn data_dir_layout_uses_repo_and_rc_roots() {
        let layout = GetterDataDirLayout::new("/tmp/ua-getter");

        assert_eq!(layout.main_db, PathBuf::from("/tmp/ua-getter/main.db"));
        assert_eq!(layout.cache_db, PathBuf::from("/tmp/ua-getter/cache.db"));
        assert_eq!(layout.repository_root, PathBuf::from("/tmp/ua-getter/repo"));
        assert_eq!(
            layout.runtime_config_root,
            PathBuf::from("/tmp/ua-getter/rc")
        );
    }

    #[test]
    fn missing_root_metadata_uses_default_priorities_and_generated_target() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("community")).unwrap();
        fs::create_dir_all(root.join("local")).unwrap();
        fs::create_dir_all(root.join("autogen")).unwrap();

        let layout = RepositoryRootLayout::load(&root).unwrap();

        assert_eq!(
            layout.config.generated_repository.as_str(),
            DEFAULT_GENERATED_REPOSITORY_ALIAS
        );
        assert_eq!(
            layout
                .repositories
                .iter()
                .map(|repo| (repo.alias.as_str().to_owned(), repo.priority.value()))
                .collect::<Vec<_>>(),
            vec![
                ("local".to_owned(), 100),
                ("community".to_owned(), 0),
                ("autogen".to_owned(), -1),
            ]
        );
    }

    #[test]
    fn root_metadata_priority_map_is_lookup_only_for_existing_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("official")).unwrap();
        fs::write(
            root.join(REPOSITORY_ROOT_METADATA_FILE),
            r#"{
  "version": 1,
  // This entry must not create or sort a missing repository.
  "generated_repository": "autogen",
  "priority": {
    "missing": 500,
    "not an alias": 400,
    "official": 7
  }
}
"#,
        )
        .unwrap();

        let layout = RepositoryRootLayout::load(&root).unwrap();

        assert_eq!(layout.repositories.len(), 1);
        assert_eq!(layout.repositories[0].alias.as_str(), "official");
        assert_eq!(layout.repositories[0].priority.value(), 7);
        assert_eq!(
            layout.config.generated_repository.as_str(),
            DEFAULT_GENERATED_REPOSITORY_ALIAS
        );
    }

    #[test]
    fn malformed_root_metadata_is_an_error_instead_of_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(REPOSITORY_ROOT_METADATA_FILE), "{not-json").unwrap();

        assert!(matches!(
            RepositoryRootLayout::load(&root),
            Err(RepositoryLoadError::ParseRootMetadata { .. })
        ));
    }

    #[test]
    fn generated_repository_target_creates_only_default_alias() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();

        let target = generated_repository_target(data_dir).unwrap();

        assert!(matches!(
            target,
            GeneratedRepositoryTarget::CreateDefault { ref alias, ref path }
                if alias.as_str() == DEFAULT_GENERATED_REPOSITORY_ALIAS
                    && path == &data_dir.join("repo").join(DEFAULT_GENERATED_REPOSITORY_ALIAS)
        ));
    }

    #[test]
    fn generated_repository_target_rejects_missing_custom_alias() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        fs::create_dir_all(&repo_root).unwrap();
        fs::write(
            repo_root.join(REPOSITORY_ROOT_METADATA_FILE),
            r#"{
  "version": 1,
  "generated_repository": "generated",
  "priority": {}
}
"#,
        )
        .unwrap();

        assert!(matches!(
            generated_repository_target(temp.path()),
            Err(RepositoryLoadError::MissingGeneratedRepository { ref alias, .. })
                if alias.as_str() == "generated"
        ));
    }

    #[test]
    fn generated_repository_target_accepts_existing_custom_alias() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        fs::create_dir_all(repo_root.join("generated")).unwrap();
        fs::write(
            repo_root.join(REPOSITORY_ROOT_METADATA_FILE),
            r#"{
  "version": 1,
  "generated_repository": "generated"
}
"#,
        )
        .unwrap();

        let target = generated_repository_target(temp.path()).unwrap();

        assert!(matches!(
            target,
            GeneratedRepositoryTarget::Existing { ref alias, ref path }
                if alias.as_str() == "generated" && path == &repo_root.join("generated")
        ));
    }

    #[test]
    fn derives_package_id_from_lua_path() {
        let root = PathBuf::from("repo/packages");
        let id =
            package_id_from_path(&root, "repo/packages/android/org.fdroid.fdroid.lua").unwrap();
        assert_eq!(id.to_string(), "android/org.fdroid.fdroid");
    }

    #[test]
    fn loads_repository_layout_with_required_directories() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(
            root.join("repo.toml"),
            r#"id = "official"
name = "UpgradeAll Official"
priority = 0
api_version = "getter.repo.v1"
"#,
        )
        .unwrap();
        fs::create_dir(root.join("packages")).unwrap();
        fs::create_dir(root.join("packages/android")).unwrap();
        fs::create_dir(root.join("lib")).unwrap();
        fs::create_dir(root.join("templates")).unwrap();
        let mut file =
            fs::File::create(root.join("packages/android/org.fdroid.fdroid.lua")).unwrap();
        writeln!(file, "return {{ id = 'android/org.fdroid.fdroid' }}").unwrap();

        let layout = RepositoryLayout::load(root).unwrap();
        assert_eq!(layout.metadata.id.as_str(), "official");
        assert_eq!(layout.metadata.priority, RepositoryPriority::DEFAULT);
        assert_eq!(layout.packages.len(), 1);
        assert_eq!(
            layout.packages[0].id.to_string(),
            "android/org.fdroid.fdroid"
        );
    }

    #[test]
    fn package_cache_key_changes_when_package_file_content_changes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(
            root.join("repo.toml"),
            r#"id = "official"
name = "UpgradeAll Official"
priority = 0
api_version = "getter.repo.v1"
"#,
        )
        .unwrap();
        fs::create_dir(root.join("packages")).unwrap();
        fs::create_dir(root.join("packages/android")).unwrap();
        fs::create_dir(root.join("lib")).unwrap();
        fs::create_dir(root.join("templates")).unwrap();
        let package_path = root.join("packages/android/org.fdroid.fdroid.lua");
        fs::write(
            &package_path,
            r#"return { id = "android/org.fdroid.fdroid", name = "F-Droid" }"#,
        )
        .unwrap();
        let layout = RepositoryLayout::load(root).unwrap();
        let first = package_cache_key(&layout, &layout.packages[0]).unwrap();

        fs::write(
            &package_path,
            r#"return { id = "android/org.fdroid.fdroid", name = "F-Droid Nightly" }"#,
        )
        .unwrap();
        let layout = RepositoryLayout::load(root).unwrap();
        let second = package_cache_key(&layout, &layout.packages[0]).unwrap();

        assert_eq!(first.repository_id.as_str(), "official");
        assert_eq!(first.package_id.to_string(), "android/org.fdroid.fdroid");
        assert_eq!(first.api_version, REPO_API_VERSION_V1);
        assert_ne!(first.package_file_hash, second.package_file_hash);
    }

    #[test]
    fn highest_priority_selects_larger_number() {
        let priorities = [
            RepositoryPriority::GENERATED_FALLBACK,
            RepositoryPriority::DEFAULT,
            RepositoryPriority::LOCAL,
        ];
        let selected = highest_priority(&priorities, |priority| *priority).unwrap();
        assert_eq!(*selected, RepositoryPriority::LOCAL);
    }
}
