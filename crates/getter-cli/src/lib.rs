//! User-facing getter CLI implementation.
//!
//! The CLI is intentionally thin: it owns command parsing and JSON envelopes,
//! while durable state is initialized through `getter-storage` so the command
//! surface exercises the same Rust-owned SQLite direction used by embedders.

use getter_core::autogen::{
    content_hash, local_autogen_repo_toml, local_repo_toml, plan_local_autogen, AutogenManifest,
    AutogenManifestEntry, AutogenPlan, AutogenSkipReason, InstalledInventory,
    LOCAL_AUTOGEN_REPOSITORY_ID, LOCAL_AUTOGEN_REPOSITORY_NAME, LOCAL_REPOSITORY_ID,
    LOCAL_REPOSITORY_NAME,
};
use getter_core::diagnostics::validate_repository_path;
use getter_core::lua::evaluate_package_file;
use getter_core::repository::{RepositoryLayout, RepositoryMetadata, REPO_API_VERSION_V1};
use getter_core::task::{
    DownloadTaskRequest, InstallHandoffStatus, TaskEventPage, DOWNLOAD_REQUEST_FORMAT,
    DOWNLOAD_REQUEST_VERSION,
};
use getter_core::update::{run_offline_update_check, OfflineUpdateCheckFixture};
use getter_core::{PackageId, RepositoryId, RepositoryPriority};
use getter_downloader::{
    cancel_download_task, record_install_result, run_fake_download_task, submit_fake_download_task,
};
use getter_storage::legacy_room::{
    map_legacy_app, read_legacy_room_database, LegacyAppKind, LegacyAppRecord,
    LegacyExtraAppRecord, LegacyPackageResolution, LegacyRoomDbImport, LegacyRoomImportWarning,
    LegacyRoomReadError,
};
use getter_storage::{
    CacheDb, MainDb, MigrationRecordUpsert, StorageError, StoredPackageResolution,
    StoredRepository, StoredTrackedPackage, TrackedPackageUpsert,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};

const MAIN_DB_FILE: &str = "main.db";
const CACHE_DB_FILE: &str = "cache.db";
const MIGRATION_REPORTS_DIR: &str = "migration-reports";
const LEGACY_ROOM_MIGRATION_ID: &str = "legacy-room-v17";
const AUTOGEN_MANIFEST_FILE: &str = "autogen-manifest.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliInvocation {
    pub data_dir: PathBuf,
    pub command: CliCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    Init,
    AppList,
    HubList,
    RepoList,
    RepoAdd {
        id: RepositoryId,
        path: PathBuf,
        priority: Option<RepositoryPriority>,
    },
    RepoEval {
        id: RepositoryId,
    },
    RepoValidate {
        path: PathBuf,
    },
    PackageEval {
        package_id: PackageId,
        repo_id: Option<RepositoryId>,
    },
    StorageValidate,
    UpdateCheck {
        fixture: PathBuf,
    },
    TaskSubmit {
        request: PathBuf,
    },
    TaskRun {
        task_id: String,
    },
    TaskList,
    TaskCancel {
        task_id: String,
    },
    TaskEvents {
        after: u64,
        limit: usize,
    },
    TaskInstallResult {
        handoff_id: String,
        status: InstallHandoffStatus,
    },
    AutogenInstalledPreview {
        inventory: PathBuf,
    },
    AutogenInstalledApply {
        preview: PathBuf,
        acceptance: AutogenAcceptance,
    },
    AutogenCleanupPreview {
        inventory: PathBuf,
    },
    AutogenCleanupApply {
        preview: PathBuf,
        acceptance: AutogenAcceptance,
    },
    LegacyImportRoomBundle {
        bundle: PathBuf,
    },
    LegacyImportRoomDb {
        db: PathBuf,
    },
    LegacyReportList,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutogenAcceptance {
    AcceptAll,
    Accept(Vec<PackageId>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Success = 0,
    GenericFailure = 1,
    Usage = 2,
    Storage = 10,
    Migration = 20,
    Download = 40,
}

impl ExitCode {
    pub const fn code(self) -> i32 {
        self as i32
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CliError {
    #[error("{0}")]
    Usage(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("repository error: {0}")]
    Repository(String),
    #[error("package evaluation error: {0}")]
    PackageEval(String),
    #[error("update check error: {0}")]
    Update(String),
    #[error("download task error: {0}")]
    Download(String),
    #[error("autogen error: {0}")]
    Autogen(String),
    #[error("Legacy Room export bundle is invalid")]
    InvalidLegacyBundle { report_path: PathBuf },
    #[error("Legacy Room bundle import is not implemented yet")]
    UnsupportedLegacyBundle { report_path: PathBuf },
    #[error("Legacy Room database is invalid")]
    InvalidLegacyDb { report_path: PathBuf },
    #[error("Legacy Room database version is not supported")]
    UnsupportedLegacyDb { report_path: PathBuf },
}

impl CliError {
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Usage(_) => ExitCode::Usage,
            Self::Storage(_) => ExitCode::Storage,
            Self::Repository(_) | Self::PackageEval(_) | Self::Update(_) | Self::Autogen(_) => {
                ExitCode::GenericFailure
            }
            Self::Download(_) => ExitCode::Download,
            Self::InvalidLegacyBundle { .. }
            | Self::UnsupportedLegacyBundle { .. }
            | Self::InvalidLegacyDb { .. }
            | Self::UnsupportedLegacyDb { .. } => ExitCode::Migration,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::Usage(_) => "cli.usage",
            Self::Storage(_) => "storage.error",
            Self::Repository(_) => "repository.error",
            Self::PackageEval(_) => "package.eval_error",
            Self::Update(_) => "update.check_error",
            Self::Download(_) => "download.task_error",
            Self::Autogen(_) => "autogen.error",
            Self::InvalidLegacyBundle { .. } => "migration.invalid_bundle",
            Self::UnsupportedLegacyBundle { .. } => "migration.unsupported_bundle",
            Self::InvalidLegacyDb { .. } => "migration.invalid_db",
            Self::UnsupportedLegacyDb { .. } => "migration.unsupported_db",
        }
    }

    fn message(&self) -> &'static str {
        match self {
            Self::Usage(_) => "Invalid getter CLI usage",
            Self::Storage(_) => "Getter storage operation failed",
            Self::Repository(_) => "Getter repository operation failed",
            Self::PackageEval(_) => "Getter package evaluation failed",
            Self::Update(_) => "Getter update check failed",
            Self::Download(_) => "Getter download task operation failed",
            Self::Autogen(_) => "Getter autogen operation failed",
            Self::InvalidLegacyBundle { .. } => "Legacy Room export bundle is invalid",
            Self::UnsupportedLegacyBundle { .. } => {
                "Legacy Room bundle import is not implemented yet"
            }
            Self::InvalidLegacyDb { .. } => "Legacy Room database is invalid",
            Self::UnsupportedLegacyDb { .. } => "Legacy Room database version is not supported",
        }
    }

    fn detail(&self) -> Option<&str> {
        match self {
            Self::Usage(detail)
            | Self::Storage(detail)
            | Self::Repository(detail)
            | Self::PackageEval(detail)
            | Self::Update(detail)
            | Self::Download(detail)
            | Self::Autogen(detail) => Some(detail.as_str()),
            Self::InvalidLegacyBundle { .. }
            | Self::UnsupportedLegacyBundle { .. }
            | Self::InvalidLegacyDb { .. }
            | Self::UnsupportedLegacyDb { .. } => None,
        }
    }

    fn report_path(&self) -> Option<&Path> {
        match self {
            Self::InvalidLegacyBundle { report_path }
            | Self::UnsupportedLegacyBundle { report_path }
            | Self::InvalidLegacyDb { report_path }
            | Self::UnsupportedLegacyDb { report_path } => Some(report_path.as_path()),
            Self::Usage(_)
            | Self::Storage(_)
            | Self::Repository(_)
            | Self::PackageEval(_)
            | Self::Update(_)
            | Self::Download(_)
            | Self::Autogen(_) => None,
        }
    }
}

impl From<StorageError> for CliError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliOutput {
    pub exit_code: ExitCode,
    pub stdout: String,
    pub stderr: String,
}

/// Parse and execute a getter CLI invocation, returning stdout/stderr and the
/// supported process exit code. This is used by the binary entrypoint and by
/// tests so command behavior stays consistent.
pub fn run<I, S>(args: I) -> CliOutput
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    match run_inner(args) {
        Ok((command_name, data)) => CliOutput {
            exit_code: ExitCode::Success,
            stdout: success_envelope(&command_name, data),
            stderr: String::new(),
        },
        Err((command_name, error)) => {
            let exit_code = error.exit_code();
            let stderr = match error {
                CliError::Usage(_) => usage_text(),
                _ => String::new(),
            };
            CliOutput {
                exit_code,
                stdout: error_envelope(&command_name, &error),
                stderr,
            }
        }
    }
}

fn run_inner<I, S>(args: I) -> Result<(String, Value), (String, CliError)>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let invocation = parse_args(args).map_err(|error| ("unknown".to_owned(), error))?;
    let command_name = invocation.command.name().to_owned();
    execute(invocation)
        .map(|data| (command_name.clone(), data))
        .map_err(|error| (command_name, error))
}

pub fn parse_args<I, S>(args: I) -> Result<CliInvocation, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args: Vec<String> = args.into_iter().map(Into::into).collect();
    if args.first().is_some_and(|arg| arg != "--data-dir") {
        args.remove(0);
    }

    if args.first().map(String::as_str) != Some("--data-dir") {
        return Err(CliError::Usage(
            "missing required --data-dir <path> global option".to_owned(),
        ));
    }
    if args.len() < 3 {
        return Err(CliError::Usage(
            "missing command after --data-dir <path>".to_owned(),
        ));
    }

    let data_dir = PathBuf::from(args[1].clone());
    let command_args = &args[2..];
    let command = match command_args {
        [command] if command == "init" => CliCommand::Init,
        [domain, command] if domain == "app" && command == "list" => CliCommand::AppList,
        [domain, command] if domain == "hub" && command == "list" => CliCommand::HubList,
        [domain, command] if domain == "repo" && command == "list" => CliCommand::RepoList,
        [domain, command, id, path] if domain == "repo" && command == "add" => {
            CliCommand::RepoAdd {
                id: parse_repository_id(id)?,
                path: PathBuf::from(path),
                priority: None,
            }
        }
        [domain, command, id, path, flag, priority]
            if domain == "repo" && command == "add" && flag == "--priority" =>
        {
            CliCommand::RepoAdd {
                id: parse_repository_id(id)?,
                path: PathBuf::from(path),
                priority: Some(parse_priority(priority)?),
            }
        }
        [domain, command, id] if domain == "repo" && command == "eval" => CliCommand::RepoEval {
            id: parse_repository_id(id)?,
        },
        [domain, command, path] if domain == "repo" && command == "validate" => {
            CliCommand::RepoValidate {
                path: PathBuf::from(path),
            }
        }
        [domain, command, package_id] if domain == "package" && command == "eval" => {
            CliCommand::PackageEval {
                package_id: parse_package_id(package_id)?,
                repo_id: None,
            }
        }
        [domain, command, package_id, flag, repo_id]
            if domain == "package" && command == "eval" && flag == "--repo" =>
        {
            CliCommand::PackageEval {
                package_id: parse_package_id(package_id)?,
                repo_id: Some(parse_repository_id(repo_id)?),
            }
        }
        [domain, command] if domain == "storage" && command == "validate" => {
            CliCommand::StorageValidate
        }
        [domain, command, flag, fixture]
            if domain == "update" && command == "check" && flag == "--fixture" =>
        {
            CliCommand::UpdateCheck {
                fixture: PathBuf::from(fixture),
            }
        }
        [domain, command, flag, request]
            if domain == "task" && command == "submit" && flag == "--request" =>
        {
            CliCommand::TaskSubmit {
                request: PathBuf::from(request),
            }
        }
        [domain, command, task_id] if domain == "task" && command == "run" => CliCommand::TaskRun {
            task_id: task_id.clone(),
        },
        [domain, command] if domain == "task" && command == "list" => CliCommand::TaskList,
        [domain, command, task_id] if domain == "task" && command == "cancel" => {
            CliCommand::TaskCancel {
                task_id: task_id.clone(),
            }
        }
        [domain, command, after_flag, after, limit_flag, limit]
            if domain == "task"
                && command == "events"
                && after_flag == "--after"
                && limit_flag == "--limit" =>
        {
            CliCommand::TaskEvents {
                after: parse_u64(after, "--after")?,
                limit: parse_positive_usize(limit, "--limit")?,
            }
        }
        [domain, command, handoff_id, status_flag, status]
            if domain == "task" && command == "install-result" && status_flag == "--status" =>
        {
            CliCommand::TaskInstallResult {
                handoff_id: handoff_id.clone(),
                status: parse_install_handoff_status(status)?,
            }
        }
        [domain, subject, action, flag, inventory]
            if domain == "autogen"
                && subject == "installed"
                && action == "preview"
                && flag == "--inventory" =>
        {
            CliCommand::AutogenInstalledPreview {
                inventory: PathBuf::from(inventory),
            }
        }
        [domain, subject, action, flag, preview, rest @ ..]
            if domain == "autogen"
                && subject == "installed"
                && action == "apply"
                && flag == "--preview" =>
        {
            CliCommand::AutogenInstalledApply {
                preview: PathBuf::from(preview),
                acceptance: parse_autogen_acceptance(rest)?,
            }
        }
        [domain, subject, action, flag, inventory]
            if domain == "autogen"
                && subject == "cleanup"
                && action == "preview"
                && flag == "--inventory" =>
        {
            CliCommand::AutogenCleanupPreview {
                inventory: PathBuf::from(inventory),
            }
        }
        [domain, subject, action, flag, preview, rest @ ..]
            if domain == "autogen"
                && subject == "cleanup"
                && action == "apply"
                && flag == "--preview" =>
        {
            CliCommand::AutogenCleanupApply {
                preview: PathBuf::from(preview),
                acceptance: parse_autogen_acceptance(rest)?,
            }
        }
        [domain, command, bundle] if domain == "legacy" && command == "import-room-bundle" => {
            CliCommand::LegacyImportRoomBundle {
                bundle: PathBuf::from(bundle),
            }
        }
        [domain, command, db] if domain == "legacy" && command == "import-room-db" => {
            CliCommand::LegacyImportRoomDb {
                db: PathBuf::from(db),
            }
        }
        [domain, command] if domain == "legacy" && command == "report-list" => {
            CliCommand::LegacyReportList
        }
        _ => {
            return Err(CliError::Usage(format!(
                "unsupported command: {}",
                command_args.join(" ")
            )))
        }
    };

    Ok(CliInvocation { data_dir, command })
}

fn execute(invocation: CliInvocation) -> Result<Value, CliError> {
    match invocation.command {
        CliCommand::Init => {
            initialize_storage(&invocation.data_dir)?;
            Ok(json!({
                "data_dir": invocation.data_dir,
                "main_db": main_db_path(&invocation.data_dir),
                "cache_db": cache_db_path(&invocation.data_dir),
            }))
        }
        CliCommand::AppList => {
            let db = open_main_db(&invocation.data_dir)?;
            Ok(json!({ "apps": tracked_packages_json(db.tracked_packages()?) }))
        }
        CliCommand::HubList => {
            open_initialized_storage(&invocation.data_dir)?;
            Ok(json!({ "hubs": [] }))
        }
        CliCommand::RepoList => {
            let db = open_main_db(&invocation.data_dir)?;
            Ok(json!({ "repositories": list_repositories(&db)? }))
        }
        CliCommand::RepoAdd { id, path, priority } => {
            let db = open_main_db(&invocation.data_dir)?;
            let layout = load_repository_layout(&path)?;
            if layout.metadata.id != id {
                return Err(CliError::Repository(format!(
                    "repo.toml id '{}' does not match requested id '{}'",
                    layout.metadata.id, id
                )));
            }
            let metadata = RepositoryMetadata {
                priority: priority.unwrap_or(layout.metadata.priority),
                ..layout.metadata.clone()
            };
            db.upsert_repository(&metadata, Some(&path), None)?;
            Ok(json!({ "repository": repository_metadata_json(&metadata, Some(&path), None) }))
        }
        CliCommand::RepoEval { id } => {
            let db = open_main_db(&invocation.data_dir)?;
            let repo = find_repository(&db, &id)?;
            let path = repo_path(&repo)?;
            let layout = load_repository_layout(&path)?;
            let mut packages = Vec::new();
            for package_file in &layout.packages {
                let package = evaluate_package_file(&layout, &package_file.path)
                    .map_err(|error| CliError::PackageEval(error.to_string()))?;
                packages.push(package_json(package)?);
            }
            Ok(json!({
                "repository": repository_json(repo),
                "packages": packages,
            }))
        }
        CliCommand::RepoValidate { path } => {
            open_initialized_storage(&invocation.data_dir)?;
            serde_json::to_value(validate_repository_path(path)).map_err(|source| {
                CliError::Repository(format!("failed to serialize validation report: {source}"))
            })
        }
        CliCommand::PackageEval {
            package_id,
            repo_id,
        } => {
            let db = open_main_db(&invocation.data_dir)?;
            let package = match repo_id {
                Some(repo_id) => evaluate_package_from_repo(&db, &repo_id, &package_id)?,
                None => evaluate_highest_priority_package(&db, &package_id)?,
            };
            Ok(json!({ "package": package_json(package)? }))
        }
        CliCommand::StorageValidate => {
            initialize_storage(&invocation.data_dir)?;
            Ok(json!({
                "valid": true,
                "main_db": main_db_path(&invocation.data_dir),
                "cache_db": cache_db_path(&invocation.data_dir),
            }))
        }
        CliCommand::UpdateCheck { fixture } => {
            open_initialized_storage(&invocation.data_dir)?;
            let fixture = read_update_check_fixture(&fixture)?;
            serde_json::to_value(run_offline_update_check(fixture).map_err(|source| {
                CliError::Update(format!("offline update check failed: {source}"))
            })?)
            .map_err(|source| {
                CliError::Update(format!("failed to serialize update check: {source}"))
            })
        }
        CliCommand::TaskSubmit { request } => {
            let db = open_main_db(&invocation.data_dir)?;
            let request = read_download_task_request(&request)?;
            serde_json::to_value(submit_fake_download_task(&db, request).map_err(|source| {
                CliError::Download(format!("offline task submit failed: {source}"))
            })?)
            .map_err(|source| CliError::Download(format!("failed to serialize task: {source}")))
        }
        CliCommand::TaskRun { task_id } => {
            let db = open_main_db(&invocation.data_dir)?;
            serde_json::to_value(run_fake_download_task(&db, &task_id).map_err(|source| {
                CliError::Download(format!("offline task run failed: {source}"))
            })?)
            .map_err(|source| CliError::Download(format!("failed to serialize task: {source}")))
        }
        CliCommand::TaskList => {
            let db = open_main_db(&invocation.data_dir)?;
            Ok(json!({ "tasks": db.download_tasks()? }))
        }
        CliCommand::TaskCancel { task_id } => {
            let db = open_main_db(&invocation.data_dir)?;
            serde_json::to_value(cancel_download_task(&db, &task_id).map_err(|source| {
                CliError::Download(format!("offline task cancel failed: {source}"))
            })?)
            .map_err(|source| CliError::Download(format!("failed to serialize task: {source}")))
        }
        CliCommand::TaskEvents { after, limit } => {
            let db = open_main_db(&invocation.data_dir)?;
            let events: TaskEventPage = db.task_events_after(after, limit)?;
            serde_json::to_value(events).map_err(|source| {
                CliError::Download(format!("failed to serialize task events: {source}"))
            })
        }
        CliCommand::TaskInstallResult { handoff_id, status } => {
            let db = open_main_db(&invocation.data_dir)?;
            serde_json::to_value(record_install_result(&db, &handoff_id, status).map_err(
                |source| CliError::Download(format!("offline install result failed: {source}")),
            )?)
            .map_err(|source| CliError::Download(format!("failed to serialize handoff: {source}")))
        }
        CliCommand::AutogenInstalledPreview { inventory } => {
            let db = open_main_db(&invocation.data_dir)?;
            let inventory = read_installed_inventory(&inventory)?;
            let plan = build_local_autogen_plan(&db, &inventory)?;
            Ok(autogen_installed_preview_json(&invocation.data_dir, &plan))
        }
        CliCommand::AutogenInstalledApply {
            preview,
            acceptance,
        } => {
            let db = open_main_db(&invocation.data_dir)?;
            let preview = read_autogen_preview(&preview, "installed.preview")?;
            apply_autogen_installed_preview(&invocation.data_dir, &db, &preview, &acceptance)
        }
        CliCommand::AutogenCleanupPreview { inventory } => {
            let db = open_main_db(&invocation.data_dir)?;
            let inventory = read_installed_inventory(&inventory)?;
            cleanup_preview_json(&invocation.data_dir, &db, &inventory)
        }
        CliCommand::AutogenCleanupApply {
            preview,
            acceptance,
        } => {
            let db = open_main_db(&invocation.data_dir)?;
            let preview = read_autogen_preview(&preview, "cleanup.preview")?;
            apply_autogen_cleanup_preview(&invocation.data_dir, &db, &preview, &acceptance)
        }
        CliCommand::LegacyImportRoomBundle { bundle } => {
            let db = open_main_db(&invocation.data_dir)?;
            if db.migration_record_exists(LEGACY_ROOM_MIGRATION_ID)? {
                let records = db.tracked_packages()?;
                return Ok(json!({
                    "already_imported": true,
                    "imported_records": 0,
                    "apps": tracked_packages_json(records),
                }));
            }
            let bytes = fs::read(&bundle).map_err(|source| {
                match create_migration_report(
                    &invocation.data_dir,
                    &bundle,
                    "migration.invalid_bundle",
                    &format!("failed to read bundle: {source}"),
                    0,
                    0,
                    &[],
                ) {
                    Ok(report_path) => CliError::InvalidLegacyBundle { report_path },
                    Err(error) => error,
                }
            })?;
            let parsed: LegacyRoomBundle = serde_json::from_slice(&bytes).map_err(|source| {
                match create_migration_report(
                    &invocation.data_dir,
                    &bundle,
                    "migration.invalid_bundle",
                    &format!("failed to parse JSON bundle: {source}"),
                    0,
                    0,
                    &[],
                ) {
                    Ok(report_path) => CliError::InvalidLegacyBundle { report_path },
                    Err(error) => error,
                }
            })?;
            if parsed.format != "upgradeall-legacy-room-bundle" || parsed.version != 17 {
                let report_path = create_migration_report(
                    &invocation.data_dir,
                    &bundle,
                    "migration.unsupported_bundle",
                    &format!(
                        "unsupported legacy bundle format '{}' version {}",
                        parsed.format, parsed.version
                    ),
                    0,
                    0,
                    &[],
                )?;
                return Err(CliError::UnsupportedLegacyBundle { report_path });
            }
            import_legacy_room_bundle(&db, &parsed)?;
            let report_path = create_migration_report(
                &invocation.data_dir,
                &bundle,
                "migration.imported",
                "Legacy Room bundle imported",
                parsed.apps.len() as u64,
                parsed.apps.len() as u64,
                &[],
            )?;
            let records = db.tracked_packages()?;
            Ok(json!({
                "report_path": report_path,
                "imported_records": parsed.apps.len(),
                "apps": tracked_packages_json(records),
            }))
        }
        CliCommand::LegacyImportRoomDb { db: legacy_db } => {
            let db = open_main_db(&invocation.data_dir)?;
            if db.migration_record_exists(LEGACY_ROOM_MIGRATION_ID)? {
                let records = db.tracked_packages()?;
                return Ok(json!({
                    "already_imported": true,
                    "imported_records": 0,
                    "source_counts": {
                        "app_rows": 0,
                        "extra_app_rows": 0,
                        "hub_rows": 0,
                        "extra_hub_rows": 0,
                    },
                    "warnings": [],
                    "apps": tracked_packages_json(records),
                }));
            }
            let import = read_legacy_room_database(&legacy_db).map_err(|source| {
                legacy_db_import_error(&invocation.data_dir, &legacy_db, source)
            })?;
            let imported_records = import.apps.len();
            let source_counts = source_counts_json(&import);
            let warnings = import_warnings_json(&import.warnings);
            if import.source_counts.app_rows > 0 && imported_records == 0 {
                let report_path = create_migration_report_with_source_counts(
                    &invocation.data_dir,
                    &legacy_db,
                    "migration.invalid_db",
                    "Legacy Room database has app rows but no importable app rows",
                    0,
                    0,
                    &warnings,
                    Some(&source_counts),
                )?;
                return Err(CliError::InvalidLegacyDb { report_path });
            }
            import_legacy_room_db(&db, &import)?;
            let report_path = create_migration_report_with_source_counts(
                &invocation.data_dir,
                &legacy_db,
                "migration.imported",
                "Legacy Room database imported",
                imported_records as u64,
                imported_records as u64,
                &warnings,
                Some(&source_counts),
            )?;
            let records = db.tracked_packages()?;
            Ok(json!({
                "report_path": report_path,
                "imported_records": imported_records,
                "source_counts": source_counts,
                "warnings": warnings,
                "apps": tracked_packages_json(records),
            }))
        }
        CliCommand::LegacyReportList => {
            open_initialized_storage(&invocation.data_dir)?;
            Ok(json!({ "reports": list_migration_reports(&invocation.data_dir)? }))
        }
    }
}

fn initialize_storage(data_dir: &Path) -> Result<(), CliError> {
    fs::create_dir_all(data_dir).map_err(|source| {
        CliError::Storage(format!("failed to create data directory: {source}"))
    })?;
    MainDb::open(main_db_path(data_dir))?;
    CacheDb::open(cache_db_path(data_dir))?;
    Ok(())
}

fn open_initialized_storage(data_dir: &Path) -> Result<(), CliError> {
    initialize_storage(data_dir)
}

fn open_main_db(data_dir: &Path) -> Result<MainDb, CliError> {
    initialize_storage(data_dir)?;
    Ok(MainDb::open(main_db_path(data_dir))?)
}

fn parse_repository_id(value: &str) -> Result<RepositoryId, CliError> {
    RepositoryId::new(value).map_err(|source| CliError::Usage(source.to_string()))
}

fn parse_package_id(value: &str) -> Result<PackageId, CliError> {
    value
        .parse()
        .map_err(|source: getter_core::PackageIdError| CliError::Usage(source.to_string()))
}

fn parse_u64(value: &str, flag: &str) -> Result<u64, CliError> {
    value
        .parse()
        .map_err(|source| CliError::Usage(format!("invalid {flag} value '{value}': {source}")))
}

fn parse_usize(value: &str, flag: &str) -> Result<usize, CliError> {
    value
        .parse()
        .map_err(|source| CliError::Usage(format!("invalid {flag} value '{value}': {source}")))
}

fn parse_positive_usize(value: &str, flag: &str) -> Result<usize, CliError> {
    let parsed = parse_usize(value, flag)?;
    if parsed == 0 {
        return Err(CliError::Usage(format!("{flag} must be greater than zero")));
    }
    Ok(parsed)
}

fn parse_install_handoff_status(value: &str) -> Result<InstallHandoffStatus, CliError> {
    match value {
        "accepted" => Ok(InstallHandoffStatus::Accepted),
        "succeeded" => Ok(InstallHandoffStatus::Succeeded),
        "failed" => Ok(InstallHandoffStatus::Failed),
        "canceled" => Ok(InstallHandoffStatus::Canceled),
        _ => Err(CliError::Usage(format!(
            "invalid install result status '{value}'; expected accepted, succeeded, failed, or canceled"
        ))),
    }
}

fn parse_priority(value: &str) -> Result<RepositoryPriority, CliError> {
    value
        .parse::<i32>()
        .map(RepositoryPriority::new)
        .map_err(|source| CliError::Usage(format!("invalid repository priority: {source}")))
}

fn parse_autogen_acceptance(args: &[String]) -> Result<AutogenAcceptance, CliError> {
    match args {
        [flag] if flag == "--accept-all" => Ok(AutogenAcceptance::AcceptAll),
        rest if !rest.is_empty() => {
            let mut ids = Vec::new();
            let mut index = 0;
            while index < rest.len() {
                if rest[index] != "--accept" {
                    return Err(CliError::Usage(
                        "autogen apply requires --accept-all or repeated --accept <package-id>"
                            .to_owned(),
                    ));
                }
                let package_id = rest
                    .get(index + 1)
                    .ok_or_else(|| CliError::Usage("--accept requires a package id".to_owned()))?;
                ids.push(parse_package_id(package_id)?);
                index += 2;
            }
            Ok(AutogenAcceptance::Accept(ids))
        }
        _ => Err(CliError::Usage(
            "autogen apply requires --accept-all or --accept <package-id>".to_owned(),
        )),
    }
}

fn load_repository_layout(path: &Path) -> Result<RepositoryLayout, CliError> {
    RepositoryLayout::load(path).map_err(|source| CliError::Repository(source.to_string()))
}

fn list_repositories(db: &MainDb) -> Result<Vec<Value>, CliError> {
    Ok(db
        .repositories()?
        .into_iter()
        .map(repository_json)
        .collect())
}

fn find_repository(db: &MainDb, id: &RepositoryId) -> Result<StoredRepository, CliError> {
    db.repositories()?
        .into_iter()
        .find(|repo| &repo.id == id)
        .ok_or_else(|| CliError::Repository(format!("repository '{id}' is not registered")))
}

fn evaluate_package_from_repo(
    db: &MainDb,
    repo_id: &RepositoryId,
    package_id: &PackageId,
) -> Result<getter_core::ResolvedPackage, CliError> {
    let repo = find_repository(db, repo_id)?;
    let path = repo_path(&repo)?;
    let layout = load_repository_layout(&path)?;
    let package_file = layout.package_file(package_id).ok_or_else(|| {
        CliError::PackageEval(format!(
            "package '{}' was not found in repository '{}'",
            package_id, repo_id
        ))
    })?;
    evaluate_package_file(&layout, &package_file.path)
        .map_err(|error| CliError::PackageEval(error.to_string()))
}

fn evaluate_highest_priority_package(
    db: &MainDb,
    package_id: &PackageId,
) -> Result<getter_core::ResolvedPackage, CliError> {
    for repo in db.repositories()? {
        let path = repo_path(&repo)?;
        let layout = load_repository_layout(&path)?;
        if let Some(package_file) = layout.package_file(package_id) {
            return evaluate_package_file(&layout, &package_file.path)
                .map_err(|error| CliError::PackageEval(error.to_string()));
        }
    }
    Err(CliError::PackageEval(format!(
        "package '{package_id}' was not found in any registered repository"
    )))
}

fn read_installed_inventory(path: &Path) -> Result<InstalledInventory, CliError> {
    let bytes = fs::read(path)
        .map_err(|source| CliError::Autogen(format!("failed to read inventory: {source}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|source| CliError::Autogen(format!("failed to parse inventory JSON: {source}")))
}

fn read_update_check_fixture(path: &Path) -> Result<OfflineUpdateCheckFixture, CliError> {
    let bytes = fs::read(path).map_err(|source| {
        CliError::Update(format!("failed to read update check fixture: {source}"))
    })?;
    serde_json::from_slice(&bytes).map_err(|source| {
        CliError::Update(format!(
            "failed to parse update check fixture JSON: {source}"
        ))
    })
}

fn read_download_task_request(path: &Path) -> Result<DownloadTaskRequest, CliError> {
    let bytes = fs::read(path)
        .map_err(|source| CliError::Download(format!("failed to read task request: {source}")))?;
    let request: DownloadTaskRequest = serde_json::from_slice(&bytes).map_err(|source| {
        CliError::Download(format!("failed to parse task request JSON: {source}"))
    })?;
    if request.format != DOWNLOAD_REQUEST_FORMAT {
        return Err(CliError::Download(format!(
            "unsupported download request format '{}'",
            request.format
        )));
    }
    if request.version != DOWNLOAD_REQUEST_VERSION {
        return Err(CliError::Download(format!(
            "unsupported download request version {}; expected {}",
            request.version, DOWNLOAD_REQUEST_VERSION
        )));
    }
    Ok(request)
}

fn build_local_autogen_plan(
    db: &MainDb,
    inventory: &InstalledInventory,
) -> Result<AutogenPlan, CliError> {
    let covered = higher_priority_package_coverage(db)?;
    plan_local_autogen(inventory, &covered).map_err(|source| CliError::Autogen(source.to_string()))
}

fn higher_priority_package_coverage(
    db: &MainDb,
) -> Result<HashMap<PackageId, RepositoryId>, CliError> {
    let mut covered = HashMap::new();
    for repo in db.repositories()? {
        if repo.id.as_str() == LOCAL_AUTOGEN_REPOSITORY_ID {
            continue;
        }
        if repo.priority <= RepositoryPriority::LOCAL_AUTOGEN {
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

fn autogen_installed_preview_json(data_dir: &Path, plan: &AutogenPlan) -> Value {
    let candidates: Vec<Value> = plan.candidates.iter().map(autogen_candidate_json).collect();
    let skipped: Vec<Value> = plan.skipped.iter().map(autogen_skip_json).collect();
    json!({
        "operation": "installed.preview",
        "target_repo_id": plan.repository_id.as_str(),
        "target_repo_path": local_autogen_repo_path(data_dir),
        "summary": {
            "candidate_count": candidates.len(),
            "skipped_count": skipped.len(),
            "write_count": candidates.len(),
            "delete_count": 0,
        },
        "candidates": candidates,
        "skipped": skipped,
        "diagnostics": [],
    })
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

fn read_autogen_preview(path: &Path, expected_operation: &str) -> Result<Value, CliError> {
    let bytes = fs::read(path)
        .map_err(|source| CliError::Autogen(format!("failed to read autogen preview: {source}")))?;
    let raw: Value = serde_json::from_slice(&bytes).map_err(|source| {
        CliError::Autogen(format!("failed to parse autogen preview JSON: {source}"))
    })?;
    let payload = if raw.get("ok").is_some() && raw.get("data").is_some() {
        raw.get("data").cloned().unwrap_or(Value::Null)
    } else {
        raw
    };
    if payload.get("operation").and_then(Value::as_str) != Some(expected_operation) {
        return Err(CliError::Autogen(format!(
            "autogen preview operation must be '{expected_operation}'"
        )));
    }
    Ok(payload)
}

fn apply_autogen_installed_preview(
    data_dir: &Path,
    db: &MainDb,
    preview: &Value,
    acceptance: &AutogenAcceptance,
) -> Result<Value, CliError> {
    let repo_path = local_autogen_repo_path(data_dir);
    ensure_local_autogen_repository(data_dir, db)?;
    let accepted = accepted_preview_candidates(preview, acceptance)?;
    let mut manifest = read_autogen_manifest(&repo_path)?.unwrap_or_else(empty_autogen_manifest);
    let mut applied = Vec::new();
    let mut preserved = Vec::new();

    for candidate in accepted {
        let package_id = preview_package_id(candidate)?;
        let relative_path = preview_relative_path(candidate)?;
        let content = candidate
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| CliError::Autogen("preview candidate missing content".to_owned()))?;
        let expected_hash = candidate
            .get("content_hash")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CliError::Autogen("preview candidate missing content_hash".to_owned())
            })?;
        if content_hash(content) != expected_hash {
            return Err(CliError::Autogen(format!(
                "preview content hash mismatch for {package_id}"
            )));
        }
        let target = safe_join(&repo_path, &relative_path)?;
        if target.exists() {
            let current = fs::read_to_string(&target).map_err(|source| {
                CliError::Autogen(format!(
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
                CliError::Autogen(format!(
                    "failed to create autogen package directory '{}': {source}",
                    parent.display()
                ))
            })?;
        }
        fs::write(&target, content).map_err(|source| {
            CliError::Autogen(format!(
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
        db.upsert_generated_tracked_package_preserving_user_state(
            &package_id,
            &RepositoryId::new(LOCAL_AUTOGEN_REPOSITORY_ID).expect("valid id"),
        )?;
        applied.push(json!({
            "package_id": package_id.to_string(),
            "output_relative_path": relative_path,
        }));
    }

    write_autogen_manifest(&repo_path, &manifest)?;
    Ok(json!({
        "target_repo_id": LOCAL_AUTOGEN_REPOSITORY_ID,
        "target_repo_path": repo_path,
        "applied_count": applied.len(),
        "applied": applied,
        "preserved_to_local": preserved,
    }))
}

fn cleanup_preview_json(
    data_dir: &Path,
    db: &MainDb,
    inventory: &InstalledInventory,
) -> Result<Value, CliError> {
    let repo_path = local_autogen_repo_path(data_dir);
    let Some(manifest) = read_autogen_manifest(&repo_path)? else {
        return Ok(json!({
            "operation": "cleanup.preview",
            "target_repo_id": LOCAL_AUTOGEN_REPOSITORY_ID,
            "target_repo_path": repo_path,
            "summary": { "candidate_count": 0, "skipped_count": 0, "write_count": 0, "delete_count": 0 },
            "candidates": [],
            "skipped": [],
            "diagnostics": [],
        }));
    };
    let plan = build_local_autogen_plan(db, inventory)?;
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
        "target_repo_id": LOCAL_AUTOGEN_REPOSITORY_ID,
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

fn apply_autogen_cleanup_preview(
    data_dir: &Path,
    db: &MainDb,
    preview: &Value,
    acceptance: &AutogenAcceptance,
) -> Result<Value, CliError> {
    if preview.get("target_repo_id").and_then(Value::as_str) != Some(LOCAL_AUTOGEN_REPOSITORY_ID) {
        return Err(CliError::Autogen(format!(
            "cleanup preview target_repo_id must be '{LOCAL_AUTOGEN_REPOSITORY_ID}'"
        )));
    }
    let repo_path = local_autogen_repo_path(data_dir);
    let accepted = accepted_preview_candidates(preview, acceptance)?;
    let mut manifest = read_autogen_manifest(&repo_path)?.unwrap_or_else(empty_autogen_manifest);
    let mut deleted = Vec::new();
    let mut preserved = Vec::new();
    let local_autogen_id = RepositoryId::new(LOCAL_AUTOGEN_REPOSITORY_ID).expect("valid id");
    for candidate in accepted {
        let package_id = preview_package_id(candidate)?;
        let relative_path = preview_relative_path(candidate)?;
        let expected_hash = candidate
            .get("content_hash")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CliError::Autogen("cleanup preview candidate missing content_hash".to_owned())
            })?;
        let manifest_entry = manifest.package(&package_id).ok_or_else(|| {
            CliError::Autogen(format!(
                "cleanup preview candidate {package_id} is not managed by local_autogen manifest"
            ))
        })?;
        if manifest_entry.relative_path != relative_path
            || manifest_entry.content_hash != expected_hash
        {
            return Err(CliError::Autogen(format!(
                "cleanup preview candidate {package_id} does not match local_autogen manifest"
            )));
        }
        let target = safe_join(&repo_path, &relative_path)?;
        if target.exists() {
            let current = fs::read_to_string(&target).map_err(|source| {
                CliError::Autogen(format!(
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
                CliError::Autogen(format!(
                    "failed to delete autogen package '{}': {source}",
                    target.display()
                ))
            })?;
        }
        manifest
            .packages
            .retain(|entry| entry.package_id != package_id);
        db.delete_generated_tracked_package(&package_id, &local_autogen_id)?;
        deleted.push(json!({
            "package_id": package_id.to_string(),
            "output_relative_path": relative_path,
        }));
    }
    write_autogen_manifest(&repo_path, &manifest)?;
    Ok(json!({
        "target_repo_id": LOCAL_AUTOGEN_REPOSITORY_ID,
        "target_repo_path": repo_path,
        "deleted_count": deleted.len(),
        "deleted": deleted,
        "preserved_to_local": preserved,
    }))
}

fn accepted_preview_candidates<'a>(
    preview: &'a Value,
    acceptance: &AutogenAcceptance,
) -> Result<Vec<&'a Value>, CliError> {
    let candidates = preview
        .get("candidates")
        .and_then(Value::as_array)
        .ok_or_else(|| CliError::Autogen("autogen preview missing candidates".to_owned()))?;
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

fn preview_package_id(candidate: &Value) -> Result<PackageId, CliError> {
    candidate
        .get("package_id")
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::Autogen("preview candidate missing package_id".to_owned()))?
        .parse()
        .map_err(|source: getter_core::PackageIdError| CliError::Autogen(source.to_string()))
}

fn preview_relative_path(candidate: &Value) -> Result<PathBuf, CliError> {
    candidate
        .get("output_relative_path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| {
            CliError::Autogen("preview candidate missing output_relative_path".to_owned())
        })
}

fn ensure_local_autogen_repository(data_dir: &Path, db: &MainDb) -> Result<PathBuf, CliError> {
    let repo_path = local_autogen_repo_path(data_dir);
    ensure_repository_layout(&repo_path, &local_autogen_repo_toml())?;
    db.upsert_repository(
        &RepositoryMetadata {
            id: RepositoryId::new(LOCAL_AUTOGEN_REPOSITORY_ID).expect("valid id"),
            name: LOCAL_AUTOGEN_REPOSITORY_NAME.to_owned(),
            priority: RepositoryPriority::LOCAL_AUTOGEN,
            api_version: REPO_API_VERSION_V1.to_owned(),
        },
        Some(&repo_path),
        None,
    )?;
    Ok(repo_path)
}

fn ensure_local_repository(data_dir: &Path, db: &MainDb) -> Result<PathBuf, CliError> {
    let local_id = RepositoryId::new(LOCAL_REPOSITORY_ID).expect("valid id");
    if let Ok(existing) = find_repository(db, &local_id) {
        let repo_path = repo_path(&existing)?;
        ensure_repository_layout(&repo_path, &local_repo_toml())?;
        return Ok(repo_path);
    }

    let repo_path = data_dir.join("repositories").join(LOCAL_REPOSITORY_ID);
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

fn ensure_repository_layout(repo_path: &Path, repo_toml: &str) -> Result<(), CliError> {
    fs::create_dir_all(repo_path.join("packages")).map_err(|source| {
        CliError::Autogen(format!(
            "failed to create repository packages dir '{}': {source}",
            repo_path.display()
        ))
    })?;
    fs::create_dir_all(repo_path.join("lib")).map_err(|source| {
        CliError::Autogen(format!(
            "failed to create repository lib dir '{}': {source}",
            repo_path.display()
        ))
    })?;
    fs::create_dir_all(repo_path.join("templates")).map_err(|source| {
        CliError::Autogen(format!(
            "failed to create repository templates dir '{}': {source}",
            repo_path.display()
        ))
    })?;
    let repo_toml_path = repo_path.join("repo.toml");
    if !repo_toml_path.exists() {
        fs::write(&repo_toml_path, repo_toml).map_err(|source| {
            CliError::Autogen(format!(
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
) -> Result<Value, CliError> {
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
            CliError::Autogen(format!(
                "failed to create local preservation directory '{}': {source}",
                parent.display()
            ))
        })?;
    }
    fs::write(&target, content).map_err(|source| {
        CliError::Autogen(format!(
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

fn read_autogen_manifest(repo_path: &Path) -> Result<Option<AutogenManifest>, CliError> {
    let path = repo_path.join(AUTOGEN_MANIFEST_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|source| {
        CliError::Autogen(format!(
            "failed to read autogen manifest '{}': {source}",
            path.display()
        ))
    })?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|source| CliError::Autogen(format!("failed to parse autogen manifest: {source}")))
}

fn write_autogen_manifest(repo_path: &Path, manifest: &AutogenManifest) -> Result<(), CliError> {
    fs::create_dir_all(repo_path).map_err(|source| {
        CliError::Autogen(format!(
            "failed to create autogen repository '{}': {source}",
            repo_path.display()
        ))
    })?;
    let path = repo_path.join(AUTOGEN_MANIFEST_FILE);
    let bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|source| CliError::Autogen(format!("failed to serialize manifest: {source}")))?;
    fs::write(&path, bytes).map_err(|source| {
        CliError::Autogen(format!(
            "failed to write autogen manifest '{}': {source}",
            path.display()
        ))
    })
}

fn empty_autogen_manifest() -> AutogenManifest {
    AutogenManifest {
        version: getter_core::autogen::AUTOGEN_MANIFEST_VERSION,
        repository_id: RepositoryId::new(LOCAL_AUTOGEN_REPOSITORY_ID).expect("valid id"),
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

fn safe_join(root: &Path, relative: &Path) -> Result<PathBuf, CliError> {
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(CliError::Autogen(format!(
            "unsafe relative path '{}'",
            relative.display()
        )));
    }
    Ok(root.join(relative))
}

fn local_autogen_repo_path(data_dir: &Path) -> PathBuf {
    data_dir
        .join("repositories")
        .join(LOCAL_AUTOGEN_REPOSITORY_ID)
}

fn repo_path(repo: &StoredRepository) -> Result<PathBuf, CliError> {
    repo.path
        .as_ref()
        .map(PathBuf::from)
        .ok_or_else(|| CliError::Repository(format!("repository '{}' has no path", repo.id)))
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

fn repository_metadata_json(
    metadata: &RepositoryMetadata,
    path: Option<&Path>,
    revision: Option<&str>,
) -> Value {
    json!({
        "id": metadata.id.as_str(),
        "name": metadata.name,
        "priority": metadata.priority.value(),
        "api_version": metadata.api_version,
        "path": path.map(|path| path.to_string_lossy().to_string()),
        "revision": revision,
    })
}

fn package_json(package: getter_core::ResolvedPackage) -> Result<Value, CliError> {
    serde_json::to_value(package)
        .map_err(|source| CliError::PackageEval(format!("failed to serialize package: {source}")))
}

fn tracked_packages_json(packages: Vec<StoredTrackedPackage>) -> Vec<Value> {
    packages
        .into_iter()
        .map(|package| {
            json!({
                "id": package.package_id.to_string(),
                "enabled": package.enabled,
                "favorite": package.favorite,
                "ignored_version": package.ignored_version,
                "repository_id": package.repository_id.map(|id| id.to_string()),
                "package_resolution": package.package_resolution.as_str(),
            })
        })
        .collect()
}

fn import_legacy_room_bundle(db: &MainDb, bundle: &LegacyRoomBundle) -> Result<(), CliError> {
    let mut packages = Vec::new();
    for app in &bundle.apps {
        let mapping = map_legacy_app(
            &LegacyAppRecord {
                kind: app.kind.to_legacy_kind(),
                installed_id: app.installed_id.clone(),
                official_package_available: app.official_package_available,
                common_conversion_available: app.common_conversion_available,
            },
            Some(&LegacyExtraAppRecord {
                ignored_version: app.ignored_version.clone(),
                favorite: app.favorite,
            }),
        )
        .map_err(|source| CliError::Storage(source.to_string()))?;
        packages.push(tracked_package_upsert(mapping));
    }

    let report_json = json!({
        "ok": true,
        "source": "legacy-room-bundle",
        "imported_records": bundle.apps.len(),
    })
    .to_string();
    db.import_tracked_packages_with_migration_record(
        &packages,
        &MigrationRecordUpsert {
            id: LEGACY_ROOM_MIGRATION_ID,
            source: "legacy-room-bundle",
            report_json: &report_json,
        },
    )?;
    Ok(())
}

fn import_legacy_room_db(db: &MainDb, import: &LegacyRoomDbImport) -> Result<(), CliError> {
    let mut packages = Vec::new();
    for app in &import.apps {
        let mapping = map_legacy_app(&app.app, Some(&app.user_state))
            .map_err(|source| CliError::Storage(source.to_string()))?;
        packages.push(tracked_package_upsert(mapping));
    }

    let report_json = json!({
        "ok": true,
        "source": "legacy-room-db",
        "version": import.version,
        "imported_records": import.apps.len(),
        "source_counts": source_counts_json(import),
        "warnings": import_warnings_json(&import.warnings),
    })
    .to_string();
    db.import_tracked_packages_with_migration_record(
        &packages,
        &MigrationRecordUpsert {
            id: LEGACY_ROOM_MIGRATION_ID,
            source: "legacy-room-db",
            report_json: &report_json,
        },
    )?;
    Ok(())
}

fn tracked_package_upsert(
    mapping: getter_storage::legacy_room::LegacyAppMapping,
) -> TrackedPackageUpsert {
    TrackedPackageUpsert {
        package_id: mapping.package_id,
        enabled: true,
        favorite: mapping.user_state.favorite,
        ignored_version: mapping.user_state.ignored_version,
        repository_id: None,
        package_resolution: stored_resolution(mapping.package_resolution),
    }
}

fn stored_resolution(resolution: LegacyPackageResolution) -> StoredPackageResolution {
    match resolution {
        LegacyPackageResolution::OfficialRepositoryPackage => {
            StoredPackageResolution::OfficialRepositoryPackage
        }
        LegacyPackageResolution::GenerateLocalPackage => {
            StoredPackageResolution::GenerateLocalPackage
        }
        LegacyPackageResolution::MissingPackageDefinition => {
            StoredPackageResolution::MissingPackageDefinition
        }
    }
}

fn legacy_db_import_error(
    data_dir: &Path,
    legacy_db: &Path,
    source: LegacyRoomReadError,
) -> CliError {
    let (code, unsupported) = match source {
        LegacyRoomReadError::UnsupportedVersion { .. } => ("migration.unsupported_db", true),
        LegacyRoomReadError::Sqlite(_) | LegacyRoomReadError::MissingRequiredTable(_) => {
            ("migration.invalid_db", false)
        }
    };
    match create_migration_report_with_source_counts(
        data_dir,
        legacy_db,
        code,
        &source.to_string(),
        0,
        0,
        &[],
        None,
    ) {
        Ok(report_path) if unsupported => CliError::UnsupportedLegacyDb { report_path },
        Ok(report_path) => CliError::InvalidLegacyDb { report_path },
        Err(error) => error,
    }
}

fn source_counts_json(import: &LegacyRoomDbImport) -> Value {
    json!({
        "app_rows": import.source_counts.app_rows,
        "extra_app_rows": import.source_counts.extra_app_rows,
        "hub_rows": import.source_counts.hub_rows,
        "extra_hub_rows": import.source_counts.extra_hub_rows,
    })
}

fn import_warnings_json(warnings: &[LegacyRoomImportWarning]) -> Vec<Value> {
    warnings
        .iter()
        .map(|warning| {
            json!({
                "code": warning.code(),
                "message": warning.message(),
            })
        })
        .collect()
}

fn main_db_path(data_dir: &Path) -> PathBuf {
    data_dir.join(MAIN_DB_FILE)
}

fn cache_db_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CACHE_DB_FILE)
}

fn create_migration_report(
    data_dir: &Path,
    source_file: &Path,
    code: &str,
    detail: &str,
    imported_records: u64,
    tracked_records: u64,
    warnings: &[Value],
) -> Result<PathBuf, CliError> {
    create_migration_report_with_source_counts(
        data_dir,
        source_file,
        code,
        detail,
        imported_records,
        tracked_records,
        warnings,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn create_migration_report_with_source_counts(
    data_dir: &Path,
    source_file: &Path,
    code: &str,
    detail: &str,
    imported_records: u64,
    tracked_records: u64,
    warnings: &[Value],
    source_counts: Option<&Value>,
) -> Result<PathBuf, CliError> {
    let reports_dir = data_dir.join(MIGRATION_REPORTS_DIR);
    fs::create_dir_all(&reports_dir).map_err(|source| {
        CliError::Storage(format!(
            "failed to create migration report directory: {source}"
        ))
    })?;
    let report_path = reports_dir.join(report_file_name(code));
    let report = MigrationReport {
        ok: code == "migration.imported",
        code,
        message: match code {
            "migration.invalid_bundle" => "Legacy Room export bundle is invalid",
            "migration.unsupported_bundle" => "Legacy Room bundle import is not implemented yet",
            "migration.invalid_db" => "Legacy Room database is invalid",
            "migration.unsupported_db" => "Legacy Room database version is not supported",
            "migration.imported" => "Legacy Room data imported",
            _ => "Legacy migration failed",
        },
        source_file_name: source_file.file_name().and_then(|name| name.to_str()),
        detail,
        imported_records,
        tracked_records,
        warnings,
        source_counts,
    };
    let bytes = serde_json::to_vec_pretty(&report)
        .map_err(|source| CliError::Storage(format!("failed to serialize report: {source}")))?;
    fs::write(&report_path, bytes)
        .map_err(|source| CliError::Storage(format!("failed to write report: {source}")))?;
    Ok(report_path)
}

fn report_file_name(code: &str) -> String {
    format!("{}.json", code.replace('.', "-"))
}

fn list_migration_reports(data_dir: &Path) -> Result<Vec<Value>, CliError> {
    let reports_dir = data_dir.join(MIGRATION_REPORTS_DIR);
    if !reports_dir.exists() {
        return Ok(Vec::new());
    }

    let mut report_paths = fs::read_dir(&reports_dir)
        .map_err(|source| CliError::Storage(format!("failed to read migration reports: {source}")))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| {
            CliError::Storage(format!("failed to read migration report entry: {source}"))
        })?;
    report_paths.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "json")
    });
    report_paths.sort();

    report_paths
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path).map_err(|source| {
                CliError::Storage(format!(
                    "failed to read migration report '{}': {source}",
                    path.display()
                ))
            })?;
            let report: Value = serde_json::from_slice(&bytes).map_err(|source| {
                CliError::Storage(format!(
                    "failed to parse migration report '{}': {source}",
                    path.display()
                ))
            })?;
            Ok(json!({
                "ok": report.get("ok").and_then(Value::as_bool).unwrap_or(false),
                "code": report.get("code").and_then(Value::as_str).unwrap_or("migration.unknown"),
                "message": report.get("message").and_then(Value::as_str).unwrap_or("Legacy migration report"),
                "source_file_name": report
                    .get("source_file_name")
                    .or_else(|| report.get("bundle_file_name"))
                    .and_then(Value::as_str),
                "imported_records": report.get("imported_records").and_then(Value::as_u64).unwrap_or(0),
                "tracked_records": report.get("tracked_records").and_then(Value::as_u64).unwrap_or(0),
                "warnings": report
                    .get("warnings")
                    .cloned()
                    .unwrap_or_else(|| Value::Array(Vec::new())),
                "source_counts": report.get("source_counts").cloned().unwrap_or(Value::Null),
            }))
        })
        .collect()
}

#[derive(Debug, Serialize)]
struct MigrationReport<'a> {
    ok: bool,
    code: &'a str,
    message: &'a str,
    source_file_name: Option<&'a str>,
    detail: &'a str,
    imported_records: u64,
    tracked_records: u64,
    warnings: &'a [Value],
    #[serde(skip_serializing_if = "Option::is_none")]
    source_counts: Option<&'a Value>,
}

fn success_envelope(command: &str, data: Value) -> String {
    envelope_to_string(json!({
        "ok": true,
        "command": command,
        "data": data,
        "warnings": []
    }))
}

fn error_envelope(command: &str, error: &CliError) -> String {
    let mut error_value = json!({
        "code": error.code(),
        "message": error.message(),
    });
    if let Some(detail) = error.detail() {
        error_value["detail"] = json!(detail);
    }
    if let Some(report_path) = error.report_path() {
        error_value["report_path"] = json!(report_path);
    }

    envelope_to_string(json!({
        "ok": false,
        "command": command,
        "error": error_value,
    }))
}

fn envelope_to_string(value: Value) -> String {
    let mut rendered = serde_json::to_string_pretty(&value).expect("JSON envelope is serializable");
    rendered.push('\n');
    rendered
}

fn usage_text() -> String {
    "Usage: getter --data-dir <path> <init|app list|repo list|repo add <repo-id> <path> [--priority <n>]|repo eval <repo-id>|repo validate <path>|package eval <package-id> [--repo <repo-id>]|storage validate|hub list|update check --fixture <fixture.json>|task submit --request <request.json>|task run <task-id>|task list|task cancel <task-id>|task events --after <cursor> --limit <n>|task install-result <handoff-id> --status <accepted|succeeded|failed|canceled>|autogen installed preview --inventory <installed.json>|autogen installed apply --preview <preview.json> (--accept-all|--accept <package-id>...)|autogen cleanup preview --inventory <installed.json>|autogen cleanup apply --preview <preview.json> (--accept-all|--accept <package-id>...)|legacy import-room-bundle <bundle.json>|legacy import-room-db <db.sqlite>|legacy report-list>\n".to_owned()
}

#[derive(Debug, Deserialize)]
struct LegacyRoomBundle {
    format: String,
    version: u32,
    #[serde(default)]
    apps: Vec<LegacyBundleApp>,
}

#[derive(Debug, Deserialize)]
struct LegacyBundleApp {
    kind: LegacyBundleAppKind,
    installed_id: String,
    #[serde(default)]
    official_package_available: bool,
    #[serde(default)]
    common_conversion_available: bool,
    #[serde(default)]
    ignored_version: Option<String>,
    #[serde(default)]
    favorite: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyBundleAppKind {
    Android,
    Magisk,
}

impl LegacyBundleAppKind {
    const fn to_legacy_kind(&self) -> LegacyAppKind {
        match self {
            Self::Android => LegacyAppKind::Android,
            Self::Magisk => LegacyAppKind::Magisk,
        }
    }
}

impl CliCommand {
    fn name(&self) -> &'static str {
        match self {
            Self::Init => "init",
            Self::AppList => "app list",
            Self::HubList => "hub list",
            Self::RepoList => "repo list",
            Self::RepoAdd { .. } => "repo add",
            Self::RepoEval { .. } => "repo eval",
            Self::RepoValidate { .. } => "repo validate",
            Self::PackageEval { .. } => "package eval",
            Self::StorageValidate => "storage validate",
            Self::UpdateCheck { .. } => "update check",
            Self::TaskSubmit { .. } => "task submit",
            Self::TaskRun { .. } => "task run",
            Self::TaskList => "task list",
            Self::TaskCancel { .. } => "task cancel",
            Self::TaskEvents { .. } => "task events",
            Self::TaskInstallResult { .. } => "task install-result",
            Self::AutogenInstalledPreview { .. } => "autogen installed preview",
            Self::AutogenInstalledApply { .. } => "autogen installed apply",
            Self::AutogenCleanupPreview { .. } => "autogen cleanup preview",
            Self::AutogenCleanupApply { .. } => "autogen cleanup apply",
            Self::LegacyImportRoomBundle { .. } => "legacy import-room-bundle",
            Self::LegacyImportRoomDb { .. } => "legacy import-room-db",
            Self::LegacyReportList => "legacy report-list",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_global_data_dir_and_app_list_command() {
        let invocation =
            parse_args(["getter", "--data-dir", "/tmp/ua-getter", "app", "list"]).unwrap();

        assert_eq!(invocation.data_dir, PathBuf::from("/tmp/ua-getter"));
        assert_eq!(invocation.command, CliCommand::AppList);
    }

    #[test]
    fn run_init_creates_sqlite_database_files_and_json_envelope() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("getter-data");

        let output = run([
            "getter".to_owned(),
            "--data-dir".to_owned(),
            data_dir.to_string_lossy().to_string(),
            "init".to_owned(),
        ]);

        assert_eq!(output.exit_code, ExitCode::Success);
        assert!(data_dir.join(MAIN_DB_FILE).is_file());
        assert!(data_dir.join(CACHE_DB_FILE).is_file());
        let json: Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["command"], "init");
    }

    #[test]
    fn malformed_bundle_report_does_not_create_imported_state() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("getter-data");
        let bundle = temp.path().join("bad.json");
        fs::write(&bundle, "not-json").unwrap();

        let init = run([
            "getter".to_owned(),
            "--data-dir".to_owned(),
            data_dir.to_string_lossy().to_string(),
            "init".to_owned(),
        ]);
        assert_eq!(init.exit_code, ExitCode::Success);

        let output = run([
            "getter".to_owned(),
            "--data-dir".to_owned(),
            data_dir.to_string_lossy().to_string(),
            "legacy".to_owned(),
            "import-room-bundle".to_owned(),
            bundle.to_string_lossy().to_string(),
        ]);

        assert_eq!(output.exit_code, ExitCode::Migration);
        let json: Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(json["ok"], false);
        assert_eq!(json["error"]["code"], "migration.invalid_bundle");
        let report_path = json["error"]["report_path"].as_str().unwrap();
        assert!(Path::new(report_path).is_file());
    }
}
