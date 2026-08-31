//! Getter-owned installed-inventory autogen operations.
//!
//! The CLI and native bridge both call this module so there is one implementation
//! of installed-autogen preview/apply semantics. Platform layers provide installed
//! inventory facts; this module decides generated package directories,
//! repository coverage, package-local `.autogen.jsonc` ownership, file writes,
//! cleanup, and tracked state updates.

use fs2::FileExt;
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
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::Write;
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
    recover_autogen_transactions(data_dir, db)?;
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
    recover_autogen_transactions(data_dir, db)?;
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

pub(crate) struct AutogenApplySpec<'a> {
    pub(crate) preview: &'a Value,
    pub(crate) acceptance: &'a AutogenAcceptance,
    pub(crate) expected_operation: &'static str,
    pub(crate) expected_generator: &'static str,
}

struct PreparedCandidate {
    package_id: PackageId,
    relative_path: PathBuf,
    target_dir: PathBuf,
    payload: PreviewCandidatePayload,
}

const AUTOGEN_JOURNAL_FILE: &str = "journal.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AutogenTransactionPhase {
    Prepared,
    BackupsDone,
    SwapsDone,
    DbCommitted,
}

#[derive(Debug, Serialize, Deserialize)]
struct AutogenTransactionEntry {
    package_id: String,
    target: PathBuf,
    backup: PathBuf,
    staged: PathBuf,
    existed: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct AutogenTransactionJournal {
    transaction_id: String,
    phase: AutogenTransactionPhase,
    package_ids: Vec<String>,
    entries: Vec<AutogenTransactionEntry>,
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

fn sync_tree(path: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_tree(&entry.path())?;
        } else {
            File::open(entry.path())?.sync_all()?;
        }
    }
    sync_directory(path)
}

#[cfg(test)]
thread_local! {
    static CRASH_AFTER_PHASE: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn simulate_crash_after(phase: AutogenTransactionPhase) -> Result<(), String> {
    let value = match phase {
        AutogenTransactionPhase::BackupsDone => 1,
        AutogenTransactionPhase::SwapsDone => 2,
        AutogenTransactionPhase::DbCommitted => 3,
        AutogenTransactionPhase::Prepared => 0,
    };
    if CRASH_AFTER_PHASE.get() == value {
        return Err(format!("simulated process crash after {phase:?}"));
    }
    Ok(())
}

#[cfg(not(test))]
fn simulate_crash_after(_phase: AutogenTransactionPhase) -> Result<(), String> {
    Ok(())
}

fn write_journal(
    transaction_dir: &Path,
    journal: &AutogenTransactionJournal,
) -> Result<(), String> {
    let temporary = transaction_dir.join("journal.json.tmp");
    let destination = transaction_dir.join(AUTOGEN_JOURNAL_FILE);
    let bytes = serde_json::to_vec_pretty(journal)
        .map_err(|error| format!("failed to serialize autogen transaction journal: {error}"))?;
    let mut file = File::create(&temporary)
        .map_err(|error| format!("failed to create autogen transaction journal: {error}"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("failed to persist autogen transaction journal: {error}"))?;
    fs::rename(&temporary, &destination)
        .map_err(|error| format!("failed to publish autogen transaction journal: {error}"))?;
    sync_directory(transaction_dir)
        .map_err(|error| format!("failed to sync autogen transaction directory: {error}"))
}

fn rollback_journal(journal: &AutogenTransactionJournal) -> Result<(), String> {
    let mut failures = Vec::new();
    for entry in journal.entries.iter().rev() {
        if entry.backup.exists() {
            if entry.target.exists() {
                if let Err(error) = fs::remove_dir_all(&entry.target) {
                    failures.push(format!(
                        "remove replacement '{}': {error}",
                        entry.target.display()
                    ));
                    continue;
                }
            }
            if let Err(error) = fs::rename(&entry.backup, &entry.target) {
                failures.push(format!(
                    "restore backup '{}' to '{}': {error}",
                    entry.backup.display(),
                    entry.target.display()
                ));
            }
        } else if !entry.existed && entry.target.exists() {
            if let Err(error) = fs::remove_dir_all(&entry.target) {
                failures.push(format!(
                    "remove new target '{}': {error}",
                    entry.target.display()
                ));
            }
        } else if entry.existed && !entry.target.exists() {
            failures.push(format!(
                "original target '{}' is missing and backup '{}' is unavailable",
                entry.target.display(),
                entry.backup.display()
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

struct AutogenApplyLock {
    file: File,
}

impl Drop for AutogenApplyLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn lock_autogen_apply(data_dir: &Path) -> AutogenOperationResult<AutogenApplyLock> {
    fs::create_dir_all(data_dir).map_err(|error| {
        AutogenOperationError::Autogen(format!("failed to access Getter data directory: {error}"))
    })?;
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .open(data_dir.join(".autogen-apply.lock"))
        .map_err(|error| {
            AutogenOperationError::Autogen(format!("failed to open autogen lock: {error}"))
        })?;
    file.try_lock_exclusive().map_err(|error| {
        AutogenOperationError::Autogen(format!(
            "another autogen apply or recovery is active: {error}"
        ))
    })?;
    Ok(AutogenApplyLock { file })
}

fn path_is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

fn journal_target_is_contained(repo_path: &Path, target: &Path) -> bool {
    if path_is_symlink(repo_path) {
        return false;
    }
    let Ok(relative) = target.strip_prefix(repo_path) else {
        return false;
    };
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return false;
    }
    let mut current = repo_path.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        if current.exists() && path_is_symlink(&current) {
            return false;
        }
    }
    true
}

fn validate_autogen_journal(
    repo_path: &Path,
    transaction_dir: &Path,
    journal: &AutogenTransactionJournal,
) -> AutogenOperationResult<()> {
    if transaction_dir.file_name().and_then(|name| name.to_str())
        != Some(journal.transaction_id.as_str())
    {
        return Err(AutogenOperationError::Autogen(
            "autogen journal transaction id does not match its directory".into(),
        ));
    }
    let staged_root = transaction_dir.join("staged");
    let backup_root = transaction_dir.join("backups");
    for (index, entry) in journal.entries.iter().enumerate() {
        if !journal_target_is_contained(repo_path, &entry.target)
            || entry.backup != backup_root.join(index.to_string())
            || entry.staged != staged_root.join(index.to_string())
        {
            return Err(AutogenOperationError::Autogen(format!(
                "autogen journal contains unsafe paths for package '{}'",
                entry.package_id
            )));
        }
    }
    Ok(())
}

pub fn recover_autogen_transactions(data_dir: &Path, db: &MainDb) -> AutogenOperationResult<()> {
    let _lock = lock_autogen_apply(data_dir)?;
    recover_autogen_transactions_locked(data_dir, db)
}

fn recover_autogen_transactions_locked(data_dir: &Path, db: &MainDb) -> AutogenOperationResult<()> {
    let (_, repo_path, _) = generated_repository_config(data_dir)?;
    if path_is_symlink(&repo_path) {
        return Err(AutogenOperationError::Autogen(
            "configured generated repository must not be a symbolic link".into(),
        ));
    }
    if !repo_path.is_dir() {
        return Ok(());
    }
    let entries = fs::read_dir(&repo_path).map_err(|error| {
        AutogenOperationError::Autogen(format!("failed to scan autogen transactions: {error}"))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| AutogenOperationError::Autogen(error.to_string()))?;
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(".autogen-transaction-")
            || !entry
                .file_type()
                .map_err(|error| AutogenOperationError::Autogen(error.to_string()))?
                .is_dir()
        {
            continue;
        }
        let transaction_dir = entry.path();
        if path_is_symlink(&transaction_dir) {
            return Err(AutogenOperationError::Autogen(format!(
                "autogen transaction directory must not be a symbolic link: '{}'",
                transaction_dir.display()
            )));
        }
        let journal_path = transaction_dir.join(AUTOGEN_JOURNAL_FILE);
        if !journal_path.is_file() {
            return Err(AutogenOperationError::Autogen(format!(
                "autogen transaction '{}' has no durable journal",
                transaction_dir.display()
            )));
        }
        let journal: AutogenTransactionJournal =
            serde_json::from_slice(&fs::read(&journal_path).map_err(|error| {
                AutogenOperationError::Autogen(format!(
                    "failed to read '{}': {error}",
                    journal_path.display()
                ))
            })?)
            .map_err(|error| {
                AutogenOperationError::Autogen(format!(
                    "invalid autogen journal '{}': {error}",
                    journal_path.display()
                ))
            })?;
        validate_autogen_journal(&repo_path, &transaction_dir, &journal)?;
        let committed = db.has_autogen_transaction_commit(&journal.transaction_id)?;
        if !committed {
            rollback_journal(&journal).map_err(|error| {
                AutogenOperationError::Autogen(format!(
                    "failed to roll back autogen transaction '{}': {error}; transaction retained at '{}'",
                    journal.transaction_id, transaction_dir.display()
                ))
            })?;
        }
        fs::remove_dir_all(&transaction_dir).map_err(|error| {
            AutogenOperationError::Autogen(format!(
                "failed to clean recovered autogen transaction '{}': {error}",
                transaction_dir.display()
            ))
        })?;
        if committed {
            db.delete_autogen_transaction_commit(&journal.transaction_id)?;
        }
    }
    Ok(())
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
    expected_operation: &'static str,
    expected_generator: &'static str,
) -> AutogenOperationResult<Value> {
    apply_preview_batch(
        data_dir,
        db,
        &[AutogenApplySpec {
            preview,
            acceptance,
            expected_operation,
            expected_generator,
        }],
    )
}

pub(crate) fn apply_preview_batch(
    data_dir: &Path,
    db: &MainDb,
    specs: &[AutogenApplySpec<'_>],
) -> AutogenOperationResult<Value> {
    let _lock = lock_autogen_apply(data_dir)?;
    apply_preview_batch_locked(data_dir, db, specs)
}

fn apply_preview_batch_locked(
    data_dir: &Path,
    db: &MainDb,
    specs: &[AutogenApplySpec<'_>],
) -> AutogenOperationResult<Value> {
    if specs.is_empty() {
        return Err(AutogenOperationError::Autogen(
            "autogen apply batch must contain at least one preview".to_owned(),
        ));
    }
    recover_autogen_transactions_locked(data_dir, db)?;
    let (target_alias, repo_path, target_priority) = generated_repository_config(data_dir)?;
    if path_is_symlink(&repo_path) {
        return Err(AutogenOperationError::Autogen(
            "configured generated repository must not be a symbolic link".into(),
        ));
    }
    let mut prepared = Vec::new();
    let mut package_ids = BTreeSet::new();
    let mut target_paths = BTreeSet::new();

    for spec in specs {
        if spec.preview.get("operation").and_then(Value::as_str) != Some(spec.expected_operation) {
            return Err(AutogenOperationError::Autogen(format!(
                "autogen preview operation must be '{}'",
                spec.expected_operation
            )));
        }
        if spec.preview.get("target_repo_id").and_then(Value::as_str) != Some(target_alias.as_str())
        {
            return Err(AutogenOperationError::Autogen(format!(
                "{} target_repo_id must be '{}'",
                spec.expected_operation,
                target_alias.as_str()
            )));
        }
        for candidate in accepted_preview_candidates(spec.preview, spec.acceptance)? {
            let package_id = preview_package_id(candidate)?;
            let relative_path = preview_relative_path(candidate)?;
            let payload = preview_candidate_payload(
                candidate,
                &package_id,
                &relative_path,
                spec.expected_generator,
            )?;
            let target_dir = safe_join(&repo_path, &relative_path)?;
            if !package_ids.insert(package_id.to_string()) {
                return Err(AutogenOperationError::Autogen(format!(
                    "duplicate accepted package id '{package_id}' in autogen apply batch"
                )));
            }
            if !target_paths.insert(target_dir.clone()) {
                return Err(AutogenOperationError::Autogen(format!(
                    "duplicate accepted target path '{}' in autogen apply batch",
                    target_dir.display()
                )));
            }
            if target_dir.exists() {
                read_owned_record(
                    &target_dir,
                    &package_id,
                    &relative_path,
                    spec.expected_generator,
                )?;
            }
            prepared.push(PreparedCandidate {
                package_id,
                relative_path,
                target_dir,
                payload,
            });
        }
    }

    let repo_existed = repo_path.exists();
    fs::create_dir_all(&repo_path).map_err(|source| {
        AutogenOperationError::Autogen(format!(
            "failed to create generated repository '{}': {source}",
            repo_path.display()
        ))
    })?;
    let transaction_dir = repo_path.join(format!(
        ".autogen-transaction-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let staged_root = transaction_dir.join("staged");
    let backup_root = transaction_dir.join("backups");
    fs::create_dir_all(&backup_root).map_err(|source| {
        AutogenOperationError::Autogen(format!(
            "failed to create autogen backup directory: {source}"
        ))
    })?;
    for (index, candidate) in prepared.iter().enumerate() {
        let staged = staged_root.join(index.to_string());
        fs::create_dir_all(&staged).map_err(|source| {
            AutogenOperationError::Autogen(format!(
                "failed to create autogen staging directory '{}': {source}",
                staged.display()
            ))
        })?;
        write_generated_package(
            &staged,
            &candidate.payload.files,
            &candidate.payload.record_content,
        )?;
    }
    sync_tree(&transaction_dir).map_err(|source| {
        AutogenOperationError::Autogen(format!(
            "failed to sync staged autogen transaction: {source}"
        ))
    })?;
    let transaction_id = transaction_dir
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut journal = AutogenTransactionJournal {
        transaction_id: transaction_id.clone(),
        phase: AutogenTransactionPhase::Prepared,
        package_ids: prepared
            .iter()
            .map(|candidate| candidate.package_id.to_string())
            .collect(),
        entries: prepared
            .iter()
            .enumerate()
            .map(|(index, candidate)| AutogenTransactionEntry {
                package_id: candidate.package_id.to_string(),
                target: candidate.target_dir.clone(),
                backup: backup_root.join(index.to_string()),
                staged: staged_root.join(index.to_string()),
                existed: candidate.target_dir.exists(),
            })
            .collect(),
    };
    write_journal(&transaction_dir, &journal).map_err(AutogenOperationError::Autogen)?;
    sync_directory(&repo_path).map_err(|error| {
        AutogenOperationError::Autogen(format!(
            "failed to persist autogen transaction directory entry: {error}"
        ))
    })?;

    let result_package_ids = journal.package_ids.clone();
    let result_packages = prepared
        .iter()
        .map(|candidate| {
            json!({
                "package_id": candidate.package_id.to_string(),
                "output_relative_path": candidate.relative_path,
            })
        })
        .collect::<Vec<_>>();
    let committed_result = || {
        json!({
            "operation": "autogen.apply",
            "target_repo_id": target_alias.as_str(),
            "accepted_package_ids": result_package_ids,
            "applied": result_packages,
            "written": result_package_ids.len(),
            "applied_count": result_package_ids.len(),
        })
    };
    let apply_result = (|| -> AutogenOperationResult<Value> {
        for entry in &journal.entries {
            if let Some(parent) = entry.target.parent() {
                fs::create_dir_all(parent).map_err(|source| {
                    AutogenOperationError::Autogen(format!(
                        "failed to create generated package parent: {source}"
                    ))
                })?;
            }
            if entry.existed {
                fs::rename(&entry.target, &entry.backup).map_err(|source| {
                    AutogenOperationError::Autogen(format!(
                        "failed to retain generated package backup '{}': {source}",
                        entry.target.display()
                    ))
                })?;
                sync_directory(&backup_root).map_err(|source| {
                    AutogenOperationError::Autogen(format!(
                        "failed to sync autogen backup directory: {source}"
                    ))
                })?;
                if let Some(parent) = entry.target.parent() {
                    sync_directory(parent).map_err(|source| {
                        AutogenOperationError::Autogen(format!(
                            "failed to sync generated package parent: {source}"
                        ))
                    })?;
                }
            }
        }
        journal.phase = AutogenTransactionPhase::BackupsDone;
        write_journal(&transaction_dir, &journal).map_err(AutogenOperationError::Autogen)?;
        simulate_crash_after(journal.phase).map_err(AutogenOperationError::Autogen)?;

        for entry in &journal.entries {
            fs::rename(&entry.staged, &entry.target).map_err(|source| {
                AutogenOperationError::Autogen(format!(
                    "failed to atomically replace generated package '{}': {source}",
                    entry.target.display()
                ))
            })?;
            if let Some(parent) = entry.target.parent() {
                sync_directory(parent).map_err(|source| {
                    AutogenOperationError::Autogen(format!(
                        "failed to sync generated package swap: {source}"
                    ))
                })?;
            }
        }
        journal.phase = AutogenTransactionPhase::SwapsDone;
        write_journal(&transaction_dir, &journal).map_err(AutogenOperationError::Autogen)?;
        simulate_crash_after(journal.phase).map_err(AutogenOperationError::Autogen)?;

        let metadata = RepositoryMetadata {
            id: target_alias.clone(),
            name: DEFAULT_AUTOGEN_REPOSITORY_NAME.to_owned(),
            priority: target_priority,
            api_version: REPO_API_VERSION_V1.to_owned(),
        };
        let ids = prepared
            .iter()
            .map(|candidate| candidate.package_id.clone())
            .collect::<Vec<_>>();
        db.apply_generated_repository_batch_with_marker(
            &metadata,
            &repo_path,
            &ids,
            Some(&transaction_id),
        )?;
        journal.phase = AutogenTransactionPhase::DbCommitted;
        write_journal(&transaction_dir, &journal).map_err(AutogenOperationError::Autogen)?;
        simulate_crash_after(journal.phase).map_err(AutogenOperationError::Autogen)?;
        Ok(committed_result())
    })();

    if let Err(error) = apply_result {
        if error.to_string().contains("simulated process crash") {
            return Err(error);
        }
        if db.has_autogen_transaction_commit(&transaction_id)? {
            // The database is the commit point. Leave journal/marker recovery
            // artifacts for the next startup rather than reporting a false failure.
            return Ok(committed_result());
        }
        if let Err(rollback_error) = rollback_journal(&journal) {
            return Err(AutogenOperationError::Autogen(format!(
                "{error}; rollback also failed: {rollback_error}; transaction retained at '{}'",
                transaction_dir.display()
            )));
        }
        fs::remove_dir_all(&transaction_dir).map_err(|cleanup_error| {
            AutogenOperationError::Autogen(format!(
                "{error}; rollback succeeded but cleanup failed: {cleanup_error}"
            ))
        })?;
        if !repo_existed {
            let _ = fs::remove_dir(&repo_path);
        }
        return Err(error);
    }

    if fs::remove_dir_all(&transaction_dir).is_ok() {
        // A stale commit marker is harmless and recoverable; cleanup failure
        // must not turn an already committed apply into a reported failure.
        let _ = db.delete_autogen_transaction_commit(&transaction_id);
    }
    apply_result
}

pub fn apply_cleanup_preview(
    data_dir: &Path,
    db: &MainDb,
    preview: &Value,
    acceptance: &AutogenAcceptance,
) -> AutogenOperationResult<Value> {
    let _lock = lock_autogen_apply(data_dir)?;
    recover_autogen_transactions_locked(data_dir, db)?;
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
            let available: BTreeSet<&str> = candidates
                .iter()
                .filter_map(|candidate| candidate.get("package_id").and_then(Value::as_str))
                .collect();
            if let Some(unknown) = accepted
                .iter()
                .find(|package_id| !available.contains(package_id.as_str()))
            {
                return Err(AutogenOperationError::Autogen(format!(
                    "accepted package id '{unknown}' is not present in the autogen preview"
                )));
            }
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
        assert_eq!(
            result["applied"][0],
            json!({
                "package_id": "android/app/com.example.autogen",
                "output_relative_path": "android/app/com.example.autogen",
            })
        );
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
    fn batch_preflight_rejects_later_invalid_spec_without_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let db = MainDb::open(data_dir.join("main.db")).unwrap();
        let plan = build_installed_autogen_plan(data_dir, &db, &test_inventory()).unwrap();
        let preview = installed_preview_json(data_dir, &plan).unwrap();
        let mut invalid_preview = preview.clone();
        invalid_preview["candidates"][0]["relative_path"] = json!("../escape");
        let acceptance = AutogenAcceptance::AcceptAll;

        let error = apply_preview_batch(
            data_dir,
            &db,
            &[
                AutogenApplySpec {
                    preview: &preview,
                    acceptance: &acceptance,
                    expected_operation: "installed.preview",
                    expected_generator: INSTALLED_AUTOGEN_GENERATOR,
                },
                AutogenApplySpec {
                    preview: &invalid_preview,
                    acceptance: &acceptance,
                    expected_operation: "installed.preview",
                    expected_generator: INSTALLED_AUTOGEN_GENERATOR,
                },
            ],
        )
        .unwrap_err();

        assert!(matches!(error, AutogenOperationError::Autogen(_)));
        assert!(!data_dir.join("repo/autogen").exists());
        assert!(db.repositories().unwrap().is_empty());
        assert!(db.tracked_packages().unwrap().is_empty());
    }

    #[test]
    fn crash_phases_recover_old_or_committed_state() {
        for (crash_phase, expect_new) in [(1, false), (2, false), (3, true)] {
            let temp = tempfile::tempdir().unwrap();
            let data_dir = temp.path();
            let db_path = data_dir.join("main.db");
            let db = MainDb::open(&db_path).unwrap();
            let plan = build_installed_autogen_plan(data_dir, &db, &test_inventory()).unwrap();
            let preview = installed_preview_json(data_dir, &plan).unwrap();
            apply_installed_preview(data_dir, &db, &preview, &AutogenAcceptance::AcceptAll)
                .unwrap();
            let package_dir = data_dir.join("repo/autogen/android/app/com.example.autogen");
            let old_lua = fs::read(package_dir.join("9999.lua")).unwrap();

            let changed_inventory = InstalledInventory::new(vec![
                getter_core::autogen::InstalledInventoryItem::AndroidPackage {
                    package_name: "com.example.autogen".to_owned(),
                    label: Some("Changed label".to_owned()),
                    version_name: Some("999".to_owned()),
                    version_code: Some(999),
                },
            ]);
            let changed_plan =
                build_installed_autogen_plan(data_dir, &db, &changed_inventory).unwrap();
            let changed_preview = installed_preview_json(data_dir, &changed_plan).unwrap();
            CRASH_AFTER_PHASE.set(crash_phase);
            let error = apply_installed_preview(
                data_dir,
                &db,
                &changed_preview,
                &AutogenAcceptance::AcceptAll,
            )
            .unwrap_err();
            CRASH_AFTER_PHASE.set(0);
            assert!(error.to_string().contains("simulated process crash"));
            drop(db);

            let reopened = MainDb::open(&db_path).unwrap();
            recover_autogen_transactions(data_dir, &reopened).unwrap();
            if expect_new {
                let metadata = fs::read_to_string(package_dir.join("metadata.jsonc")).unwrap();
                assert!(metadata.contains("Changed label"));
            } else {
                assert_eq!(fs::read(package_dir.join("9999.lua")).unwrap(), old_lua);
            }
            assert_eq!(reopened.repositories().unwrap().len(), 1);
            assert_eq!(reopened.tracked_packages().unwrap().len(), 1);
            assert!(!fs::read_dir(data_dir.join("repo/autogen"))
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".autogen-transaction-")));
        }
    }

    #[test]
    fn database_failure_after_swaps_restores_previous_package_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let db_path = data_dir.join("main.db");
        let db = MainDb::open(&db_path).unwrap();
        let plan = build_installed_autogen_plan(data_dir, &db, &test_inventory()).unwrap();
        let preview = installed_preview_json(data_dir, &plan).unwrap();
        apply_installed_preview(data_dir, &db, &preview, &AutogenAcceptance::AcceptAll).unwrap();
        let package_dir = data_dir.join("repo/autogen/android/app/com.example.autogen");
        let old_lua = fs::read(package_dir.join("9999.lua")).unwrap();

        let changed_inventory = InstalledInventory::new(vec![
            getter_core::autogen::InstalledInventoryItem::AndroidPackage {
                package_name: "com.example.autogen".to_owned(),
                label: Some("Changed label".to_owned()),
                version_name: Some("999".to_owned()),
                version_code: Some(999),
            },
        ]);
        let changed_plan = build_installed_autogen_plan(data_dir, &db, &changed_inventory).unwrap();
        let changed_preview = installed_preview_json(data_dir, &changed_plan).unwrap();
        let injector = rusqlite::Connection::open(&db_path).unwrap();
        injector
            .execute_batch(
                "CREATE TRIGGER fail_generated_tracking BEFORE INSERT ON tracked_packages BEGIN SELECT RAISE(ABORT, 'injected'); END;",
            )
            .unwrap();

        let error = apply_installed_preview(
            data_dir,
            &db,
            &changed_preview,
            &AutogenAcceptance::AcceptAll,
        )
        .unwrap_err();

        assert!(matches!(error, AutogenOperationError::Storage(_)));
        assert_eq!(fs::read(package_dir.join("9999.lua")).unwrap(), old_lua);
        assert!(!data_dir
            .join("repo/autogen")
            .read_dir()
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".autogen-transaction-")));
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
