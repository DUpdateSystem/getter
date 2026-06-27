//! Getter-owned read-model JSON operations for product UI snapshots.
//!
//! Flutter and Android bridge code should request these operations instead of
//! fabricating app/repository state in the UI layer. The operations read the
//! getter storage/repository/Lua model and return DTO-shaped JSON for adapters
//! to parse and render.

#[cfg(feature = "lua")]
use getter_core::lua::evaluate_package_directory_script;
#[cfg(feature = "lua")]
use getter_core::repository::{RepositoryLoadError, RepositoryPackageDirectoryLayout};
#[cfg(feature = "lua")]
use getter_core::{PackageId, RepositoryId};
use getter_storage::{MainDb, StorageError, StoredRepository, StoredTrackedPackage};
#[cfg(feature = "lua")]
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const MAIN_DB_FILE: &str = "main.db";

#[derive(Debug, thiserror::Error)]
pub enum ReadModelOperationError {
    #[error("invalid read-model request: {0}")]
    InvalidRequest(String),
    #[error("storage operation failed: {0}")]
    Storage(#[from] StorageError),
    #[cfg(feature = "lua")]
    #[error("repository operation failed: {0}")]
    Repository(#[from] RepositoryLoadError),
    #[cfg(feature = "lua")]
    #[error("package evaluation failed: {0}")]
    PackageEval(String),
    #[error("read-model response serialization failed: {0}")]
    Serialize(String),
}

impl ReadModelOperationError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "read_model.invalid_request",
            Self::Storage(_) => "storage.error",
            #[cfg(feature = "lua")]
            Self::Repository(_) => "repository.error",
            #[cfg(feature = "lua")]
            Self::PackageEval(_) => "package.eval_error",
            Self::Serialize(_) => "read_model.serialize_error",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "Getter read-model request is invalid",
            Self::Storage(_) => "Getter storage operation failed",
            #[cfg(feature = "lua")]
            Self::Repository(_) => "Getter repository operation failed",
            #[cfg(feature = "lua")]
            Self::PackageEval(_) => "Getter package evaluation failed",
            Self::Serialize(_) => "Getter read-model response serialization failed",
        }
    }

    pub fn detail(&self) -> Option<String> {
        match self {
            Self::InvalidRequest(detail) | Self::Serialize(detail) => Some(detail.clone()),
            Self::Storage(error) => Some(error.to_string()),
            #[cfg(feature = "lua")]
            Self::Repository(error) => Some(error.to_string()),
            #[cfg(feature = "lua")]
            Self::PackageEval(detail) => Some(detail.clone()),
        }
    }
}

pub fn repository_list_json(data_dir: &Path) -> Result<Value, ReadModelOperationError> {
    let db = open_main_db(data_dir)?;
    Ok(json!({
        "repositories": db
            .repositories()?
            .into_iter()
            .map(repository_json)
            .collect::<Vec<_>>(),
    }))
}

pub fn tracked_package_list_json(data_dir: &Path) -> Result<Value, ReadModelOperationError> {
    let db = open_main_db(data_dir)?;
    Ok(json!({
        "packages": db
            .tracked_packages()?
            .into_iter()
            .map(tracked_package_json)
            .collect::<Vec<_>>(),
    }))
}

#[cfg(feature = "lua")]
pub fn package_eval_json(
    data_dir: &Path,
    request_json: &str,
) -> Result<Value, ReadModelOperationError> {
    let request: PackageEvalRequest = parse_request(request_json)?;
    let db = open_main_db(data_dir)?;
    let package = match request.repository_id {
        Some(repository_id) => {
            evaluate_package_from_repo(&db, &repository_id, &request.package_id)?
        }
        None => evaluate_highest_priority_package(&db, &request.package_id)?,
    };
    let package = serde_json::to_value(package)
        .map_err(|source| ReadModelOperationError::Serialize(source.to_string()))?;
    Ok(json!({ "package": package }))
}

#[cfg(feature = "lua")]
#[derive(Debug, Deserialize)]
struct PackageEvalRequest {
    package_id: PackageId,
    #[serde(default)]
    repository_id: Option<RepositoryId>,
}

#[cfg(feature = "lua")]
fn parse_request<T>(request_json: &str) -> Result<T, ReadModelOperationError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str(request_json)
        .map_err(|source| ReadModelOperationError::InvalidRequest(source.to_string()))
}

fn open_main_db(data_dir: &Path) -> Result<MainDb, ReadModelOperationError> {
    Ok(MainDb::open(main_db_path(data_dir))?)
}

#[cfg(feature = "lua")]
fn evaluate_package_from_repo(
    db: &MainDb,
    repo_id: &RepositoryId,
    package_id: &PackageId,
) -> Result<getter_core::ResolvedPackage, ReadModelOperationError> {
    let repo = find_repository(db, repo_id)?;
    evaluate_package_in_repository(&repo, package_id)?.ok_or_else(|| {
        ReadModelOperationError::PackageEval(format!(
            "package '{}' was not found in repository '{}'",
            package_id, repo_id
        ))
    })
}

#[cfg(feature = "lua")]
fn evaluate_highest_priority_package(
    db: &MainDb,
    package_id: &PackageId,
) -> Result<getter_core::ResolvedPackage, ReadModelOperationError> {
    for repo in db.repositories()? {
        if let Some(package) = evaluate_package_in_repository(&repo, package_id)? {
            return Ok(package);
        }
    }
    Err(ReadModelOperationError::PackageEval(format!(
        "package '{package_id}' was not found in any registered repository"
    )))
}

#[cfg(feature = "lua")]
fn evaluate_package_in_repository(
    repo: &StoredRepository,
    package_id: &PackageId,
) -> Result<Option<getter_core::ResolvedPackage>, ReadModelOperationError> {
    let path = repo_path(repo)?;
    let layout = RepositoryPackageDirectoryLayout::load(&path)?;
    let Some(package) = layout.package(package_id) else {
        return Ok(None);
    };
    let metadata = layout.package_metadata(package)?;
    let script = layout.unambiguous_version_script(package)?;
    evaluate_package_directory_script(&repo.id, package, &metadata, script)
        .map(Some)
        .map_err(|source| ReadModelOperationError::PackageEval(source.to_string()))
}

#[cfg(feature = "lua")]
fn find_repository(
    db: &MainDb,
    id: &RepositoryId,
) -> Result<StoredRepository, ReadModelOperationError> {
    db.repositories()?
        .into_iter()
        .find(|repo| &repo.id == id)
        .ok_or_else(|| {
            ReadModelOperationError::PackageEval(format!("repository '{id}' is not registered"))
        })
}

#[cfg(feature = "lua")]
fn repo_path(repo: &StoredRepository) -> Result<PathBuf, ReadModelOperationError> {
    repo.path.as_ref().map(PathBuf::from).ok_or_else(|| {
        ReadModelOperationError::PackageEval(format!("repository '{}' has no path", repo.id))
    })
}

fn repository_json(repo: StoredRepository) -> Value {
    json!({
        "id": repo.id.as_str(),
        "name": repo.name,
        "priority": repo.priority.value(),
        "api_version": repo.api_version,
        "path": repo.path,
        "revision": repo.revision,
    })
}

fn tracked_package_json(package: StoredTrackedPackage) -> Value {
    json!({
        "id": package.package_id.to_string(),
        "enabled": package.enabled,
        "favorite": package.favorite,
        "pin_version": package.pin_version,
        "repository_id": package.repository_id.map(|id| id.to_string()),
        "package_resolution": package.package_resolution.as_str(),
    })
}

fn main_db_path(data_dir: &Path) -> PathBuf {
    data_dir.join(MAIN_DB_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use getter_core::repository::{RepositoryMetadata, REPO_API_VERSION_V1};
    use getter_core::{RepositoryId, RepositoryPriority};
    use getter_storage::{StoredPackageResolution, TrackedPackageUpsert};
    #[cfg(feature = "lua")]
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn repository_and_tracked_package_lists_use_getter_storage_shapes() {
        let temp = tempdir().unwrap();
        let db = MainDb::open(temp.path().join(MAIN_DB_FILE)).unwrap();
        let repo_id: RepositoryId = "official".parse().unwrap();
        db.upsert_repository(
            &RepositoryMetadata {
                id: repo_id.clone(),
                name: "Official".to_owned(),
                priority: RepositoryPriority::new(10),
                api_version: REPO_API_VERSION_V1.to_owned(),
            },
            Some(Path::new("/tmp/official")),
            Some("rev-1"),
        )
        .unwrap();
        db.upsert_tracked_package(&TrackedPackageUpsert {
            package_id: "android/org.fdroid.fdroid".parse().unwrap(),
            enabled: true,
            favorite: true,
            pin_version: Some("1.2.3".to_owned()),
            repository_id: Some(repo_id),
            package_resolution: StoredPackageResolution::OfficialRepositoryPackage,
        })
        .unwrap();

        let repositories = repository_list_json(temp.path()).unwrap();
        assert_eq!(repositories["repositories"][0]["id"], "official");
        assert_eq!(repositories["repositories"][0]["name"], "Official");
        assert_eq!(repositories["repositories"][0]["priority"], 10);
        assert_eq!(repositories["repositories"][0]["revision"], "rev-1");

        let packages = tracked_package_list_json(temp.path()).unwrap();
        assert_eq!(packages["packages"][0]["id"], "android/org.fdroid.fdroid");
        assert_eq!(packages["packages"][0]["favorite"], true);
        assert_eq!(packages["packages"][0]["pin_version"], "1.2.3");
        assert_eq!(packages["packages"][0]["repository_id"], "official");
        assert_eq!(
            packages["packages"][0]["package_resolution"],
            "official_repository_package"
        );
    }

    #[cfg(feature = "lua")]
    #[test]
    fn package_eval_reads_registered_package_directory_repository() {
        let temp = tempdir().unwrap();
        let repo_path = temp.path().join("repo");
        write_package_directory_repo(&repo_path);

        let db = MainDb::open(temp.path().join(MAIN_DB_FILE)).unwrap();
        db.upsert_repository(
            &RepositoryMetadata {
                id: "autogen".parse().unwrap(),
                name: "Autogen".to_owned(),
                priority: RepositoryPriority::new(-1),
                api_version: REPO_API_VERSION_V1.to_owned(),
            },
            Some(&repo_path),
            None,
        )
        .unwrap();

        let result = package_eval_json(
            temp.path(),
            r#"{"package_id":"android/app/com.example.autogen","repository_id":"autogen"}"#,
        )
        .unwrap();

        assert_eq!(result["package"]["id"], "android/app/com.example.autogen");
        assert_eq!(result["package"]["repository"], "autogen");
        assert_eq!(result["package"]["name"], "Example Autogen");
        assert_eq!(
            result["package"]["installed"][0]["package_name"],
            "com.example.autogen"
        );
    }

    #[cfg(feature = "lua")]
    fn write_package_directory_repo(repo_path: &Path) {
        let package_dir = repo_path.join("android/app/com.example.autogen");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{
  "type": "android:app",
  "display_name": "Example Autogen",
  "android": { "package_name": "com.example.autogen" }
}"#,
        )
        .unwrap();
        fs::write(package_dir.join("Manifest"), "").unwrap();
        fs::write(
            package_dir.join("9999.lua"),
            r#"#!/bin/upa-lua v1
return package_version {
  installed = {
    { kind = "android_package", package_name = "com.example.autogen" },
  },
}
"#,
        )
        .unwrap();
    }
}
