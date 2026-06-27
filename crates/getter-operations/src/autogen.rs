//! Getter-owned installed-inventory autogen operations.
//!
//! The CLI and native bridge both call this module so there is one implementation
//! of installed-autogen preview/apply semantics. Platform layers provide installed
//! inventory facts; this module decides generated package directories,
//! repository coverage, package-local `.autogen.jsonc` ownership, file writes,
//! cleanup, and tracked state updates.

use getter_core::autogen::{
    content_hash, content_hash_bytes, plan_installed_autogen, record_file_key,
    render_autogen_record, AutogenCandidate, AutogenPlan, AutogenRecord, AutogenSkipReason,
    GeneratedPackageFile, InstalledInventory, AUTOGEN_RECORD_FILE, AUTOGEN_RECORD_VERSION,
    DEFAULT_AUTOGEN_REPOSITORY_ID, DEFAULT_AUTOGEN_REPOSITORY_NAME, INSTALLED_AUTOGEN_GENERATOR,
};
use getter_core::repository::{
    generated_repository_target, GeneratedRepositoryTarget, GetterDataDirLayout,
    RepositoryLoadError, RepositoryMetadata, RepositoryPackageDirectoryLayout,
    RepositoryRootConfig, REPO_API_VERSION_V1,
};
use getter_core::{PackageId, RepositoryId, RepositoryPriority};
use getter_storage::{MainDb, StorageError};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};

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
    let candidates: Vec<Value> = plan
        .candidates
        .iter()
        .map(autogen_candidate_json)
        .collect::<AutogenOperationResult<_>>()?;
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
    if !repo_path.is_dir() {
        return Ok(cleanup_preview_response(
            target_alias,
            repo_path,
            Vec::new(),
            Vec::new(),
        ));
    }
    let plan = build_installed_autogen_plan(data_dir, db, inventory)?;
    let installed_ids: BTreeSet<String> = plan
        .candidates
        .iter()
        .map(|candidate| candidate.package_id.to_string())
        .chain(plan.skipped.iter().map(|skip| skip.package_id.to_string()))
        .collect();

    let layout = RepositoryPackageDirectoryLayout::load(&repo_path)?;
    let mut candidates = Vec::new();
    let mut diagnostics = Vec::new();
    for package in layout.packages {
        let relative_path = relative_package_path(&repo_path, &package.path)?;
        let ownership = match read_owned_record(
            &package.path,
            &package.id,
            &relative_path,
            INSTALLED_AUTOGEN_GENERATOR,
        ) {
            Ok(ownership) => ownership,
            Err(error) => {
                diagnostics.push(autogen_diagnostic(
                    "autogen.ownership_conflict",
                    format!(
                        "generated package '{}' has invalid ownership: {error}",
                        package.id
                    ),
                    Some(package.id.to_string()),
                ));
                continue;
            }
        };
        if !installed_ids.contains(&package.id.to_string()) {
            candidates.push(json!({
                "package_id": package.id.to_string(),
                "action": "delete",
                "output_relative_path": relative_path,
                "content_hash": ownership.content_hash,
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
    Ok(cleanup_preview_response(
        target_alias,
        repo_path,
        candidates,
        diagnostics,
    ))
}

pub fn apply_installed_preview(
    data_dir: &Path,
    db: &MainDb,
    preview: &Value,
    acceptance: &AutogenAcceptance,
) -> AutogenOperationResult<Value> {
    apply_preview(
        data_dir,
        db,
        preview,
        acceptance,
        "installed.preview",
        INSTALLED_AUTOGEN_GENERATOR,
    )
}

pub(crate) fn apply_preview(
    data_dir: &Path,
    db: &MainDb,
    preview: &Value,
    acceptance: &AutogenAcceptance,
    expected_operation: &str,
    expected_generator: &str,
) -> AutogenOperationResult<Value> {
    if preview.get("operation").and_then(Value::as_str) != Some(expected_operation) {
        return Err(AutogenOperationError::Autogen(format!(
            "autogen preview operation must be '{expected_operation}'"
        )));
    }
    let (target_alias, repo_path, target_priority) = generated_repository_config(data_dir)?;
    if preview.get("target_repo_id").and_then(Value::as_str) != Some(target_alias.as_str()) {
        return Err(AutogenOperationError::Autogen(format!(
            "{expected_operation} target_repo_id must be '{}'",
            target_alias.as_str()
        )));
    }
    ensure_generated_repository(&repo_path, db, &target_alias, target_priority)?;
    let accepted = accepted_preview_candidates(preview, acceptance)?;
    let mut applied = Vec::new();

    for candidate in accepted {
        let package_id = preview_package_id(candidate)?;
        let relative_path = preview_relative_path(candidate)?;
        let payload =
            preview_candidate_payload(candidate, &package_id, &relative_path, expected_generator)?;
        let target_dir = safe_join(&repo_path, &relative_path)?;
        if target_dir.exists() {
            read_owned_record(&target_dir, &package_id, &relative_path, expected_generator)?;
            clear_directory_contents(&target_dir)?;
        } else {
            fs::create_dir_all(&target_dir).map_err(|source| {
                AutogenOperationError::Autogen(format!(
                    "failed to create generated package directory '{}': {source}",
                    target_dir.display()
                ))
            })?;
        }
        write_generated_package(&target_dir, &payload.files, &payload.record_content)?;
        db.upsert_generated_tracked_package_preserving_user_state(&package_id, &target_alias)?;
        applied.push(json!({
            "package_id": package_id.to_string(),
            "output_relative_path": relative_path,
        }));
    }

    Ok(json!({
        "target_repo_id": target_alias.as_str(),
        "target_repo_path": repo_path,
        "applied_count": applied.len(),
        "applied": applied,
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
    let mut deleted = Vec::new();
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
        let target_dir = safe_join(&repo_path, &relative_path)?;
        let ownership = read_owned_record(
            &target_dir,
            &package_id,
            &relative_path,
            INSTALLED_AUTOGEN_GENERATOR,
        )?;
        if ownership.content_hash != expected_hash {
            return Err(AutogenOperationError::Autogen(format!(
                "cleanup preview candidate {package_id} does not match current autogen record"
            )));
        }
        clear_directory_contents(&target_dir)?;
        db.delete_generated_tracked_package(&package_id, &target_alias)?;
        deleted.push(json!({
            "package_id": package_id.to_string(),
            "output_relative_path": relative_path,
        }));
    }
    Ok(json!({
        "target_repo_id": target_alias.as_str(),
        "target_repo_path": repo_path,
        "deleted_count": deleted.len(),
        "deleted": deleted,
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

pub(crate) fn generated_repository_config(
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

pub(crate) fn higher_priority_package_coverage(
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
        for package_id in load_repository_package_ids(Path::new(path))? {
            covered.entry(package_id).or_insert_with(|| repo.id.clone());
        }
    }
    Ok(covered)
}

fn load_repository_package_ids(path: &Path) -> AutogenOperationResult<Vec<PackageId>> {
    let layout = RepositoryPackageDirectoryLayout::load(path)
        .map_err(|source| AutogenOperationError::Repository(source.to_string()))?;
    Ok(layout
        .packages
        .into_iter()
        .map(|package| package.id)
        .collect())
}

fn autogen_candidate_json(candidate: &AutogenCandidate) -> AutogenOperationResult<Value> {
    let record_content = render_autogen_record(&candidate.record)
        .map_err(|source| AutogenOperationError::Autogen(source.to_string()))?;
    let files: Vec<Value> = candidate
        .files
        .iter()
        .map(|file| {
            json!({
                "relative_path": file.relative_path,
                "content_hash": file.content_hash,
                "content": file.content,
            })
        })
        .collect();
    Ok(json!({
        "package_id": candidate.package_id.to_string(),
        "kind": candidate.package_id.kind().as_str(),
        "display_name": candidate.name,
        "installed_target": candidate.installed,
        "action": "create",
        "output_relative_path": candidate.relative_path,
        "content_hash": candidate.content_hash,
        "content": record_content,
        "autogen_record_content": record_content,
        "files": files,
    }))
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

fn cleanup_preview_response(
    target_alias: RepositoryId,
    repo_path: PathBuf,
    candidates: Vec<Value>,
    diagnostics: Vec<Value>,
) -> Value {
    json!({
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
        "diagnostics": diagnostics,
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

struct PreviewCandidatePayload {
    record_content: String,
    files: Vec<GeneratedPackageFile>,
}

fn preview_candidate_payload(
    candidate: &Value,
    package_id: &PackageId,
    relative_path: &Path,
    expected_generator: &str,
) -> AutogenOperationResult<PreviewCandidatePayload> {
    let record_content = candidate
        .get("autogen_record_content")
        .or_else(|| candidate.get("content"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AutogenOperationError::Autogen(
                "preview candidate missing autogen_record_content".to_owned(),
            )
        })?
        .to_owned();
    let expected_hash = candidate
        .get("content_hash")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AutogenOperationError::Autogen("preview candidate missing content_hash".to_owned())
        })?;
    if content_hash(&record_content) != expected_hash {
        return Err(AutogenOperationError::Autogen(format!(
            "preview autogen record hash mismatch for {package_id}"
        )));
    }
    let files = preview_generated_files(candidate)?;
    let record = parse_record_content(&record_content)?;
    validate_record(
        &record,
        package_id,
        relative_path,
        &files,
        expected_generator,
    )?;
    Ok(PreviewCandidatePayload {
        record_content,
        files,
    })
}

fn preview_generated_files(candidate: &Value) -> AutogenOperationResult<Vec<GeneratedPackageFile>> {
    let files = candidate
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            AutogenOperationError::Autogen("preview candidate missing files".to_owned())
        })?;
    files
        .iter()
        .map(|file| {
            let relative_path = file
                .get("relative_path")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .ok_or_else(|| {
                    AutogenOperationError::Autogen(
                        "preview generated file missing relative_path".to_owned(),
                    )
                })?;
            validate_relative_path(&relative_path)?;
            let content = file
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    AutogenOperationError::Autogen(
                        "preview generated file missing content".to_owned(),
                    )
                })?
                .to_owned();
            let expected_hash = file
                .get("content_hash")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    AutogenOperationError::Autogen(
                        "preview generated file missing content_hash".to_owned(),
                    )
                })?;
            let actual_hash = content_hash(&content);
            if actual_hash != expected_hash {
                return Err(AutogenOperationError::Autogen(format!(
                    "preview generated file '{}' hash mismatch",
                    relative_path.display()
                )));
            }
            Ok(GeneratedPackageFile {
                relative_path,
                content,
                content_hash: actual_hash,
            })
        })
        .collect()
}

fn ensure_generated_repository(
    repo_path: &Path,
    db: &MainDb,
    alias: &RepositoryId,
    priority: RepositoryPriority,
) -> AutogenOperationResult<()> {
    fs::create_dir_all(repo_path).map_err(|source| {
        AutogenOperationError::Autogen(format!(
            "failed to create generated repository '{}': {source}",
            repo_path.display()
        ))
    })?;
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

fn generated_repository_name(alias: &RepositoryId) -> String {
    if alias.as_str() == DEFAULT_AUTOGEN_REPOSITORY_ID {
        DEFAULT_AUTOGEN_REPOSITORY_NAME.to_owned()
    } else {
        alias.to_string()
    }
}

struct LoadedAutogenRecord {
    content_hash: String,
}

fn read_owned_record(
    package_dir: &Path,
    package_id: &PackageId,
    relative_path: &Path,
    expected_generator: &str,
) -> AutogenOperationResult<LoadedAutogenRecord> {
    let record_path = package_dir.join(AUTOGEN_RECORD_FILE);
    if !record_path.is_file() {
        return Err(AutogenOperationError::Autogen(format!(
            "generated package '{}' is missing {AUTOGEN_RECORD_FILE}",
            package_dir.display()
        )));
    }
    let bytes = fs::read(&record_path).map_err(|source| {
        AutogenOperationError::Autogen(format!(
            "failed to read autogen record '{}': {source}",
            record_path.display()
        ))
    })?;
    let record: AutogenRecord = serde_json::from_reader(json_comments::StripComments::new(
        bytes.as_slice(),
    ))
    .map_err(|source| {
        AutogenOperationError::Autogen(format!(
            "failed to parse autogen record '{}': {source}",
            record_path.display()
        ))
    })?;
    let files = read_recorded_files(package_dir, &record)?;
    validate_record(
        &record,
        package_id,
        relative_path,
        &files,
        expected_generator,
    )?;
    Ok(LoadedAutogenRecord {
        content_hash: content_hash_bytes(&bytes),
    })
}

fn read_recorded_files(
    package_dir: &Path,
    record: &AutogenRecord,
) -> AutogenOperationResult<Vec<GeneratedPackageFile>> {
    let mut files = Vec::new();
    for (relative, expected_hash) in &record.files {
        let relative_path = PathBuf::from(relative);
        validate_relative_path(&relative_path)?;
        let path = package_dir.join(&relative_path);
        if !path.is_file() {
            return Err(AutogenOperationError::Autogen(format!(
                "generated file '{}' listed in {AUTOGEN_RECORD_FILE} is missing",
                path.display()
            )));
        }
        let bytes = fs::read(&path).map_err(|source| {
            AutogenOperationError::Autogen(format!(
                "failed to read generated file '{}': {source}",
                path.display()
            ))
        })?;
        let actual_hash = content_hash_bytes(&bytes);
        if &actual_hash != expected_hash {
            return Err(AutogenOperationError::Autogen(format!(
                "generated file '{}' hash does not match {AUTOGEN_RECORD_FILE}",
                path.display()
            )));
        }
        let content = String::from_utf8(bytes).map_err(|source| {
            AutogenOperationError::Autogen(format!(
                "generated file '{}' is not UTF-8: {source}",
                path.display()
            ))
        })?;
        files.push(GeneratedPackageFile {
            relative_path,
            content,
            content_hash: actual_hash,
        });
    }
    Ok(files)
}

fn parse_record_content(content: &str) -> AutogenOperationResult<AutogenRecord> {
    serde_json::from_str(content).map_err(|source| {
        AutogenOperationError::Autogen(format!("failed to parse preview autogen record: {source}"))
    })
}

fn validate_record(
    record: &AutogenRecord,
    package_id: &PackageId,
    relative_path: &Path,
    files: &[GeneratedPackageFile],
    expected_generator: &str,
) -> AutogenOperationResult<()> {
    if record.version != AUTOGEN_RECORD_VERSION {
        return Err(AutogenOperationError::Autogen(format!(
            "unsupported autogen record version {}; expected {AUTOGEN_RECORD_VERSION}",
            record.version
        )));
    }
    if record.generator != expected_generator {
        return Err(AutogenOperationError::Autogen(format!(
            "autogen record generator '{}' does not match '{expected_generator}'",
            record.generator
        )));
    }
    if &record.package_id != package_id {
        return Err(AutogenOperationError::Autogen(format!(
            "autogen record package '{}' does not match '{package_id}'",
            record.package_id
        )));
    }
    if record.output_relative_path != relative_path {
        return Err(AutogenOperationError::Autogen(format!(
            "autogen record output path '{}' does not match '{}'",
            record.output_relative_path.display(),
            relative_path.display()
        )));
    }
    let file_hashes: BTreeMap<String, String> = files
        .iter()
        .map(|file| {
            (
                record_file_key(&file.relative_path),
                file.content_hash.clone(),
            )
        })
        .collect();
    if record.files.contains_key(AUTOGEN_RECORD_FILE) {
        return Err(AutogenOperationError::Autogen(format!(
            "autogen record must not list {AUTOGEN_RECORD_FILE} in files"
        )));
    }
    if record.files != file_hashes {
        return Err(AutogenOperationError::Autogen(
            "autogen record files do not match generated files".to_owned(),
        ));
    }
    Ok(())
}

fn write_generated_package(
    package_dir: &Path,
    files: &[GeneratedPackageFile],
    record_content: &str,
) -> AutogenOperationResult<()> {
    for file in files {
        let target = safe_join(package_dir, &file.relative_path)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| {
                AutogenOperationError::Autogen(format!(
                    "failed to create generated package directory '{}': {source}",
                    parent.display()
                ))
            })?;
        }
        fs::write(&target, &file.content).map_err(|source| {
            AutogenOperationError::Autogen(format!(
                "failed to write generated package file '{}': {source}",
                target.display()
            ))
        })?;
    }
    fs::write(package_dir.join(AUTOGEN_RECORD_FILE), record_content).map_err(|source| {
        AutogenOperationError::Autogen(format!(
            "failed to write autogen record '{}': {source}",
            package_dir.join(AUTOGEN_RECORD_FILE).display()
        ))
    })
}

fn clear_directory_contents(path: &Path) -> AutogenOperationResult<()> {
    fs::create_dir_all(path).map_err(|source| {
        AutogenOperationError::Autogen(format!(
            "failed to create generated package directory '{}': {source}",
            path.display()
        ))
    })?;
    for entry in fs::read_dir(path).map_err(|source| {
        AutogenOperationError::Autogen(format!(
            "failed to read generated package directory '{}': {source}",
            path.display()
        ))
    })? {
        let entry = entry.map_err(|source| {
            AutogenOperationError::Autogen(format!(
                "failed to read generated package directory '{}': {source}",
                path.display()
            ))
        })?;
        let child = entry.path();
        let file_type = entry.file_type().map_err(|source| {
            AutogenOperationError::Autogen(format!(
                "failed to inspect generated package entry '{}': {source}",
                child.display()
            ))
        })?;
        if file_type.is_dir() {
            fs::remove_dir_all(&child).map_err(|source| {
                AutogenOperationError::Autogen(format!(
                    "failed to delete generated package directory '{}': {source}",
                    child.display()
                ))
            })?;
        } else {
            fs::remove_file(&child).map_err(|source| {
                AutogenOperationError::Autogen(format!(
                    "failed to delete generated package file '{}': {source}",
                    child.display()
                ))
            })?;
        }
    }
    Ok(())
}

fn relative_package_path(root: &Path, package_dir: &Path) -> AutogenOperationResult<PathBuf> {
    package_dir
        .strip_prefix(root)
        .map(Path::to_path_buf)
        .map_err(|_| {
            AutogenOperationError::Autogen(format!(
                "package directory '{}' is not under generated repository '{}'",
                package_dir.display(),
                root.display()
            ))
        })
}

fn autogen_diagnostic(code: &str, message: String, package_id: Option<String>) -> Value {
    json!({
        "code": code,
        "message": message,
        "detail": package_id,
    })
}

fn safe_join(root: &Path, relative: &Path) -> AutogenOperationResult<PathBuf> {
    validate_relative_path(relative)?;
    Ok(root.join(relative))
}

fn validate_relative_path(relative: &Path) -> AutogenOperationResult<()> {
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
    Ok(())
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
        assert_eq!(
            preview["candidates"][0]["output_relative_path"],
            "android/app/com.example.autogen"
        );
        assert!(preview["candidates"][0]["files"].is_array());
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
        assert!(data_dir.join("repo/autogen").is_dir());
        assert!(data_dir
            .join("repo/autogen/android/app/com.example.autogen/metadata.jsonc")
            .is_file());
        assert!(data_dir
            .join("repo/autogen/android/app/com.example.autogen/9999.lua")
            .is_file());
        assert!(data_dir
            .join("repo/autogen/android/app/com.example.autogen/.autogen.jsonc")
            .is_file());
        assert!(!data_dir.join("repo/autogen/repo.toml").exists());
        assert!(!data_dir.join("repo/autogen/packages").exists());
        let repos = db.repositories().unwrap();
        assert_eq!(repos[0].id.as_str(), "autogen");
        assert_eq!(repos[0].priority.value(), -1);
    }

    #[test]
    fn apply_rejects_existing_package_directory_without_ownership_record() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let db = MainDb::open(data_dir.join("main.db")).unwrap();
        fs::create_dir_all(data_dir.join("repo/autogen/android/app/com.example.autogen")).unwrap();
        fs::write(
            data_dir.join("repo/autogen/android/app/com.example.autogen/metadata.jsonc"),
            r#"{ "type": "android:app" }"#,
        )
        .unwrap();
        let plan = build_installed_autogen_plan(data_dir, &db, &test_inventory()).unwrap();
        let preview = installed_preview_json(data_dir, &plan).unwrap();

        let error = apply_installed_preview(data_dir, &db, &preview, &AutogenAcceptance::AcceptAll)
            .unwrap_err();

        assert!(
            matches!(error, AutogenOperationError::Autogen(detail) if detail.contains("missing .autogen.jsonc"))
        );
    }

    #[test]
    fn apply_rejects_modified_generated_files_instead_of_preserving_to_local() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let db = MainDb::open(data_dir.join("main.db")).unwrap();
        let plan = build_installed_autogen_plan(data_dir, &db, &test_inventory()).unwrap();
        let preview = installed_preview_json(data_dir, &plan).unwrap();
        apply_installed_preview(data_dir, &db, &preview, &AutogenAcceptance::AcceptAll).unwrap();
        fs::write(
            data_dir.join("repo/autogen/android/app/com.example.autogen/9999.lua"),
            "#!/bin/upa-lua v1\n-- user edited\nreturn {}\n",
        )
        .unwrap();

        let error = apply_installed_preview(data_dir, &db, &preview, &AutogenAcceptance::AcceptAll)
            .unwrap_err();

        assert!(
            matches!(error, AutogenOperationError::Autogen(detail) if detail.contains("hash does not match"))
        );
        assert!(!data_dir.join("repo/local").exists());
    }

    #[test]
    fn cleanup_clears_generated_package_directory_but_keeps_directory() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let db = MainDb::open(data_dir.join("main.db")).unwrap();
        let plan = build_installed_autogen_plan(data_dir, &db, &test_inventory()).unwrap();
        let preview = installed_preview_json(data_dir, &plan).unwrap();
        apply_installed_preview(data_dir, &db, &preview, &AutogenAcceptance::AcceptAll).unwrap();
        let empty_inventory = InstalledInventory::new(Vec::new());
        let cleanup = cleanup_preview_json(data_dir, &db, &empty_inventory).unwrap();

        assert_eq!(
            cleanup["candidates"][0]["package_id"],
            "android/app/com.example.autogen"
        );
        apply_cleanup_preview(data_dir, &db, &cleanup, &AutogenAcceptance::AcceptAll).unwrap();

        let package_dir = data_dir.join("repo/autogen/android/app/com.example.autogen");
        assert!(package_dir.is_dir());
        assert_eq!(fs::read_dir(package_dir).unwrap().count(), 0);
    }

    #[test]
    fn cleanup_rejects_stale_preview_when_autogen_record_changes() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let db = MainDb::open(data_dir.join("main.db")).unwrap();
        let plan = build_installed_autogen_plan(data_dir, &db, &test_inventory()).unwrap();
        let preview = installed_preview_json(data_dir, &plan).unwrap();
        apply_installed_preview(data_dir, &db, &preview, &AutogenAcceptance::AcceptAll).unwrap();
        let empty_inventory = InstalledInventory::new(Vec::new());
        let cleanup = cleanup_preview_json(data_dir, &db, &empty_inventory).unwrap();
        fs::write(
            data_dir.join("repo/autogen/android/app/com.example.autogen/.autogen.jsonc"),
            r#"{ "version": 1, "generator": "installed-inventory", "package_id": "android/app/com.example.autogen", "output_relative_path": "android/app/com.example.autogen", "input": { "kind": "installed_android_package", "package_name": "com.example.autogen" }, "files": {} }"#,
        )
        .unwrap();

        let error = apply_cleanup_preview(data_dir, &db, &cleanup, &AutogenAcceptance::AcceptAll)
            .unwrap_err();

        assert!(
            matches!(error, AutogenOperationError::Autogen(detail) if detail.contains("does not match current autogen record") || detail.contains("files do not match"))
        );
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
