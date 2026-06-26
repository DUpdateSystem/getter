//! Repository layout loading for Lua package repositories.

use crate::{PackageId, PackageIdError, RepositoryId, RepositoryIdError, RepositoryPriority};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
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
pub const REPOSITORY_SELF_METADATA_DIR: &str = ".metadata";
pub const REPOSITORY_LUACLASS_DIR: &str = "luaclass";
pub const PACKAGE_METADATA_FILE: &str = "metadata.jsonc";
pub const PACKAGE_MANIFEST_FILE: &str = "Manifest";
pub const LUA_SCRIPT_EXTENSION: &str = "lua";
pub const LUA_API_SHEBANG_V1: &str = "#!/bin/upa-lua v1";
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryPackageDirectoryLayout {
    pub root: PathBuf,
    pub packages: Vec<PackageDirectory>,
    pub invalid_packages: Vec<InvalidPackageDirectory>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageDirectory {
    pub id: PackageId,
    pub path: PathBuf,
    pub metadata_path: PathBuf,
    pub version_scripts: Vec<PackageVersionScript>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageDirectoryMetadata {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub lua: HashMap<String, PackageLuaMetadata>,
    #[serde(flatten)]
    pub package: PackageTypeMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PackageTypeMetadata {
    #[serde(rename = "android:app")]
    AndroidApp { android: AndroidPackageMetadata },
    #[serde(rename = "magisk:module")]
    MagiskModule { magisk: MagiskPackageMetadata },
    #[serde(rename = "generic")]
    Generic { generic: GenericPackageMetadata },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AndroidPackageMetadata {
    pub package_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagiskPackageMetadata {
    pub module_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenericPackageMetadata {
    pub id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageLuaMetadata {
    #[serde(default)]
    pub permission: Vec<PackageLuaPermission>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageLuaPermission {
    AllowFreeNetwork,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageVersionScript {
    pub version: String,
    pub file_name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidPackageDirectory {
    pub id: Option<PackageId>,
    pub path: PathBuf,
    pub metadata_path: PathBuf,
    pub reason: String,
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
    #[error("failed to read package metadata at {path}: {source}")]
    ReadPackageMetadata {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse package metadata at {path}: {source}")]
    ParsePackageMetadata {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid package metadata at {path}: {reason}")]
    InvalidPackageMetadata { path: PathBuf, reason: String },
    #[error("package '{package_id}' has no enabled version scripts")]
    MissingPackageVersionScript { package_id: PackageId },
    #[error("package '{package_id}' has multiple enabled version scripts; explicit version selection is not implemented")]
    AmbiguousPackageVersionScript { package_id: PackageId },
    #[error("Lua version script {path} is missing required shebang '{expected}'")]
    MissingLuaApiShebang {
        path: PathBuf,
        expected: &'static str,
    },
}

impl RepositoryPackageDirectoryLayout {
    pub fn load(root: impl AsRef<Path>) -> Result<Self, RepositoryLoadError> {
        let root = root.as_ref().to_path_buf();
        let mut packages = Vec::new();
        let mut invalid_packages = Vec::new();
        collect_package_directories(&root, &root, &mut packages, &mut invalid_packages)?;
        packages.sort_by_key(|package| package.id.to_string());
        invalid_packages.sort_by_key(|package| package.path.clone());
        Ok(Self {
            root,
            packages,
            invalid_packages,
        })
    }

    pub fn package(&self, id: &PackageId) -> Option<&PackageDirectory> {
        self.packages.iter().find(|package| &package.id == id)
    }

    pub fn package_metadata(
        &self,
        package: &PackageDirectory,
    ) -> Result<PackageDirectoryMetadata, RepositoryLoadError> {
        load_package_metadata(&package.metadata_path)
    }

    pub fn unambiguous_version_script<'a>(
        &self,
        package: &'a PackageDirectory,
    ) -> Result<&'a PackageVersionScript, RepositoryLoadError> {
        match package.version_scripts.as_slice() {
            [script] => Ok(script),
            [] => Err(RepositoryLoadError::MissingPackageVersionScript {
                package_id: package.id.clone(),
            }),
            _ => Err(RepositoryLoadError::AmbiguousPackageVersionScript {
                package_id: package.id.clone(),
            }),
        }
    }
}

impl PackageDirectoryMetadata {
    pub fn display_name_for(&self, package_id: &PackageId) -> String {
        self.display_name
            .clone()
            .unwrap_or_else(|| package_id.to_string())
    }

    pub fn permissions_for(&self, file_name: &str) -> &[PackageLuaPermission] {
        self.lua
            .get(file_name)
            .map(|metadata| metadata.permission.as_slice())
            .unwrap_or(&[])
    }
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
    for entry in read_dir_entries(current)? {
        let path = entry.path();
        let file_type = entry_file_type(&entry, current)?;
        if file_type.is_dir() {
            collect_package_files(packages_root, &path, out)?;
        } else if file_type.is_file()
            && path
                .extension()
                .is_some_and(|ext| ext == LUA_SCRIPT_EXTENSION)
        {
            let id = package_id_from_path(packages_root, &path)?;
            out.push(PackageFile { id, path });
        }
    }
    Ok(())
}

fn collect_package_directories(
    repository_root: &Path,
    current: &Path,
    packages: &mut Vec<PackageDirectory>,
    invalid_packages: &mut Vec<InvalidPackageDirectory>,
) -> Result<(), RepositoryLoadError> {
    if current == repository_root.join(REPOSITORY_SELF_METADATA_DIR)
        || current == repository_root.join(REPOSITORY_LUACLASS_DIR)
    {
        return Ok(());
    }

    let metadata_path = current.join(PACKAGE_METADATA_FILE);
    if metadata_path.is_file() {
        collect_package_boundary(
            repository_root,
            current,
            &metadata_path,
            packages,
            invalid_packages,
        )?;
        return Ok(());
    }

    for entry in read_dir_entries(current)? {
        let file_type = entry_file_type(&entry, current)?;
        if file_type.is_dir() {
            collect_package_directories(
                repository_root,
                &entry.path(),
                packages,
                invalid_packages,
            )?;
        }
    }
    Ok(())
}

fn collect_package_boundary(
    repository_root: &Path,
    package_dir: &Path,
    metadata_path: &Path,
    packages: &mut Vec<PackageDirectory>,
    invalid_packages: &mut Vec<InvalidPackageDirectory>,
) -> Result<(), RepositoryLoadError> {
    let id = package_id_from_package_dir(repository_root, package_dir);
    if let Err(error) = parse_package_metadata(metadata_path) {
        invalid_packages.push(InvalidPackageDirectory {
            id: id.ok(),
            path: package_dir.to_path_buf(),
            metadata_path: metadata_path.to_path_buf(),
            reason: error,
        });
        return Ok(());
    }
    let id = match id {
        Ok(id) => id,
        Err(error) => {
            invalid_packages.push(InvalidPackageDirectory {
                id: None,
                path: package_dir.to_path_buf(),
                metadata_path: metadata_path.to_path_buf(),
                reason: error.to_string(),
            });
            return Ok(());
        }
    };
    packages.push(PackageDirectory {
        id,
        path: package_dir.to_path_buf(),
        metadata_path: metadata_path.to_path_buf(),
        version_scripts: discover_version_scripts(package_dir)?,
    });
    Ok(())
}

fn parse_package_metadata(metadata_path: &Path) -> Result<(), String> {
    load_package_metadata(metadata_path)
        .map(|_| ())
        .map_err(|source| source.to_string())
}

pub fn load_package_metadata(
    metadata_path: impl AsRef<Path>,
) -> Result<PackageDirectoryMetadata, RepositoryLoadError> {
    let metadata_path = metadata_path.as_ref();
    let bytes =
        fs::read(metadata_path).map_err(|source| RepositoryLoadError::ReadPackageMetadata {
            path: metadata_path.to_path_buf(),
            source,
        })?;
    let value: serde_json::Value = serde_json::from_reader(json_comments::StripComments::new(
        bytes.as_slice(),
    ))
    .map_err(|source| RepositoryLoadError::ParsePackageMetadata {
        path: metadata_path.to_path_buf(),
        source,
    })?;
    if !value.is_object() {
        return Err(RepositoryLoadError::InvalidPackageMetadata {
            path: metadata_path.to_path_buf(),
            reason: "package metadata must be a JSON object".to_owned(),
        });
    }
    serde_json::from_value(value).map_err(|source| RepositoryLoadError::ParsePackageMetadata {
        path: metadata_path.to_path_buf(),
        source,
    })
}

fn discover_version_scripts(
    package_dir: &Path,
) -> Result<Vec<PackageVersionScript>, RepositoryLoadError> {
    let mut scripts = Vec::new();
    for entry in read_dir_entries(package_dir)? {
        let path = entry.path();
        let file_type = entry_file_type(&entry, package_dir)?;
        if !file_type.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if file_name.starts_with('.') {
            continue;
        }
        if path
            .extension()
            .is_none_or(|extension| extension != LUA_SCRIPT_EXTENSION)
        {
            continue;
        }
        let Some(version) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        scripts.push(PackageVersionScript {
            version: version.to_owned(),
            file_name: file_name.to_owned(),
            path,
        });
    }
    scripts.sort_by_key(|script| script.file_name.clone());
    Ok(scripts)
}

fn read_dir_entries(path: &Path) -> Result<Vec<fs::DirEntry>, RepositoryLoadError> {
    fs::read_dir(path)
        .map_err(|source| RepositoryLoadError::ReadPackagesDir {
            path: path.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| RepositoryLoadError::ReadPackagesDir {
            path: path.to_path_buf(),
            source,
        })
}

fn entry_file_type(
    entry: &fs::DirEntry,
    parent: &Path,
) -> Result<fs::FileType, RepositoryLoadError> {
    entry
        .file_type()
        .map_err(|source| RepositoryLoadError::ReadPackagesDir {
            path: parent.to_path_buf(),
            source,
        })
}

pub fn package_id_from_package_dir(
    repository_root: impl AsRef<Path>,
    package_dir: impl AsRef<Path>,
) -> Result<PackageId, RepositoryLoadError> {
    let repository_root = repository_root.as_ref();
    let package_dir = package_dir.as_ref();
    let relative = package_dir.strip_prefix(repository_root).map_err(|_| {
        RepositoryLoadError::InvalidPackagePath {
            path: package_dir.to_path_buf(),
            reason: format!("path is not under {}", repository_root.display()),
        }
    })?;
    package_id_from_relative_path(package_dir, relative)
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
    package_id_from_relative_path(path, &relative.with_extension(""))
}

fn package_id_from_relative_path(
    path: &Path,
    relative: &Path,
) -> Result<PackageId, RepositoryLoadError> {
    let mut parts = relative.components();
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

pub fn package_directory_cache_key(
    repository_id: &RepositoryId,
    package: &PackageDirectory,
    script: &PackageVersionScript,
) -> Result<RepositoryPackageCacheKey, RepositoryLoadError> {
    let package_file_hash = format!(
        "metadata={};script={};manifest={}",
        package_file_content_hash(&package.metadata_path)?,
        package_file_content_hash(&script.path)?,
        optional_file_content_hash(&package.path.join(PACKAGE_MANIFEST_FILE))?
            .unwrap_or_else(|| "missing".to_owned())
    );
    Ok(RepositoryPackageCacheKey {
        repository_id: repository_id.clone(),
        package_id: package.id.clone(),
        api_version: REPO_API_VERSION_V1.to_owned(),
        package_file_hash,
    })
}

fn optional_file_content_hash(path: &Path) -> Result<Option<String>, RepositoryLoadError> {
    if path.exists() {
        package_file_content_hash(path).map(Some)
    } else {
        Ok(None)
    }
}

pub fn package_file_content_hash(path: impl AsRef<Path>) -> Result<String, RepositoryLoadError> {
    let path = path.as_ref();
    let bytes = fs::read(path).map_err(|source| RepositoryLoadError::HashPackageFile {
        path: path.to_path_buf(),
        source,
    })?;
    let hash = Sha512::digest(&bytes);
    Ok(format!("sha512:{hash:x}"))
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
    fn derives_package_id_from_package_directory() {
        let root = PathBuf::from("repo/official");
        let id = package_id_from_package_dir(&root, "repo/official/android/f-droid/magisk/hello")
            .unwrap();
        assert_eq!(id.to_string(), "android/f-droid/magisk/hello");
    }

    #[test]
    fn discovers_package_directories_and_direct_child_version_scripts() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let package_dir = root.join("android/app/org.fdroid.fdroid");
        fs::create_dir_all(package_dir.join("nested")).unwrap();
        fs::write(
            package_dir.join(PACKAGE_METADATA_FILE),
            r#"{ "type": "android:app", "android": { "package_name": "org.fdroid.fdroid" } }"#,
        )
        .unwrap();
        fs::write(package_dir.join("1.2.3.lua"), "return {}").unwrap();
        fs::write(package_dir.join("9999.lua"), "return {}").unwrap();
        fs::write(package_dir.join(".disabled.lua"), "return {}").unwrap();
        fs::write(package_dir.join("nested/2.0.lua"), "return {}").unwrap();

        let layout = RepositoryPackageDirectoryLayout::load(root).unwrap();

        assert_eq!(layout.invalid_packages, Vec::new());
        assert_eq!(layout.packages.len(), 1);
        let package = &layout.packages[0];
        assert_eq!(package.id.to_string(), "android/app/org.fdroid.fdroid");
        assert_eq!(package.path, package_dir);
        assert_eq!(
            package.metadata_path,
            package.path.join(PACKAGE_METADATA_FILE)
        );
        assert_eq!(
            package
                .version_scripts
                .iter()
                .map(|script| (script.version.as_str(), script.file_name.as_str()))
                .collect::<Vec<_>>(),
            vec![("1.2.3", "1.2.3.lua"), ("9999", "9999.lua")]
        );
    }

    #[test]
    fn package_metadata_boundary_stops_nested_discovery_even_when_invalid() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let package_dir = root.join("android/app/broken");
        fs::create_dir_all(package_dir.join("nested/android/app/hidden")).unwrap();
        fs::write(package_dir.join(PACKAGE_METADATA_FILE), "{not-json").unwrap();
        fs::write(
            package_dir.join("nested/android/app/hidden/metadata.jsonc"),
            r#"{ "type": "android:app", "android": { "package_name": "hidden" } }"#,
        )
        .unwrap();

        let layout = RepositoryPackageDirectoryLayout::load(root).unwrap();

        assert!(layout.packages.is_empty());
        assert_eq!(layout.invalid_packages.len(), 1);
        assert_eq!(
            layout.invalid_packages[0]
                .id
                .as_ref()
                .map(ToString::to_string),
            Some("android/app/broken".to_owned())
        );
    }

    #[test]
    fn repository_reserved_roots_are_not_package_discovery_roots() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join(".metadata/android/app/hidden")).unwrap();
        fs::write(
            root.join(".metadata/android/app/hidden/metadata.jsonc"),
            r#"{ "type": "android:app", "android": { "package_name": "hidden" } }"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("luaclass/android/app/hidden")).unwrap();
        fs::write(
            root.join("luaclass/android/app/hidden/metadata.jsonc"),
            r#"{ "type": "android:app", "android": { "package_name": "hidden" } }"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("android/app/visible")).unwrap();
        fs::write(
            root.join("android/app/visible/metadata.jsonc"),
            r#"{ "type": "android:app", "android": { "package_name": "visible" } }"#,
        )
        .unwrap();

        let layout = RepositoryPackageDirectoryLayout::load(root).unwrap();

        assert_eq!(layout.packages.len(), 1);
        assert_eq!(layout.packages[0].id.to_string(), "android/app/visible");
    }

    #[test]
    fn autogen_record_alone_does_not_create_package_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("android/app/generated")).unwrap();
        fs::write(root.join("android/app/generated/.autogen.jsonc"), "{}").unwrap();

        let layout = RepositoryPackageDirectoryLayout::load(root).unwrap();

        assert!(layout.packages.is_empty());
        assert!(layout.invalid_packages.is_empty());
    }

    #[test]
    fn package_metadata_must_be_an_object() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("android/app/not-object")).unwrap();
        fs::write(root.join("android/app/not-object/metadata.jsonc"), "[]").unwrap();

        let layout = RepositoryPackageDirectoryLayout::load(root).unwrap();

        assert!(layout.packages.is_empty());
        assert_eq!(layout.invalid_packages.len(), 1);
        assert!(layout.invalid_packages[0]
            .reason
            .contains("package metadata must be a JSON object"));
    }

    #[test]
    fn invalid_package_path_is_reported_as_invalid_package() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("invalidkind/app/example")).unwrap();
        fs::write(
            root.join("invalidkind/app/example/metadata.jsonc"),
            r#"{ "type": "android:app", "android": { "package_name": "example" } }"#,
        )
        .unwrap();

        let layout = RepositoryPackageDirectoryLayout::load(root).unwrap();

        assert!(layout.packages.is_empty());
        assert_eq!(layout.invalid_packages.len(), 1);
        assert!(layout.invalid_packages[0]
            .reason
            .contains("unsupported package kind"));
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
