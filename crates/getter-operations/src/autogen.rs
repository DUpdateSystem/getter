//! Getter-owned installed-inventory autogen operations.
//!
//! The CLI and native bridge both call this module so there is one implementation
//! of installed-autogen preview/apply semantics. Platform layers provide installed
//! inventory facts; this module decides generated package ids, repository
//! coverage, file writes, manifest updates, preservation behavior, and tracked
//! state updates.

use getter_core::autogen::{
    content_hash, local_repo_toml, plan_installed_autogen, AutogenManifest, AutogenManifestEntry,
    AutogenPlan, AutogenSkipReason, InstalledInventory, DEFAULT_AUTOGEN_REPOSITORY_ID,
    DEFAULT_AUTOGEN_REPOSITORY_NAME, LOCAL_REPOSITORY_ID, LOCAL_REPOSITORY_NAME,
};
use getter_core::repository::{
    generated_repository_target, GeneratedRepositoryTarget, GetterDataDirLayout, RepositoryLayout,
    RepositoryLoadError, RepositoryMetadata, RepositoryRootConfig, REPO_API_VERSION_V1,
};
use getter_core::{PackageId, RepositoryId, RepositoryPriority};
use getter_storage::{MainDb, StorageError, StoredRepository};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const AUTOGEN_MANIFEST_FILE: &str = "autogen-manifest.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutogenAcceptance {
    AcceptAll,
    Accept(Vec<PackageId>),
}

#[derive(Debug, thiserror::Error)]
pub enum AutogenOperationError {
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("repository error: {0}")]
    Repository(String),
    #[error("configured generated repository '{alias}' does not exist at {path}")]
    MissingGeneratedRepository { alias: RepositoryId, path: PathBuf },
    #[error("autogen error: {0}")]
    Autogen(String),
}

impl From<RepositoryLoadError> for AutogenOperationError {
    fn from(value: RepositoryLoadError) -> Self {
        match value {
            RepositoryLoadError::MissingGeneratedRepository { alias, path } => {
                Self::MissingGeneratedRepository { alias, path }
            }
            other => Self::Repository(other.to_string()),
        }
    }
}

pub type AutogenOperationResult<T> = Result<T, AutogenOperationError>;

pub fn build_installed_autogen_plan(
    data_dir: &Path,
    db: &MainDb,
    inventory: &InstalledInventory,
) -> AutogenOperationResult<AutogenPlan> {
    let (target_alias, _target_path, target_priority) = generated_repository_config(data_dir)?;
    let covered = higher_priority_package_coverage(db, &target_alias, target_priority)?;
    let mut plan = plan_installed_autogen(inventory, &covered)
        .map_err(|source| AutogenOperationError::Autogen(source.to_string()))?;
    plan.repository_id = target_alias.clone();
    plan.repository_name = if target_alias.as_str() == DEFAULT_AUTOGEN_REPOSITORY_ID {
        DEFAULT_AUTOGEN_REPOSITORY_NAME.to_owned()
    } else {
        target_alias.to_string()
    };
    plan.repository_priority = target_priority;
    Ok(plan)
}

pub fn installed_preview_json(
    data_dir: &Path,
    plan: &AutogenPlan,
) -> AutogenOperationResult<Value> {
    let (target_alias, target_path, _target_priority) = generated_repository_config(data_dir)?;
    let candidates: Vec<Value> = plan.candidates.iter().map(autogen_candidate_json).collect();
    let skipped: Vec<Value> = plan.skipped.iter().map(autogen_skip_json).collect();
    Ok(json!({
        "operation": "installed.preview",
        "target_repo_id": target_alias.as_str(),
        "target_repo_path": target_path,
        "summary": {
            "candidate_count": candidates.len(),
            "skipped_count": skipped.len(),
            "write_count": candidates.len(),
            "delete_count": 0,
        },
        "candidates": candidates,
        "skipped": skipped,
        "diagnostics": [],
    }))
}

pub fn cleanup_preview_json(
    data_dir: &Path,
    db: &MainDb,
    inventory: &InstalledInventory,
) -> AutogenOperationResult<Value> {
    let (target_alias, repo_path, _target_priority) = generated_repository_config(data_dir)?;
    let Some(manifest) = read_autogen_manifest(&repo_path)? else {
        return Ok(json!({
            "operation": "cleanup.preview",
            "target_repo_id": target_alias.as_str(),
            "target_repo_path": repo_path,
            "summary": { "candidate_count": 0, "skipped_count": 0, "write_count": 0, "delete_count": 0 },
            "candidates": [],
            "skipped": [],
            "diagnostics": [],
        }));
    };
    let plan = build_installed_autogen_plan(data_dir, db, inventory)?;
    let installed_ids: BTreeSet<String> = plan
        .candidates
        .iter()
        .map(|candidate| candidate.package_id.to_string())
        .chain(plan.skipped.iter().map(|skip| skip.package_id.to_string()))
        .collect();
    let mut candidates = Vec::new();
    for entry in manifest.packages {
        if !installed_ids.contains(&entry.package_id.to_string()) {
            candidates.push(json!({
                "package_id": entry.package_id.to_string(),
                "action": "delete",
                "output_relative_path": entry.relative_path,
                "content_hash": entry.content_hash,
                "reason": "not_in_installed_inventory",
            }));
        }
    }
    candidates.sort_by_key(|candidate| {
        candidate
            .get("package_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    });
    Ok(json!({
        "operation": "cleanup.preview",
        "target_repo_id": target_alias.as_str(),
        "target_repo_path": repo_path,
        "summary": {
            "candidate_count": candidates.len(),
            "skipped_count": 0,
            "write_count": 0,
            "delete_count": candidates.len(),
        },
        "candidates": candidates,
        "skipped": [],
        "diagnostics": [],
    }))
}

pub fn apply_installed_preview(
    data_dir: &Path,
    db: &MainDb,
    preview: &Value,
    acceptance: &AutogenAcceptance,
) -> AutogenOperationResult<Value> {
    let (target_alias, repo_path, target_priority) = generated_repository_config(data_dir)?;
    if preview.get("target_repo_id").and_then(Value::as_str) != Some(target_alias.as_str()) {
        return Err(AutogenOperationError::Autogen(format!(
            "installed preview target_repo_id must be '{}'",
            target_alias.as_str()
        )));
    }
    ensure_generated_repository(&repo_path, db, &target_alias, target_priority)?;
    let accepted = accepted_preview_candidates(preview, acceptance)?;
    let mut manifest =
        read_autogen_manifest(&repo_path)?.unwrap_or_else(|| empty_autogen_manifest(&target_alias));
    let mut applied = Vec::new();
    let mut preserved = Vec::new();

    for candidate in accepted {
        let package_id = preview_package_id(candidate)?;
        let relative_path = preview_relative_path(candidate)?;
        let content = candidate
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AutogenOperationError::Autogen("preview candidate missing content".to_owned())
            })?;
        let expected_hash = candidate
            .get("content_hash")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AutogenOperationError::Autogen("preview candidate missing content_hash".to_owned())
            })?;
        if content_hash(content) != expected_hash {
            return Err(AutogenOperationError::Autogen(format!(
                "preview content hash mismatch for {package_id}"
            )));
        }
        let target = safe_join(&repo_path, &relative_path)?;
        if target.exists() {
            let current = fs::read_to_string(&target).map_err(|source| {
                AutogenOperationError::Autogen(format!(
                    "failed to read existing autogen file '{}': {source}",
                    target.display()
                ))
            })?;
            let current_hash = content_hash(&current);
            let known_hash = manifest
                .package(&package_id)
                .map(|entry| entry.content_hash.as_str());
            if known_hash != Some(current_hash.as_str()) {
                preserved.push(preserve_autogen_file_in_local(
                    data_dir,
                    db,
                    &package_id,
                    &current,
                )?);
            }
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| {
                AutogenOperationError::Autogen(format!(
                    "failed to create autogen package directory '{}': {source}",
                    parent.display()
                ))
            })?;
        }
        fs::write(&target, content).map_err(|source| {
            AutogenOperationError::Autogen(format!(
                "failed to write autogen package '{}': {source}",
                target.display()
            ))
        })?;
        upsert_manifest_entry(
            &mut manifest,
            AutogenManifestEntry {
                package_id: package_id.clone(),
                relative_path: relative_path.clone(),
                content_hash: expected_hash.to_owned(),
            },
        );
        db.upsert_generated_tracked_package_preserving_user_state(&package_id, &target_alias)?;
        applied.push(json!({
            "package_id": package_id.to_string(),
            "output_relative_path": relative_path,
        }));
    }

    write_autogen_manifest(&repo_path, &manifest)?;
    Ok(json!({
        "target_repo_id": target_alias.as_str(),
        "target_repo_path": repo_path,
        "applied_count": applied.len(),
        "applied": applied,
        "preserved_to_local": preserved,
    }))
}

pub fn apply_cleanup_preview(
    data_dir: &Path,
    db: &MainDb,
    preview: &Value,
    acceptance: &AutogenAcceptance,
) -> AutogenOperationResult<Value> {
    let (target_alias, repo_path, _target_priority) = generated_repository_config(data_dir)?;
    if preview.get("target_repo_id").and_then(Value::as_str) != Some(target_alias.as_str()) {
        return Err(AutogenOperationError::Autogen(format!(
            "cleanup preview target_repo_id must be '{}'",
            target_alias.as_str()
        )));
    }
    let accepted = accepted_preview_candidates(preview, acceptance)?;
    let mut manifest =
        read_autogen_manifest(&repo_path)?.unwrap_or_else(|| empty_autogen_manifest(&target_alias));
    let mut deleted = Vec::new();
    let mut preserved = Vec::new();
    for candidate in accepted {
        let package_id = preview_package_id(candidate)?;
        let relative_path = preview_relative_path(candidate)?;
        let expected_hash = candidate
            .get("content_hash")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AutogenOperationError::Autogen(
                    "cleanup preview candidate missing content_hash".to_owned(),
                )
            })?;
        let manifest_entry = manifest.package(&package_id).ok_or_else(|| {
            AutogenOperationError::Autogen(format!(
                "cleanup preview candidate {package_id} is not managed by autogen manifest"
            ))
        })?;
        if manifest_entry.relative_path != relative_path
            || manifest_entry.content_hash != expected_hash
        {
            return Err(AutogenOperationError::Autogen(format!(
                "cleanup preview candidate {package_id} does not match autogen manifest"
            )));
        }
        let target = safe_join(&repo_path, &relative_path)?;
        if target.exists() {
            let current = fs::read_to_string(&target).map_err(|source| {
                AutogenOperationError::Autogen(format!(
                    "failed to read existing autogen file '{}': {source}",
                    target.display()
                ))
            })?;
            if content_hash(&current) != expected_hash {
                preserved.push(preserve_autogen_file_in_local(
                    data_dir,
                    db,
                    &package_id,
                    &current,
                )?);
            }
            fs::remove_file(&target).map_err(|source| {
                AutogenOperationError::Autogen(format!(
                    "failed to delete autogen package '{}': {source}",
                    target.display()
                ))
            })?;
        }
        manifest
            .packages
            .retain(|entry| entry.package_id != package_id);
        db.delete_generated_tracked_package(&package_id, &target_alias)?;
        deleted.push(json!({
            "package_id": package_id.to_string(),
            "output_relative_path": relative_path,
        }));
    }
    write_autogen_manifest(&repo_path, &manifest)?;
    Ok(json!({
        "target_repo_id": target_alias.as_str(),
        "target_repo_path": repo_path,
        "deleted_count": deleted.len(),
        "deleted": deleted,
        "preserved_to_local": preserved,
    }))
}

pub fn unwrap_preview_payload(
    raw: Value,
    expected_operation: &str,
) -> AutogenOperationResult<Value> {
    let payload = if raw.get("ok").is_some() && raw.get("data").is_some() {
        raw.get("data").cloned().unwrap_or(Value::Null)
    } else {
        raw
    };
    if payload.get("operation").and_then(Value::as_str) != Some(expected_operation) {
        return Err(AutogenOperationError::Autogen(format!(
            "autogen preview operation must be '{expected_operation}'"
        )));
    }
    Ok(payload)
}

pub fn default_autogen_repo_path(data_dir: &Path) -> PathBuf {
    GetterDataDirLayout::new(data_dir)
        .repository_path(&RepositoryId::new(DEFAULT_AUTOGEN_REPOSITORY_ID).expect("valid id"))
}

fn generated_repository_config(
    data_dir: &Path,
) -> AutogenOperationResult<(RepositoryId, PathBuf, RepositoryPriority)> {
    let layout = GetterDataDirLayout::new(data_dir);
    let config = RepositoryRootConfig::load(&layout.repository_root)?;
    let target = generated_repository_target(data_dir)?;
    let (alias, path) = match target {
        GeneratedRepositoryTarget::CreateDefault { alias, path }
        | GeneratedRepositoryTarget::Existing { alias, path } => (alias, path),
    };
    let priority = config.priority_for(&alias);
    Ok((alias, path, priority))
}

fn higher_priority_package_coverage(
    db: &MainDb,
    target_alias: &RepositoryId,
    target_priority: RepositoryPriority,
) -> AutogenOperationResult<HashMap<PackageId, RepositoryId>> {
    let mut covered = HashMap::new();
    for repo in db.repositories()? {
        if &repo.id == target_alias {
            continue;
        }
        if repo.priority <= target_priority {
            continue;
        }
        let Some(path) = repo.path.as_ref() else {
            continue;
        };
        let layout = load_repository_layout(Path::new(path))?;
        for package in layout.packages {
            covered.entry(package.id).or_insert_with(|| repo.id.clone());
        }
    }
    Ok(covered)
}

fn load_repository_layout(path: &Path) -> AutogenOperationResult<RepositoryLayout> {
    RepositoryLayout::load(path)
        .map_err(|source| AutogenOperationError::Repository(source.to_string()))
}

fn autogen_candidate_json(candidate: &getter_core::autogen::AutogenCandidate) -> Value {
    json!({
        "package_id": candidate.package_id.to_string(),
        "kind": candidate.package_id.kind().as_str(),
        "display_name": candidate.name,
        "installed_target": candidate.installed,
        "action": "create",
        "output_relative_path": candidate.relative_path,
        "content_hash": candidate.content_hash,
        "content": candidate.content,
    })
}

fn autogen_skip_json(skip: &getter_core::autogen::AutogenSkip) -> Value {
    json!({
        "package_id": skip.package_id.to_string(),
        "reason": match skip.reason {
            AutogenSkipReason::DuplicateInventoryItem => "duplicate_inventory_item",
            AutogenSkipReason::CoveredByHigherPriorityRepository => "covered_by_higher_priority_repo",
        },
        "covering_repo_id": skip.repository_id.as_ref().map(RepositoryId::as_str),
    })
}

fn accepted_preview_candidates<'a>(
    preview: &'a Value,
    acceptance: &AutogenAcceptance,
) -> AutogenOperationResult<Vec<&'a Value>> {
    let candidates = preview
        .get("candidates")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            AutogenOperationError::Autogen("autogen preview missing candidates".to_owned())
        })?;
    match acceptance {
        AutogenAcceptance::AcceptAll => Ok(candidates.iter().collect()),
        AutogenAcceptance::Accept(ids) => {
            let accepted: BTreeSet<String> = ids.iter().map(ToString::to_string).collect();
            Ok(candidates
                .iter()
                .filter(|candidate| {
                    candidate
                        .get("package_id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| accepted.contains(id))
                })
                .collect())
        }
    }
}

fn preview_package_id(candidate: &Value) -> AutogenOperationResult<PackageId> {
    candidate
        .get("package_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AutogenOperationError::Autogen("preview candidate missing package_id".to_owned())
        })?
        .parse()
        .map_err(|source: getter_core::PackageIdError| {
            AutogenOperationError::Autogen(source.to_string())
        })
}

fn preview_relative_path(candidate: &Value) -> AutogenOperationResult<PathBuf> {
    candidate
        .get("output_relative_path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| {
            AutogenOperationError::Autogen(
                "preview candidate missing output_relative_path".to_owned(),
            )
        })
}

fn ensure_generated_repository(
    repo_path: &Path,
    db: &MainDb,
    alias: &RepositoryId,
    priority: RepositoryPriority,
) -> AutogenOperationResult<()> {
    ensure_repository_layout(repo_path, &generated_repo_toml(alias, priority))?;
    db.upsert_repository(
        &RepositoryMetadata {
            id: alias.clone(),
            name: generated_repository_name(alias),
            priority,
            api_version: REPO_API_VERSION_V1.to_owned(),
        },
        Some(repo_path),
        None,
    )?;
    Ok(())
}

fn generated_repo_toml(alias: &RepositoryId, priority: RepositoryPriority) -> String {
    format!(
        "id = \"{}\"\nname = \"{}\"\npriority = {}\napi_version = \"{REPO_API_VERSION_V1}\"\n",
        alias.as_str(),
        generated_repository_name(alias),
        priority.value()
    )
}

fn generated_repository_name(alias: &RepositoryId) -> String {
    if alias.as_str() == DEFAULT_AUTOGEN_REPOSITORY_ID {
        DEFAULT_AUTOGEN_REPOSITORY_NAME.to_owned()
    } else {
        alias.to_string()
    }
}

fn ensure_local_repository(data_dir: &Path, db: &MainDb) -> AutogenOperationResult<PathBuf> {
    let local_id = RepositoryId::new(LOCAL_REPOSITORY_ID).expect("valid id");
    if let Ok(existing) = find_repository(db, &local_id) {
        let repo_path = repo_path(&existing)?;
        ensure_repository_layout(&repo_path, &local_repo_toml())?;
        return Ok(repo_path);
    }

    let repo_path = GetterDataDirLayout::new(data_dir)
        .repository_path(&RepositoryId::new(LOCAL_REPOSITORY_ID).expect("valid id"));
    ensure_repository_layout(&repo_path, &local_repo_toml())?;
    db.upsert_repository(
        &RepositoryMetadata {
            id: local_id,
            name: LOCAL_REPOSITORY_NAME.to_owned(),
            priority: RepositoryPriority::LOCAL,
            api_version: REPO_API_VERSION_V1.to_owned(),
        },
        Some(&repo_path),
        None,
    )?;
    Ok(repo_path)
}

fn find_repository(db: &MainDb, id: &RepositoryId) -> AutogenOperationResult<StoredRepository> {
    db.repositories()?
        .into_iter()
        .find(|repo| &repo.id == id)
        .ok_or_else(|| {
            AutogenOperationError::Repository(format!("repository '{id}' is not registered"))
        })
}

fn repo_path(repo: &StoredRepository) -> AutogenOperationResult<PathBuf> {
    repo.path.as_ref().map(PathBuf::from).ok_or_else(|| {
        AutogenOperationError::Repository(format!("repository '{}' has no path", repo.id))
    })
}

fn ensure_repository_layout(repo_path: &Path, repo_toml: &str) -> AutogenOperationResult<()> {
    fs::create_dir_all(repo_path.join("packages")).map_err(|source| {
        AutogenOperationError::Autogen(format!(
            "failed to create repository packages dir '{}': {source}",
            repo_path.display()
        ))
    })?;
    fs::create_dir_all(repo_path.join("lib")).map_err(|source| {
        AutogenOperationError::Autogen(format!(
            "failed to create repository lib dir '{}': {source}",
            repo_path.display()
        ))
    })?;
    fs::create_dir_all(repo_path.join("templates")).map_err(|source| {
        AutogenOperationError::Autogen(format!(
            "failed to create repository templates dir '{}': {source}",
            repo_path.display()
        ))
    })?;
    let repo_toml_path = repo_path.join("repo.toml");
    if !repo_toml_path.exists() {
        fs::write(&repo_toml_path, repo_toml).map_err(|source| {
            AutogenOperationError::Autogen(format!(
                "failed to write repo.toml '{}': {source}",
                repo_toml_path.display()
            ))
        })?;
    }
    Ok(())
}

fn preserve_autogen_file_in_local(
    data_dir: &Path,
    db: &MainDb,
    package_id: &PackageId,
    content: &str,
) -> AutogenOperationResult<Value> {
    let local_repo = ensure_local_repository(data_dir, db)?;
    let primary_relative = getter_core::autogen::package_relative_path(package_id);
    let primary_target = safe_join(&local_repo, &primary_relative)?;
    let relative_path = if primary_target.exists() {
        let backup = PathBuf::from("autogen-preserved")
            .join(package_id.kind().as_str())
            .join(format!(
                "{}.{}.lua",
                package_id.name(),
                content_hash(content).replace(':', "-")
            ));
        safe_join(&local_repo, &backup)?;
        backup
    } else {
        primary_relative
    };
    let target = safe_join(&local_repo, &relative_path)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|source| {
            AutogenOperationError::Autogen(format!(
                "failed to create local preservation directory '{}': {source}",
                parent.display()
            ))
        })?;
    }
    fs::write(&target, content).map_err(|source| {
        AutogenOperationError::Autogen(format!(
            "failed to preserve modified autogen file '{}': {source}",
            target.display()
        ))
    })?;
    Ok(json!({
        "package_id": package_id.to_string(),
        "repository_id": LOCAL_REPOSITORY_ID,
        "relative_path": relative_path,
    }))
}

fn read_autogen_manifest(repo_path: &Path) -> AutogenOperationResult<Option<AutogenManifest>> {
    let path = repo_path.join(AUTOGEN_MANIFEST_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|source| {
        AutogenOperationError::Autogen(format!(
            "failed to read autogen manifest '{}': {source}",
            path.display()
        ))
    })?;
    serde_json::from_slice(&bytes).map(Some).map_err(|source| {
        AutogenOperationError::Autogen(format!("failed to parse autogen manifest: {source}"))
    })
}

fn write_autogen_manifest(
    repo_path: &Path,
    manifest: &AutogenManifest,
) -> AutogenOperationResult<()> {
    fs::create_dir_all(repo_path).map_err(|source| {
        AutogenOperationError::Autogen(format!(
            "failed to create autogen repository '{}': {source}",
            repo_path.display()
        ))
    })?;
    let path = repo_path.join(AUTOGEN_MANIFEST_FILE);
    let bytes = serde_json::to_vec_pretty(manifest).map_err(|source| {
        AutogenOperationError::Autogen(format!("failed to serialize manifest: {source}"))
    })?;
    fs::write(&path, bytes).map_err(|source| {
        AutogenOperationError::Autogen(format!(
            "failed to write autogen manifest '{}': {source}",
            path.display()
        ))
    })
}

fn empty_autogen_manifest(repository_id: &RepositoryId) -> AutogenManifest {
    AutogenManifest {
        version: getter_core::autogen::AUTOGEN_MANIFEST_VERSION,
        repository_id: repository_id.clone(),
        packages: Vec::new(),
    }
}

fn upsert_manifest_entry(manifest: &mut AutogenManifest, entry: AutogenManifestEntry) {
    manifest
        .packages
        .retain(|existing| existing.package_id != entry.package_id);
    manifest.packages.push(entry);
    manifest
        .packages
        .sort_by_key(|existing| existing.package_id.to_string());
}

fn safe_join(root: &Path, relative: &Path) -> AutogenOperationResult<PathBuf> {
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(AutogenOperationError::Autogen(format!(
            "unsafe relative path '{}'",
            relative.display()
        )));
    }
    Ok(root.join(relative))
}

#[cfg(test)]
mod tests {
    use super::*;
    use getter_storage::MainDb;

    fn test_inventory() -> InstalledInventory {
        InstalledInventory::new(vec![
            getter_core::autogen::InstalledInventoryItem::AndroidPackage {
                package_name: "com.example.autogen".to_owned(),
                label: Some("Example Autogen".to_owned()),
                version_name: None,
                version_code: None,
            },
        ])
    }

    #[test]
    fn preview_uses_default_generated_repository_under_repo_root() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let db = MainDb::open(data_dir.join("main.db")).unwrap();

        let plan = build_installed_autogen_plan(data_dir, &db, &test_inventory()).unwrap();
        let preview = installed_preview_json(data_dir, &plan).unwrap();

        assert_eq!(preview["target_repo_id"], "autogen");
        assert_eq!(
            preview["target_repo_path"].as_str(),
            data_dir.join("repo/autogen").to_str()
        );
        assert!(!data_dir.join("repo/autogen").exists());
    }

    #[test]
    fn apply_creates_default_generated_repository_and_registers_it() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let db = MainDb::open(data_dir.join("main.db")).unwrap();
        let plan = build_installed_autogen_plan(data_dir, &db, &test_inventory()).unwrap();
        let preview = installed_preview_json(data_dir, &plan).unwrap();

        let result =
            apply_installed_preview(data_dir, &db, &preview, &AutogenAcceptance::AcceptAll)
                .unwrap();

        assert_eq!(result["target_repo_id"], "autogen");
        assert!(data_dir.join("repo/autogen/repo.toml").is_file());
        assert!(data_dir
            .join("repo/autogen/packages/android/com.example.autogen.lua")
            .is_file());
        let repos = db.repositories().unwrap();
        assert_eq!(repos[0].id.as_str(), "autogen");
        assert_eq!(repos[0].priority.value(), -1);
    }

    #[test]
    fn custom_generated_repository_must_already_exist() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        fs::create_dir_all(data_dir.join("repo")).unwrap();
        fs::write(
            data_dir.join("repo/metadata.jsonc"),
            r#"{ "version": 1, "generated_repository": "generated" }"#,
        )
        .unwrap();
        let db = MainDb::open(data_dir.join("main.db")).unwrap();

        let error = build_installed_autogen_plan(data_dir, &db, &test_inventory()).unwrap_err();

        assert!(matches!(
            error,
            AutogenOperationError::MissingGeneratedRepository { ref alias, .. }
                if alias.as_str() == "generated"
        ));
    }

    #[test]
    fn custom_generated_repository_uses_configured_priority() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        fs::create_dir_all(data_dir.join("repo/generated")).unwrap();
        fs::write(
            data_dir.join("repo/metadata.jsonc"),
            r#"{
  "version": 1,
  "generated_repository": "generated",
  "priority": { "generated": -5 }
}"#,
        )
        .unwrap();
        let db = MainDb::open(data_dir.join("main.db")).unwrap();

        let plan = build_installed_autogen_plan(data_dir, &db, &test_inventory()).unwrap();
        let preview = installed_preview_json(data_dir, &plan).unwrap();
        let result =
            apply_installed_preview(data_dir, &db, &preview, &AutogenAcceptance::AcceptAll)
                .unwrap();

        assert_eq!(preview["target_repo_id"], "generated");
        assert_eq!(result["target_repo_id"], "generated");
        let repos = db.repositories().unwrap();
        assert_eq!(repos[0].id.as_str(), "generated");
        assert_eq!(repos[0].priority.value(), -5);
    }
}
