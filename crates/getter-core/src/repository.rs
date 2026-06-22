//! Repository layout loading for Lua package repositories.

use crate::{PackageId, PackageIdError, RepositoryId, RepositoryIdError, RepositoryPriority};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const REPO_API_VERSION_V1: &str = "getter.repo.v1";

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

#[derive(Debug, thiserror::Error)]
pub enum RepositoryLoadError {
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
            RepositoryPriority::LOCAL_AUTOGEN,
            RepositoryPriority::DEFAULT,
            RepositoryPriority::LOCAL,
        ];
        let selected = highest_priority(&priorities, |priority| *priority).unwrap();
        assert_eq!(*selected, RepositoryPriority::LOCAL);
    }
}
