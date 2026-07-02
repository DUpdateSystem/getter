//! User-facing getter CLI implementation.
//!
//! The CLI is intentionally thin: it owns command parsing and JSON envelopes,
//! while durable state is initialized through `getter-storage` so the command
//! surface exercises the same Rust-owned SQLite direction used by embedders.

use getter_core::autogen::InstalledInventory;
use getter_core::diagnostics::validate_repository_path;
use getter_core::lua::evaluate_package_directory_script;
use getter_core::repository::{
    default_repository_priority, GetterDataDirLayout, RepositoryMetadata,
    RepositoryPackageDirectoryLayout, REPOSITORY_ROOT_METADATA_FILE, REPO_API_VERSION_V1,
};
use getter_core::runtime::{GetterRuntime, SealedActionPlan};
use getter_core::task::{
    DownloadTaskRequest, InstallHandoffStatus, TaskEventPage, DOWNLOAD_REQUEST_FORMAT,
    DOWNLOAD_REQUEST_VERSION,
};
use getter_core::update::{run_offline_update_check, OfflineUpdateCheckFixture};
use getter_core::{PackageId, RepositoryId, RepositoryPriority};
use getter_downloader::{
    cancel_download_task, record_install_result, run_fake_download_task, submit_fake_download_task,
};
use getter_operations::autogen::{self, AutogenAcceptance, AutogenOperationError};
use getter_operations::fdroid_autogen;
use getter_operations::github_autogen;
use getter_operations::github_latest_commit::{self, GithubLatestCommitOperationError};
use getter_operations::github_releases::{self, GithubReleaseOperationError};
use getter_operations::legacy_room::{self, LegacyRoomOperationError};
use getter_operations::runtime as runtime_operations;
use getter_storage::legacy_room::{
    map_legacy_app, LegacyAppKind, LegacyAppRecord, LegacyExtraAppRecord, LegacyPackageResolution,
};
use getter_storage::{
    CacheDb, MainDb, MigrationRecordUpsert, StorageError, StoredPackageResolution,
    StoredRepository, StoredTrackedPackage, TrackedPackageUpsert,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const MAIN_DB_FILE: &str = "main.db";
const CACHE_DB_FILE: &str = "cache.db";
const MIGRATION_REPORTS_DIR: &str = "migration-reports";
const LEGACY_ROOM_MIGRATION_ID: &str = "legacy-room-v17";
const REPOSITORY_ROOT_METADATA_STARTER: &str = r#"{
  "version": 1,
  // Autogen writes to "autogen" by default. Uncomment and change this
  // if generated packages should target another existing repository alias.
  // "generated_repository": "autogen",
  "priority": {
    "local": 100,
    "official": 0,
    "autogen": -1
  }
}
"#;

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
    VersionPin {
        package_id: PackageId,
        version: String,
    },
    VersionUnpin {
        package_id: PackageId,
    },
    UpdateCheck {
        fixture: PathBuf,
    },
    DebugFakeTaskSubmit {
        request: PathBuf,
    },
    DebugFakeTaskRun {
        task_id: String,
    },
    DebugFakeTaskList,
    DebugFakeTaskCancel {
        task_id: String,
    },
    DebugFakeTaskEvents {
        after: u64,
        limit: usize,
    },
    DebugFakeTaskInstallResult {
        handoff_id: String,
        status: InstallHandoffStatus,
    },
    RuntimeScript {
        script: PathBuf,
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
    AutogenFdroidPreview {
        index: PathBuf,
        inventory: Option<PathBuf>,
        package_names: Vec<String>,
    },
    AutogenFdroidApply {
        preview: PathBuf,
        acceptance: AutogenAcceptance,
    },
    AutogenGithubPreview {
        owner: String,
        repo: String,
        android_package: String,
        display_name: Option<String>,
        releases: PathBuf,
        asset_include: Option<String>,
        asset_exclude: Option<String>,
        include_prereleases: bool,
    },
    AutogenGithubApply {
        preview: PathBuf,
        acceptance: AutogenAcceptance,
    },
    ProviderGithubReleases {
        owner: String,
        repo: String,
        releases: PathBuf,
        asset_include: Option<String>,
        asset_exclude: Option<String>,
        include_prereleases: bool,
        refresh: bool,
    },
    ProviderGithubLatestCommit {
        owner: String,
        repo: String,
        reference: Option<String>,
        commit: PathBuf,
        refresh: bool,
    },
    LegacyImportRoomBundle {
        bundle: PathBuf,
    },
    LegacyImportRoomDb {
        db: PathBuf,
    },
    LegacyReportList,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Success = 0,
    GenericFailure = 1,
    Usage = 2,
    Storage = 10,
    Migration = 20,
    Provider = 30,
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
    #[error("runtime error: {0}")]
    Runtime(String),
    #[error("autogen error: {0}")]
    Autogen(String),
    #[error("provider error: {0}")]
    Provider(String),
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
            Self::Repository(_)
            | Self::PackageEval(_)
            | Self::Update(_)
            | Self::Runtime(_)
            | Self::Autogen(_) => ExitCode::GenericFailure,
            Self::Provider(_) => ExitCode::Provider,
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
            Self::Runtime(_) => "runtime.error",
            Self::Autogen(_) => "autogen.error",
            Self::Provider(_) => "provider.error",
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
            Self::Runtime(_) => "Getter runtime operation failed",
            Self::Autogen(_) => "Getter autogen operation failed",
            Self::Provider(_) => "Getter provider operation failed",
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
            | Self::Runtime(detail)
            | Self::Autogen(detail)
            | Self::Provider(detail) => Some(detail.as_str()),
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
            | Self::Runtime(_)
            | Self::Autogen(_)
            | Self::Provider(_) => None,
        }
    }
}

impl From<StorageError> for CliError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value.to_string())
    }
}

impl From<AutogenOperationError> for CliError {
    fn from(value: AutogenOperationError) -> Self {
        match value {
            AutogenOperationError::Storage(source) => Self::Storage(source.to_string()),
            AutogenOperationError::Repository(detail) => Self::Repository(detail),
            AutogenOperationError::MissingGeneratedRepository { .. } => {
                Self::Autogen(value.to_string())
            }
            AutogenOperationError::Autogen(detail) => Self::Autogen(detail),
        }
    }
}

impl From<LegacyRoomOperationError> for CliError {
    fn from(value: LegacyRoomOperationError) -> Self {
        match value {
            LegacyRoomOperationError::Storage(detail) => Self::Storage(detail),
            LegacyRoomOperationError::UnsupportedDb { report_path } => {
                Self::UnsupportedLegacyDb { report_path }
            }
            LegacyRoomOperationError::InvalidDb { report_path } => {
                Self::InvalidLegacyDb { report_path }
            }
        }
    }
}

impl From<GithubLatestCommitOperationError> for CliError {
    fn from(value: GithubLatestCommitOperationError) -> Self {
        match value {
            GithubLatestCommitOperationError::Storage(source) => Self::Storage(source.to_string()),
            GithubLatestCommitOperationError::InvalidRequest(detail) => Self::Usage(detail),
            other => Self::Provider(other.to_string()),
        }
    }
}

impl From<GithubReleaseOperationError> for CliError {
    fn from(value: GithubReleaseOperationError) -> Self {
        match value {
            GithubReleaseOperationError::Storage(source) => Self::Storage(source.to_string()),
            GithubReleaseOperationError::InvalidRequest(detail) => Self::Usage(detail),
            other => Self::Provider(other.to_string()),
        }
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
        [domain, command, package_id, version] if domain == "version" && command == "pin" => {
            CliCommand::VersionPin {
                package_id: parse_package_id(package_id)?,
                version: version.clone(),
            }
        }
        [domain, command, package_id] if domain == "version" && command == "unpin" => {
            CliCommand::VersionUnpin {
                package_id: parse_package_id(package_id)?,
            }
        }
        [domain, command, flag, fixture]
            if domain == "update" && command == "check" && flag == "--fixture" =>
        {
            CliCommand::UpdateCheck {
                fixture: PathBuf::from(fixture),
            }
        }
        [domain, subject, command, flag, request]
            if domain == "debug"
                && subject == "fake-task"
                && command == "submit"
                && flag == "--request" =>
        {
            CliCommand::DebugFakeTaskSubmit {
                request: PathBuf::from(request),
            }
        }
        [domain, subject, command, task_id]
            if domain == "debug" && subject == "fake-task" && command == "run" =>
        {
            CliCommand::DebugFakeTaskRun {
                task_id: task_id.clone(),
            }
        }
        [domain, subject, command]
            if domain == "debug" && subject == "fake-task" && command == "list" =>
        {
            CliCommand::DebugFakeTaskList
        }
        [domain, subject, command, task_id]
            if domain == "debug" && subject == "fake-task" && command == "cancel" =>
        {
            CliCommand::DebugFakeTaskCancel {
                task_id: task_id.clone(),
            }
        }
        [domain, subject, command, after_flag, after, limit_flag, limit]
            if domain == "debug"
                && subject == "fake-task"
                && command == "events"
                && after_flag == "--after"
                && limit_flag == "--limit" =>
        {
            CliCommand::DebugFakeTaskEvents {
                after: parse_u64(after, "--after")?,
                limit: parse_positive_usize(limit, "--limit")?,
            }
        }
        [domain, subject, command, handoff_id, status_flag, status]
            if domain == "debug"
                && subject == "fake-task"
                && command == "install-result"
                && status_flag == "--status" =>
        {
            CliCommand::DebugFakeTaskInstallResult {
                handoff_id: handoff_id.clone(),
                status: parse_install_handoff_status(status)?,
            }
        }
        [domain, command, flag, script]
            if domain == "runtime" && command == "script" && flag == "--script" =>
        {
            CliCommand::RuntimeScript {
                script: PathBuf::from(script),
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
        [domain, subject, action, rest @ ..]
            if domain == "autogen" && subject == "fdroid" && action == "preview" =>
        {
            let (index, inventory, package_names) = parse_fdroid_autogen_preview_args(rest)?;
            CliCommand::AutogenFdroidPreview {
                index,
                inventory,
                package_names,
            }
        }
        [domain, subject, action, flag, preview, rest @ ..]
            if domain == "autogen"
                && subject == "fdroid"
                && action == "apply"
                && flag == "--preview" =>
        {
            CliCommand::AutogenFdroidApply {
                preview: PathBuf::from(preview),
                acceptance: parse_autogen_acceptance(rest)?,
            }
        }
        [domain, subject, action, rest @ ..]
            if domain == "autogen" && subject == "github" && action == "preview" =>
        {
            let args = parse_autogen_github_preview_args(rest)?;
            CliCommand::AutogenGithubPreview {
                owner: args.owner,
                repo: args.repo,
                android_package: args.android_package,
                display_name: args.display_name,
                releases: args.releases,
                asset_include: args.asset_include,
                asset_exclude: args.asset_exclude,
                include_prereleases: args.include_prereleases,
            }
        }
        [domain, subject, action, flag, preview, rest @ ..]
            if domain == "autogen"
                && subject == "github"
                && action == "apply"
                && flag == "--preview" =>
        {
            CliCommand::AutogenGithubApply {
                preview: PathBuf::from(preview),
                acceptance: parse_autogen_acceptance(rest)?,
            }
        }
        [domain, provider, action, rest @ ..]
            if domain == "provider" && provider == "github" && action == "releases" =>
        {
            let args = parse_provider_github_releases_args(rest)?;
            CliCommand::ProviderGithubReleases {
                owner: args.owner,
                repo: args.repo,
                releases: args.releases,
                asset_include: args.asset_include,
                asset_exclude: args.asset_exclude,
                include_prereleases: args.include_prereleases,
                refresh: args.refresh,
            }
        }
        [domain, provider, action, rest @ ..]
            if domain == "provider" && provider == "github" && action == "latest-commit" =>
        {
            let args = parse_provider_github_latest_commit_args(rest)?;
            CliCommand::ProviderGithubLatestCommit {
                owner: args.owner,
                repo: args.repo,
                reference: args.reference,
                commit: args.commit,
                refresh: args.refresh,
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
            let layout = initialize_data_dir_layout(&invocation.data_dir)?;
            Ok(json!({
                "data_dir": layout.root,
                "main_db": layout.main_db,
                "cache_db": layout.cache_db,
                "repo": layout.repository_root,
                "rc": layout.runtime_config_root,
                "repo_metadata": layout.repository_root.join(REPOSITORY_ROOT_METADATA_FILE),
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
            let metadata = load_repository_metadata(&id, &path, priority)?;
            db.upsert_repository(&metadata, Some(&path), None)?;
            Ok(json!({ "repository": repository_metadata_json(&metadata, Some(&path), None) }))
        }
        CliCommand::RepoEval { id } => {
            let db = open_main_db(&invocation.data_dir)?;
            let repo = find_repository(&db, &id)?;
            let packages = evaluate_repository_packages(&repo)?
                .into_iter()
                .map(package_json)
                .collect::<Result<Vec<_>, _>>()?;
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
        CliCommand::VersionPin {
            package_id,
            version,
        } => {
            let db = open_main_db(&invocation.data_dir)?;
            let package = db.set_tracked_package_pin_version(&package_id, Some(&version))?;
            Ok(json!({ "package": tracked_package_json(package) }))
        }
        CliCommand::VersionUnpin { package_id } => {
            let db = open_main_db(&invocation.data_dir)?;
            let package = db.set_tracked_package_pin_version(&package_id, None)?;
            Ok(json!({ "package": tracked_package_json(package) }))
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
        CliCommand::DebugFakeTaskSubmit { request } => {
            let db = open_main_db(&invocation.data_dir)?;
            let request = read_download_task_request(&request)?;
            serde_json::to_value(submit_fake_download_task(&db, request).map_err(|source| {
                CliError::Download(format!("offline fake task submit failed: {source}"))
            })?)
            .map_err(|source| CliError::Download(format!("failed to serialize task: {source}")))
        }
        CliCommand::DebugFakeTaskRun { task_id } => {
            let db = open_main_db(&invocation.data_dir)?;
            serde_json::to_value(run_fake_download_task(&db, &task_id).map_err(|source| {
                CliError::Download(format!("offline fake task run failed: {source}"))
            })?)
            .map_err(|source| CliError::Download(format!("failed to serialize task: {source}")))
        }
        CliCommand::DebugFakeTaskList => {
            let db = open_main_db(&invocation.data_dir)?;
            Ok(json!({ "tasks": db.download_tasks()? }))
        }
        CliCommand::DebugFakeTaskCancel { task_id } => {
            let db = open_main_db(&invocation.data_dir)?;
            serde_json::to_value(cancel_download_task(&db, &task_id).map_err(|source| {
                CliError::Download(format!("offline fake task cancel failed: {source}"))
            })?)
            .map_err(|source| CliError::Download(format!("failed to serialize task: {source}")))
        }
        CliCommand::DebugFakeTaskEvents { after, limit } => {
            let db = open_main_db(&invocation.data_dir)?;
            let events: TaskEventPage = db.task_events_after(after, limit)?;
            serde_json::to_value(events).map_err(|source| {
                CliError::Download(format!("failed to serialize fake task events: {source}"))
            })
        }
        CliCommand::DebugFakeTaskInstallResult { handoff_id, status } => {
            let db = open_main_db(&invocation.data_dir)?;
            serde_json::to_value(record_install_result(&db, &handoff_id, status).map_err(
                |source| {
                    CliError::Download(format!("offline fake install result failed: {source}"))
                },
            )?)
            .map_err(|source| CliError::Download(format!("failed to serialize handoff: {source}")))
        }
        CliCommand::RuntimeScript { script } => run_runtime_script(&invocation.data_dir, &script),
        CliCommand::AutogenInstalledPreview { inventory } => {
            let db = open_main_db(&invocation.data_dir)?;
            let inventory = read_installed_inventory(&inventory)?;
            let plan =
                autogen::build_installed_autogen_plan(&invocation.data_dir, &db, &inventory)?;
            autogen::installed_preview_json(&invocation.data_dir, &plan).map_err(CliError::from)
        }
        CliCommand::AutogenInstalledApply {
            preview,
            acceptance,
        } => {
            let db = open_main_db(&invocation.data_dir)?;
            let preview = read_autogen_preview(&preview, "installed.preview")?;
            autogen::apply_installed_preview(&invocation.data_dir, &db, &preview, &acceptance)
                .map_err(CliError::from)
        }
        CliCommand::AutogenCleanupPreview { inventory } => {
            let db = open_main_db(&invocation.data_dir)?;
            let inventory = read_installed_inventory(&inventory)?;
            autogen::cleanup_preview_json(&invocation.data_dir, &db, &inventory)
                .map_err(CliError::from)
        }
        CliCommand::AutogenCleanupApply {
            preview,
            acceptance,
        } => {
            let db = open_main_db(&invocation.data_dir)?;
            let preview = read_autogen_preview(&preview, "cleanup.preview")?;
            autogen::apply_cleanup_preview(&invocation.data_dir, &db, &preview, &acceptance)
                .map_err(CliError::from)
        }
        CliCommand::AutogenFdroidPreview {
            index,
            inventory,
            package_names,
        } => {
            let db = open_main_db(&invocation.data_dir)?;
            let cache_db = open_cache_db(&invocation.data_dir)?;
            let index_xml = read_fdroid_index(&index)?;
            let installed_inventory = inventory
                .as_deref()
                .map(read_installed_inventory)
                .transpose()?;
            let request = json!({
                "index_xml": index_xml,
                "package_names": package_names,
                "installed_inventory": installed_inventory,
            });
            fdroid_autogen::preview_fdroid_packages_json(
                &invocation.data_dir,
                &db,
                &cache_db,
                &request.to_string(),
            )
            .map_err(CliError::from)
        }
        CliCommand::AutogenFdroidApply {
            preview,
            acceptance,
        } => {
            let db = open_main_db(&invocation.data_dir)?;
            let preview = read_autogen_preview(&preview, "fdroid.autogen.preview")?;
            fdroid_autogen::apply_fdroid_preview_json(
                &invocation.data_dir,
                &db,
                &preview,
                &acceptance,
            )
            .map_err(CliError::from)
        }
        CliCommand::AutogenGithubPreview {
            owner,
            repo,
            android_package,
            display_name,
            releases,
            asset_include,
            asset_exclude,
            include_prereleases,
        } => {
            let db = open_main_db(&invocation.data_dir)?;
            let cache_db = open_cache_db(&invocation.data_dir)?;
            let releases_json = read_github_releases_fixture(&releases)?;
            let request = json!({
                "owner": owner,
                "repo": repo,
                "android_package": android_package,
                "display_name": display_name,
                "releases_json": releases_json,
                "include_prereleases": include_prereleases,
                "asset": {
                    "include": asset_include,
                    "exclude": asset_exclude,
                },
            });
            github_autogen::preview_github_android_package_json(
                &invocation.data_dir,
                &db,
                &cache_db,
                &request.to_string(),
            )
            .map_err(CliError::from)
        }
        CliCommand::AutogenGithubApply {
            preview,
            acceptance,
        } => {
            let db = open_main_db(&invocation.data_dir)?;
            let preview = read_autogen_preview(&preview, "github.autogen.preview")?;
            github_autogen::apply_github_preview_json(
                &invocation.data_dir,
                &db,
                &preview,
                &acceptance,
            )
            .map_err(CliError::from)
        }
        CliCommand::ProviderGithubReleases {
            owner,
            repo,
            releases,
            asset_include,
            asset_exclude,
            include_prereleases,
            refresh,
        } => {
            let db = open_cache_db(&invocation.data_dir)?;
            let releases_json = read_github_releases_fixture(&releases)?;
            let request = json!({
                "owner": owner,
                "repo": repo,
                "mode": if refresh { "force_refresh" } else { "use_cached" },
                "releases_json": releases_json,
                "include_prereleases": include_prereleases,
                "asset": {
                    "include": asset_include,
                    "exclude": asset_exclude,
                },
            });
            github_releases::github_releases_json(&db, &request.to_string()).map_err(CliError::from)
        }
        CliCommand::ProviderGithubLatestCommit {
            owner,
            repo,
            reference,
            commit,
            refresh,
        } => {
            let db = open_cache_db(&invocation.data_dir)?;
            let commit_json = read_github_commit_fixture(&commit)?;
            let request = json!({
                "owner": owner,
                "repo": repo,
                "ref": reference,
                "mode": if refresh { "force_refresh" } else { "use_cached" },
                "commit_json": commit_json,
            });
            github_latest_commit::github_latest_commit_json(&db, &request.to_string())
                .map_err(CliError::from)
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
            legacy_room::import_room_db_json(&invocation.data_dir, &legacy_db)
                .map_err(CliError::from)
        }
        CliCommand::LegacyReportList => {
            legacy_room::report_list_json(&invocation.data_dir).map_err(CliError::from)
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

fn initialize_data_dir_layout(data_dir: &Path) -> Result<GetterDataDirLayout, CliError> {
    let layout = GetterDataDirLayout::new(data_dir);
    fs::create_dir_all(&layout.repository_root).map_err(|source| {
        CliError::Storage(format!("failed to create repository root: {source}"))
    })?;
    fs::create_dir_all(&layout.runtime_config_root).map_err(|source| {
        CliError::Storage(format!("failed to create runtime config root: {source}"))
    })?;
    let metadata_path = layout.repository_root.join(REPOSITORY_ROOT_METADATA_FILE);
    if !metadata_path.exists() {
        fs::write(&metadata_path, REPOSITORY_ROOT_METADATA_STARTER).map_err(|source| {
            CliError::Storage(format!(
                "failed to write repository root metadata '{}': {source}",
                metadata_path.display()
            ))
        })?;
    }
    Ok(layout)
}

fn open_initialized_storage(data_dir: &Path) -> Result<(), CliError> {
    initialize_storage(data_dir)
}

fn open_main_db(data_dir: &Path) -> Result<MainDb, CliError> {
    initialize_storage(data_dir)?;
    Ok(MainDb::open(main_db_path(data_dir))?)
}

fn open_cache_db(data_dir: &Path) -> Result<CacheDb, CliError> {
    initialize_storage(data_dir)?;
    Ok(CacheDb::open(cache_db_path(data_dir))?)
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

fn parse_fdroid_autogen_preview_args(
    args: &[String],
) -> Result<(PathBuf, Option<PathBuf>, Vec<String>), CliError> {
    let mut index = None;
    let mut inventory = None;
    let mut package_names = Vec::new();
    let mut position = 0;
    while position < args.len() {
        match args[position].as_str() {
            "--index" => {
                let path = args.get(position + 1).ok_or_else(|| {
                    CliError::Usage("autogen fdroid preview --index requires a path".to_owned())
                })?;
                index = Some(PathBuf::from(path));
                position += 2;
            }
            "--inventory" => {
                let path = args.get(position + 1).ok_or_else(|| {
                    CliError::Usage(
                        "autogen fdroid preview --inventory requires an inventory path".to_owned(),
                    )
                })?;
                inventory = Some(PathBuf::from(path));
                position += 2;
            }
            "--package" => {
                let package_name = args.get(position + 1).ok_or_else(|| {
                    CliError::Usage(
                        "autogen fdroid preview --package requires a package name".to_owned(),
                    )
                })?;
                package_names.push(package_name.clone());
                position += 2;
            }
            other => {
                return Err(CliError::Usage(format!(
                    "unsupported autogen fdroid preview argument '{other}'"
                )))
            }
        }
    }
    let index = index.ok_or_else(|| {
        CliError::Usage("autogen fdroid preview requires --index <index.xml>".to_owned())
    })?;
    if package_names.is_empty() && inventory.is_none() {
        return Err(CliError::Usage(
            "autogen fdroid preview requires --package <package-name> or --inventory <installed.json>"
                .to_owned(),
        ));
    }
    Ok((index, inventory, package_names))
}

#[derive(Debug, Default, PartialEq, Eq)]
struct AutogenGithubPreviewArgs {
    owner: String,
    repo: String,
    android_package: String,
    display_name: Option<String>,
    releases: PathBuf,
    asset_include: Option<String>,
    asset_exclude: Option<String>,
    include_prereleases: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ProviderGithubReleasesArgs {
    owner: String,
    repo: String,
    releases: PathBuf,
    asset_include: Option<String>,
    asset_exclude: Option<String>,
    include_prereleases: bool,
    refresh: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ProviderGithubLatestCommitArgs {
    owner: String,
    repo: String,
    reference: Option<String>,
    commit: PathBuf,
    refresh: bool,
}

fn parse_autogen_github_preview_args(
    args: &[String],
) -> Result<AutogenGithubPreviewArgs, CliError> {
    let mut parsed = AutogenGithubPreviewArgs::default();
    let mut position = 0;
    while position < args.len() {
        match args[position].as_str() {
            "--owner" => {
                parsed.owner = args
                    .get(position + 1)
                    .ok_or_else(|| {
                        CliError::Usage(
                            "autogen github preview --owner requires an owner".to_owned(),
                        )
                    })?
                    .clone();
                position += 2;
            }
            "--repo" => {
                parsed.repo = args
                    .get(position + 1)
                    .ok_or_else(|| {
                        CliError::Usage("autogen github preview --repo requires a repo".to_owned())
                    })?
                    .clone();
                position += 2;
            }
            "--android-package" => {
                parsed.android_package = args
                    .get(position + 1)
                    .ok_or_else(|| {
                        CliError::Usage(
                            "autogen github preview --android-package requires a package name"
                                .to_owned(),
                        )
                    })?
                    .clone();
                position += 2;
            }
            "--display-name" => {
                parsed.display_name = Some(
                    args.get(position + 1)
                        .ok_or_else(|| {
                            CliError::Usage(
                                "autogen github preview --display-name requires a name".to_owned(),
                            )
                        })?
                        .clone(),
                );
                position += 2;
            }
            "--releases" => {
                let path = args.get(position + 1).ok_or_else(|| {
                    CliError::Usage(
                        "autogen github preview --releases requires a fixture path".to_owned(),
                    )
                })?;
                parsed.releases = PathBuf::from(path);
                position += 2;
            }
            "--asset-include" => {
                parsed.asset_include = Some(
                    args.get(position + 1)
                        .ok_or_else(|| {
                            CliError::Usage(
                                "autogen github preview --asset-include requires a regex"
                                    .to_owned(),
                            )
                        })?
                        .clone(),
                );
                position += 2;
            }
            "--asset-exclude" => {
                parsed.asset_exclude = Some(
                    args.get(position + 1)
                        .ok_or_else(|| {
                            CliError::Usage(
                                "autogen github preview --asset-exclude requires a regex"
                                    .to_owned(),
                            )
                        })?
                        .clone(),
                );
                position += 2;
            }
            "--include-prereleases" => {
                parsed.include_prereleases = true;
                position += 1;
            }
            other => {
                return Err(CliError::Usage(format!(
                    "unsupported autogen github preview argument '{other}'"
                )))
            }
        }
    }

    if parsed.owner.trim().is_empty() {
        return Err(CliError::Usage(
            "autogen github preview requires --owner <owner>".to_owned(),
        ));
    }
    if parsed.repo.trim().is_empty() {
        return Err(CliError::Usage(
            "autogen github preview requires --repo <repo>".to_owned(),
        ));
    }
    if parsed.android_package.trim().is_empty() {
        return Err(CliError::Usage(
            "autogen github preview requires --android-package <package-name>".to_owned(),
        ));
    }
    if parsed.releases.as_os_str().is_empty() {
        return Err(CliError::Usage(
            "autogen github preview requires --releases <fixture.json>".to_owned(),
        ));
    }

    Ok(parsed)
}

fn parse_provider_github_releases_args(
    args: &[String],
) -> Result<ProviderGithubReleasesArgs, CliError> {
    let mut parsed = ProviderGithubReleasesArgs::default();
    let mut position = 0;
    while position < args.len() {
        match args[position].as_str() {
            "--owner" => {
                parsed.owner = args
                    .get(position + 1)
                    .ok_or_else(|| {
                        CliError::Usage(
                            "provider github releases --owner requires an owner".to_owned(),
                        )
                    })?
                    .clone();
                position += 2;
            }
            "--repo" => {
                parsed.repo = args
                    .get(position + 1)
                    .ok_or_else(|| {
                        CliError::Usage(
                            "provider github releases --repo requires a repo".to_owned(),
                        )
                    })?
                    .clone();
                position += 2;
            }
            "--releases" => {
                let path = args.get(position + 1).ok_or_else(|| {
                    CliError::Usage(
                        "provider github releases --releases requires a fixture path".to_owned(),
                    )
                })?;
                parsed.releases = PathBuf::from(path);
                position += 2;
            }
            "--asset-include" => {
                parsed.asset_include = Some(
                    args.get(position + 1)
                        .ok_or_else(|| {
                            CliError::Usage(
                                "provider github releases --asset-include requires a regex"
                                    .to_owned(),
                            )
                        })?
                        .clone(),
                );
                position += 2;
            }
            "--asset-exclude" => {
                parsed.asset_exclude = Some(
                    args.get(position + 1)
                        .ok_or_else(|| {
                            CliError::Usage(
                                "provider github releases --asset-exclude requires a regex"
                                    .to_owned(),
                            )
                        })?
                        .clone(),
                );
                position += 2;
            }
            "--include-prereleases" => {
                parsed.include_prereleases = true;
                position += 1;
            }
            "--refresh" => {
                parsed.refresh = true;
                position += 1;
            }
            other => {
                return Err(CliError::Usage(format!(
                    "unsupported provider github releases argument '{other}'"
                )))
            }
        }
    }

    if parsed.owner.trim().is_empty() {
        return Err(CliError::Usage(
            "provider github releases requires --owner <owner>".to_owned(),
        ));
    }
    if parsed.repo.trim().is_empty() {
        return Err(CliError::Usage(
            "provider github releases requires --repo <repo>".to_owned(),
        ));
    }
    if parsed.releases.as_os_str().is_empty() {
        return Err(CliError::Usage(
            "provider github releases requires --releases <fixture.json>".to_owned(),
        ));
    }

    Ok(parsed)
}

fn parse_provider_github_latest_commit_args(
    args: &[String],
) -> Result<ProviderGithubLatestCommitArgs, CliError> {
    let mut parsed = ProviderGithubLatestCommitArgs::default();
    let mut position = 0;
    while position < args.len() {
        match args[position].as_str() {
            "--owner" => {
                parsed.owner = args
                    .get(position + 1)
                    .ok_or_else(|| {
                        CliError::Usage(
                            "provider github latest-commit --owner requires an owner".to_owned(),
                        )
                    })?
                    .clone();
                position += 2;
            }
            "--repo" => {
                parsed.repo = args
                    .get(position + 1)
                    .ok_or_else(|| {
                        CliError::Usage(
                            "provider github latest-commit --repo requires a repo".to_owned(),
                        )
                    })?
                    .clone();
                position += 2;
            }
            "--commit" => {
                let path = args.get(position + 1).ok_or_else(|| {
                    CliError::Usage(
                        "provider github latest-commit --commit requires a fixture path".to_owned(),
                    )
                })?;
                parsed.commit = PathBuf::from(path);
                position += 2;
            }
            "--ref" => {
                parsed.reference = Some(
                    args.get(position + 1)
                        .ok_or_else(|| {
                            CliError::Usage(
                                "provider github latest-commit --ref requires a ref".to_owned(),
                            )
                        })?
                        .clone(),
                );
                position += 2;
            }
            "--refresh" => {
                parsed.refresh = true;
                position += 1;
            }
            other => {
                return Err(CliError::Usage(format!(
                    "unsupported provider github latest-commit argument '{other}'"
                )))
            }
        }
    }

    if parsed.owner.trim().is_empty() {
        return Err(CliError::Usage(
            "provider github latest-commit requires --owner <owner>".to_owned(),
        ));
    }
    if parsed.repo.trim().is_empty() {
        return Err(CliError::Usage(
            "provider github latest-commit requires --repo <repo>".to_owned(),
        ));
    }
    if parsed.commit.as_os_str().is_empty() {
        return Err(CliError::Usage(
            "provider github latest-commit requires --commit <fixture.json>".to_owned(),
        ));
    }

    Ok(parsed)
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

fn load_package_directory_layout(
    path: &Path,
) -> Result<RepositoryPackageDirectoryLayout, CliError> {
    RepositoryPackageDirectoryLayout::load(path)
        .map_err(|source| CliError::Repository(source.to_string()))
}

fn load_repository_metadata(
    id: &RepositoryId,
    path: &Path,
    priority: Option<RepositoryPriority>,
) -> Result<RepositoryMetadata, CliError> {
    RepositoryPackageDirectoryLayout::load(path)
        .map_err(|source| CliError::Repository(source.to_string()))?;
    Ok(RepositoryMetadata {
        id: id.clone(),
        name: id.to_string(),
        priority: priority.unwrap_or_else(|| default_repository_priority(id.as_str())),
        api_version: REPO_API_VERSION_V1.to_owned(),
    })
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
    evaluate_package_in_repository(&repo, package_id)?.ok_or_else(|| {
        CliError::PackageEval(format!(
            "package '{}' was not found in repository '{}'",
            package_id, repo_id
        ))
    })
}

fn evaluate_highest_priority_package(
    db: &MainDb,
    package_id: &PackageId,
) -> Result<getter_core::ResolvedPackage, CliError> {
    for repo in db.repositories()? {
        if let Some(package) = evaluate_package_in_repository(&repo, package_id)? {
            return Ok(package);
        }
    }
    Err(CliError::PackageEval(format!(
        "package '{package_id}' was not found in any registered repository"
    )))
}

fn evaluate_repository_packages(
    repo: &StoredRepository,
) -> Result<Vec<getter_core::ResolvedPackage>, CliError> {
    let path = repo_path(repo)?;
    let layout = load_package_directory_layout(&path)?;
    layout
        .packages
        .iter()
        .map(|package| evaluate_package_directory(&repo.id, &layout, package))
        .collect()
}

fn evaluate_package_in_repository(
    repo: &StoredRepository,
    package_id: &PackageId,
) -> Result<Option<getter_core::ResolvedPackage>, CliError> {
    let path = repo_path(repo)?;
    let layout = load_package_directory_layout(&path)?;
    let Some(package) = layout.package(package_id) else {
        return Ok(None);
    };
    evaluate_package_directory(&repo.id, &layout, package).map(Some)
}

fn evaluate_package_directory(
    repo_id: &RepositoryId,
    layout: &RepositoryPackageDirectoryLayout,
    package: &getter_core::repository::PackageDirectory,
) -> Result<getter_core::ResolvedPackage, CliError> {
    let metadata = layout
        .package_metadata(package)
        .map_err(|error| CliError::PackageEval(error.to_string()))?;
    let script = layout
        .unambiguous_version_script(package)
        .map_err(|error| CliError::PackageEval(error.to_string()))?;
    evaluate_package_directory_script(repo_id, package, &metadata, script)
        .map_err(|error| CliError::PackageEval(error.to_string()))
}

fn read_fdroid_index(path: &Path) -> Result<String, CliError> {
    fs::read_to_string(path)
        .map_err(|source| CliError::Autogen(format!("failed to read F-Droid index: {source}")))
}

fn read_github_releases_fixture(path: &Path) -> Result<String, CliError> {
    fs::read_to_string(path).map_err(|source| {
        CliError::Provider(format!("failed to read GitHub releases fixture: {source}"))
    })
}

fn read_github_commit_fixture(path: &Path) -> Result<String, CliError> {
    fs::read_to_string(path).map_err(|source| {
        CliError::Provider(format!("failed to read GitHub commit fixture: {source}"))
    })
}

fn read_installed_inventory(path: &Path) -> Result<InstalledInventory, CliError> {
    let bytes = fs::read(path)
        .map_err(|source| CliError::Autogen(format!("failed to read inventory: {source}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|source| CliError::Autogen(format!("failed to parse inventory JSON: {source}")))
}

fn read_autogen_preview(path: &Path, expected_operation: &str) -> Result<Value, CliError> {
    let bytes = fs::read(path)
        .map_err(|source| CliError::Autogen(format!("failed to read autogen preview: {source}")))?;
    let raw: Value = serde_json::from_slice(&bytes).map_err(|source| {
        CliError::Autogen(format!("failed to parse autogen preview JSON: {source}"))
    })?;
    autogen::unwrap_preview_payload(raw, expected_operation).map_err(CliError::from)
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

#[derive(Debug, Deserialize)]
struct RuntimeScript {
    steps: Vec<RuntimeScriptStep>,
}

#[derive(Debug, Deserialize)]
struct RuntimeScriptStep {
    operation: String,
    #[serde(default)]
    payload: Value,
    #[serde(default)]
    plan: Option<SealedActionPlan>,
}

fn run_runtime_script(data_dir: &Path, path: &Path) -> Result<Value, CliError> {
    let bytes = fs::read(path)
        .map_err(|source| CliError::Runtime(format!("failed to read runtime script: {source}")))?;
    let script: RuntimeScript = serde_json::from_slice(&bytes).map_err(|source| {
        CliError::Runtime(format!("failed to parse runtime script JSON: {source}"))
    })?;
    let mut runtime = GetterRuntime::new();
    let mut context = RuntimeScriptContext::default();
    let mut outputs = Vec::new();
    for step in script.steps {
        let data = execute_runtime_script_step(data_dir, &mut runtime, &mut context, step)?;
        outputs.push(data);
    }
    Ok(json!({ "steps": outputs }))
}

#[derive(Default)]
struct RuntimeScriptContext {
    last_action_id: Option<String>,
    last_task_id: Option<String>,
}

fn execute_runtime_script_step(
    data_dir: &Path,
    runtime: &mut GetterRuntime,
    context: &mut RuntimeScriptContext,
    step: RuntimeScriptStep,
) -> Result<Value, CliError> {
    let operation = step.operation.as_str();
    let data = match operation {
        "issue_action" => {
            let plan = match step.plan {
                Some(plan) => plan,
                None => serde_json::from_value(step.payload).map_err(|source| {
                    CliError::Runtime(format!("failed to parse issue_action plan: {source}"))
                })?,
            };
            runtime_operations::issue_action(runtime, plan)
        }
        "update_check_package_issue_action" => {
            let db = open_main_db(data_dir)?;
            let payload =
                replace_runtime_script_tokens(context, empty_object_payload(step.payload))?;
            let payload = serde_json::to_string(&payload).map_err(|source| {
                CliError::Runtime(format!("failed to serialize runtime request: {source}"))
            })?;
            runtime_operations::issue_action_from_registered_package_json(
                runtime, data_dir, &db, &payload,
            )
            .map_err(|source| CliError::Runtime(source.to_string()))?
        }
        "submit_action" => runtime_json_operation(
            runtime,
            runtime_operations::submit_action_json,
            default_action_payload(context, step.payload)?,
        )?,
        "task_get" => runtime_json_query_operation(
            runtime,
            runtime_operations::task_get_json,
            default_task_payload(context, step.payload)?,
        )?,
        "task_list" => runtime_json_query_operation(
            runtime,
            runtime_operations::task_list_json,
            empty_object_payload(step.payload),
        )?,
        "task_start" => runtime_json_operation(
            runtime,
            runtime_operations::task_start_json,
            default_task_payload(context, step.payload)?,
        )?,
        "task_download_progress" => runtime_json_operation(
            runtime,
            runtime_operations::task_download_progress_json,
            default_task_payload(context, step.payload)?,
        )?,
        "task_complete_download" => runtime_json_operation(
            runtime,
            runtime_operations::task_complete_download_json,
            default_task_payload(context, step.payload)?,
        )?,
        "task_pause" => runtime_json_operation(
            runtime,
            runtime_operations::task_pause_json,
            default_task_payload(context, step.payload)?,
        )?,
        "task_resume" => runtime_json_operation(
            runtime,
            runtime_operations::task_resume_json,
            default_task_payload(context, step.payload)?,
        )?,
        "task_user_result" => runtime_json_operation(
            runtime,
            runtime_operations::task_user_result_json,
            default_task_payload(context, step.payload)?,
        )?,
        "task_cancel" => runtime_json_operation(
            runtime,
            runtime_operations::task_cancel_json,
            default_task_payload(context, step.payload)?,
        )?,
        "task_retry" => runtime_json_operation(
            runtime,
            runtime_operations::task_retry_json,
            default_task_payload(context, step.payload)?,
        )?,
        "task_remove" => runtime_json_operation(
            runtime,
            runtime_operations::task_remove_json,
            default_task_payload(context, step.payload)?,
        )?,
        "task_clean" => runtime_json_operation(
            runtime,
            runtime_operations::task_clean_json,
            empty_object_payload(step.payload),
        )?,
        other => {
            return Err(CliError::Runtime(format!(
                "unsupported runtime script operation '{other}'"
            )))
        }
    };
    remember_runtime_script_ids(context, &data);
    Ok(json!({ "operation": operation, "data": data }))
}

fn runtime_json_operation(
    runtime: &mut GetterRuntime,
    operation: fn(
        &mut GetterRuntime,
        &str,
    ) -> Result<Value, runtime_operations::RuntimeOperationError>,
    payload: Value,
) -> Result<Value, CliError> {
    let payload = serde_json::to_string(&payload).map_err(|source| {
        CliError::Runtime(format!("failed to serialize runtime request: {source}"))
    })?;
    operation(runtime, &payload).map_err(|source| CliError::Runtime(source.to_string()))
}

fn runtime_json_query_operation(
    runtime: &GetterRuntime,
    operation: fn(&GetterRuntime, &str) -> Result<Value, runtime_operations::RuntimeOperationError>,
    payload: Value,
) -> Result<Value, CliError> {
    let payload = serde_json::to_string(&payload).map_err(|source| {
        CliError::Runtime(format!("failed to serialize runtime request: {source}"))
    })?;
    operation(runtime, &payload).map_err(|source| CliError::Runtime(source.to_string()))
}

fn empty_object_payload(payload: Value) -> Value {
    if payload.is_null() {
        json!({})
    } else {
        payload
    }
}

fn default_action_payload(
    context: &RuntimeScriptContext,
    payload: Value,
) -> Result<Value, CliError> {
    if payload.is_null() {
        let action_id = context.last_action_id.as_ref().ok_or_else(|| {
            CliError::Runtime("runtime script has no previous action_id".to_owned())
        })?;
        return Ok(json!({ "action_id": action_id }));
    }
    replace_runtime_script_tokens(context, payload)
}

fn default_task_payload(context: &RuntimeScriptContext, payload: Value) -> Result<Value, CliError> {
    if payload.is_null() {
        let task_id = context.last_task_id.as_ref().ok_or_else(|| {
            CliError::Runtime("runtime script has no previous task_id".to_owned())
        })?;
        return Ok(json!({ "task_id": task_id }));
    }
    replace_runtime_script_tokens(context, payload)
}

fn replace_runtime_script_tokens(
    context: &RuntimeScriptContext,
    payload: Value,
) -> Result<Value, CliError> {
    match payload {
        Value::String(value) if value == "$last_action_id" => {
            Ok(Value::String(context.last_action_id.clone().ok_or_else(
                || CliError::Runtime("runtime script has no previous action_id".to_owned()),
            )?))
        }
        Value::String(value) if value == "$last_task_id" => {
            Ok(Value::String(context.last_task_id.clone().ok_or_else(
                || CliError::Runtime("runtime script has no previous task_id".to_owned()),
            )?))
        }
        Value::Array(values) => values
            .into_iter()
            .map(|value| replace_runtime_script_tokens(context, value))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(values) => values
            .into_iter()
            .map(|(key, value)| Ok((key, replace_runtime_script_tokens(context, value)?)))
            .collect::<Result<serde_json::Map<_, _>, CliError>>()
            .map(Value::Object),
        other => Ok(other),
    }
}

fn remember_runtime_script_ids(context: &mut RuntimeScriptContext, data: &Value) {
    if let Some(action_id) = data.get("action_id").and_then(Value::as_str) {
        context.last_action_id = Some(action_id.to_owned());
    }
    if let Some(task_id) = data.get("task_id").and_then(Value::as_str) {
        context.last_task_id = Some(task_id.to_owned());
    }
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
    packages.into_iter().map(tracked_package_json).collect()
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
                ignored_version: app.pin_version.clone(),
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
        "notices": [migration_pin_version_notice()],
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

fn migration_pin_version_notice() -> Value {
    json!({
        "code": "migration.renamed_ignored_version_to_pin_version",
        "message": "Legacy ignored version state was preserved as pin_version",
    })
}

fn tracked_package_upsert(
    mapping: getter_storage::legacy_room::LegacyAppMapping,
) -> TrackedPackageUpsert {
    TrackedPackageUpsert {
        package_id: mapping.package_id,
        enabled: true,
        favorite: mapping.user_state.favorite,
        pin_version: mapping.user_state.pin_version,
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
        notices: migration_report_notices(code),
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

fn migration_report_notices(code: &str) -> Vec<Value> {
    if code == "migration.imported" {
        vec![migration_pin_version_notice()]
    } else {
        Vec::new()
    }
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notices: Vec<Value>,
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
    "Usage: getter --data-dir <path> <init|app list|repo list|repo add <repo-id> <path> [--priority <n>]|repo eval <repo-id>|repo validate <path>|package eval <package-id> [--repo <repo-id>]|storage validate|version pin <package-id> <version>|version unpin <package-id>|hub list|update check --fixture <fixture.json>|runtime script --script <script.json>|debug fake-task submit --request <request.json>|debug fake-task run <task-id>|debug fake-task list|debug fake-task cancel <task-id>|debug fake-task events --after <cursor> --limit <n>|debug fake-task install-result <handoff-id> --status <accepted|succeeded|failed|canceled>|autogen installed preview --inventory <installed.json>|autogen installed apply --preview <preview.json> (--accept-all|--accept <package-id>...)|autogen fdroid preview --index <index.xml> [--package <package-name>...] [--inventory <installed.json>]|autogen fdroid apply --preview <preview.json> (--accept-all|--accept <package-id>...)|autogen github preview --owner <owner> --repo <repo> --android-package <package-name> --releases <fixture.json> [--display-name <name>] [--asset-include <regex>] [--asset-exclude <regex>] [--include-prereleases]|autogen github apply --preview <preview.json> (--accept-all|--accept <package-id>...)|provider github releases --owner <owner> --repo <repo> --releases <fixture.json> [--asset-include <regex>] [--asset-exclude <regex>] [--include-prereleases] [--refresh]|provider github latest-commit --owner <owner> --repo <repo> --commit <fixture.json> [--ref <ref>] [--refresh]|autogen cleanup preview --inventory <installed.json>|autogen cleanup apply --preview <preview.json> (--accept-all|--accept <package-id>...)|legacy import-room-bundle <bundle.json>|legacy import-room-db <db.sqlite>|legacy report-list>\nNote: `debug fake-task` commands are persisted fake-download scaffolding. ADR-0011 runtime task debugging uses `runtime script` and does not preserve task state across CLI invocations.\n".to_owned()
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
    #[serde(default, alias = "ignored_version")]
    pin_version: Option<String>,
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
            Self::VersionPin { .. } => "version pin",
            Self::VersionUnpin { .. } => "version unpin",
            Self::UpdateCheck { .. } => "update check",
            Self::DebugFakeTaskSubmit { .. } => "debug fake-task submit",
            Self::DebugFakeTaskRun { .. } => "debug fake-task run",
            Self::DebugFakeTaskList => "debug fake-task list",
            Self::DebugFakeTaskCancel { .. } => "debug fake-task cancel",
            Self::DebugFakeTaskEvents { .. } => "debug fake-task events",
            Self::DebugFakeTaskInstallResult { .. } => "debug fake-task install-result",
            Self::RuntimeScript { .. } => "runtime script",
            Self::AutogenInstalledPreview { .. } => "autogen installed preview",
            Self::AutogenInstalledApply { .. } => "autogen installed apply",
            Self::AutogenCleanupPreview { .. } => "autogen cleanup preview",
            Self::AutogenCleanupApply { .. } => "autogen cleanup apply",
            Self::AutogenFdroidPreview { .. } => "autogen fdroid preview",
            Self::AutogenFdroidApply { .. } => "autogen fdroid apply",
            Self::AutogenGithubPreview { .. } => "autogen github preview",
            Self::AutogenGithubApply { .. } => "autogen github apply",
            Self::ProviderGithubReleases { .. } => "provider github releases",
            Self::ProviderGithubLatestCommit { .. } => "provider github latest-commit",
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
    fn parses_version_pin_and_unpin_commands() {
        let pin = parse_args([
            "getter",
            "--data-dir",
            "/tmp/ua-getter",
            "version",
            "pin",
            "android/org.fdroid.fdroid",
            "1.2.3",
        ])
        .unwrap();
        assert_eq!(
            pin.command,
            CliCommand::VersionPin {
                package_id: "android/org.fdroid.fdroid".parse().unwrap(),
                version: "1.2.3".to_owned(),
            }
        );

        let unpin = parse_args([
            "getter",
            "--data-dir",
            "/tmp/ua-getter",
            "version",
            "unpin",
            "android/org.fdroid.fdroid",
        ])
        .unwrap();
        assert_eq!(
            unpin.command,
            CliCommand::VersionUnpin {
                package_id: "android/org.fdroid.fdroid".parse().unwrap(),
            }
        );
    }

    #[test]
    fn parses_provider_github_latest_commit_command() {
        let parsed = parse_args([
            "getter",
            "--data-dir",
            "/tmp/ua-getter",
            "provider",
            "github",
            "latest-commit",
            "--owner",
            "DUpdateSystem",
            "--repo",
            "UpgradeAll",
            "--commit",
            "/tmp/commit.json",
            "--ref",
            "main",
            "--refresh",
        ])
        .unwrap();

        assert_eq!(
            parsed.command,
            CliCommand::ProviderGithubLatestCommit {
                owner: "DUpdateSystem".to_owned(),
                repo: "UpgradeAll".to_owned(),
                reference: Some("main".to_owned()),
                commit: PathBuf::from("/tmp/commit.json"),
                refresh: true,
            }
        );
    }

    #[test]
    fn parses_autogen_github_preview_and_apply_commands() {
        let preview = parse_args([
            "getter",
            "--data-dir",
            "/tmp/ua-getter",
            "autogen",
            "github",
            "preview",
            "--owner",
            "DUpdateSystem",
            "--repo",
            "UpgradeAll",
            "--android-package",
            "net.xzos.upgradeall",
            "--display-name",
            "UpgradeAll",
            "--releases",
            "/tmp/releases.json",
            "--asset-include",
            "UpgradeAll_.*[.]apk$",
            "--asset-exclude",
            "debug",
            "--include-prereleases",
        ])
        .unwrap();
        assert_eq!(
            preview.command,
            CliCommand::AutogenGithubPreview {
                owner: "DUpdateSystem".to_owned(),
                repo: "UpgradeAll".to_owned(),
                android_package: "net.xzos.upgradeall".to_owned(),
                display_name: Some("UpgradeAll".to_owned()),
                releases: PathBuf::from("/tmp/releases.json"),
                asset_include: Some("UpgradeAll_.*[.]apk$".to_owned()),
                asset_exclude: Some("debug".to_owned()),
                include_prereleases: true,
            }
        );

        let apply = parse_args([
            "getter",
            "--data-dir",
            "/tmp/ua-getter",
            "autogen",
            "github",
            "apply",
            "--preview",
            "/tmp/preview.json",
            "--accept",
            "android/github/DUpdateSystem/UpgradeAll/net.xzos.upgradeall",
        ])
        .unwrap();
        assert_eq!(
            apply.command,
            CliCommand::AutogenGithubApply {
                preview: PathBuf::from("/tmp/preview.json"),
                acceptance: AutogenAcceptance::Accept(vec![
                    "android/github/DUpdateSystem/UpgradeAll/net.xzos.upgradeall"
                        .parse()
                        .unwrap(),
                ]),
            }
        );
    }

    #[test]
    fn provider_github_latest_commit_missing_cli_flags_is_usage_error() {
        let output = run([
            "getter",
            "--data-dir",
            "/tmp/ua-getter",
            "provider",
            "github",
            "latest-commit",
            "--owner",
            "DUpdateSystem",
        ]);

        assert_eq!(output.exit_code, ExitCode::Usage);
        let json: Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(json["error"]["code"], "cli.usage");
    }

    #[test]
    fn provider_github_latest_commit_malformed_fixture_is_provider_error() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("getter-data");
        let commit = temp.path().join("commit.json");
        fs::write(&commit, "not json").unwrap();

        let output = run([
            "getter".to_owned(),
            "--data-dir".to_owned(),
            data_dir.to_string_lossy().to_string(),
            "provider".to_owned(),
            "github".to_owned(),
            "latest-commit".to_owned(),
            "--owner".to_owned(),
            "DUpdateSystem".to_owned(),
            "--repo".to_owned(),
            "UpgradeAll".to_owned(),
            "--commit".to_owned(),
            commit.to_string_lossy().to_string(),
        ]);

        assert_eq!(output.exit_code, ExitCode::Provider);
        let json: Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(json["error"]["code"], "provider.error");
    }

    #[test]
    fn old_task_namespace_is_not_public_cli_surface() {
        let output = run(["getter", "--data-dir", "/tmp/ua-getter", "task", "list"]);
        assert_eq!(output.exit_code, ExitCode::Usage);
        let json: Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(json["error"]["code"], "cli.usage");
    }

    #[test]
    fn parses_runtime_script_command() {
        let parsed = parse_args([
            "getter",
            "--data-dir",
            "/tmp/ua-getter",
            "runtime",
            "script",
            "--script",
            "/tmp/runtime-script.json",
        ])
        .unwrap();
        assert_eq!(
            parsed.command,
            CliCommand::RuntimeScript {
                script: PathBuf::from("/tmp/runtime-script.json"),
            }
        );
    }

    #[test]
    fn run_runtime_script_exercises_in_memory_task_remove_and_clean() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("getter-data");
        let script = temp.path().join("runtime-script.json");
        fs::write(
            &script,
            serde_json::to_vec_pretty(&json!({
                "steps": [
                    {
                        "operation": "issue_action",
                        "plan": {
                            "package_id": "android/org.fdroid.fdroid",
                            "actions": [
                                {
                                    "type": "download",
                                    "url": "https://example.invalid/app.apk",
                                    "file_name": "app.apk"
                                }
                            ],
                            "lua_object": {
                                "object_id": "debug:android/org.fdroid.fdroid",
                                "dependency_digest": "debug-digest"
                            }
                        }
                    },
                    { "operation": "submit_action" },
                    { "operation": "task_start" },
                    {
                        "operation": "task_download_progress",
                        "payload": {
                            "task_id": "$last_task_id",
                            "current_bits": 10,
                            "total_bits": 20
                        }
                    },
                    { "operation": "task_complete_download" },
                    { "operation": "task_remove" },
                    { "operation": "task_clean", "payload": { "mode": "all_inactive" } }
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let output = run([
            "getter".to_owned(),
            "--data-dir".to_owned(),
            data_dir.to_string_lossy().to_string(),
            "runtime".to_owned(),
            "script".to_owned(),
            "--script".to_owned(),
            script.to_string_lossy().to_string(),
        ]);

        assert_eq!(output.exit_code, ExitCode::Success);
        let json: Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(json["command"], "runtime script");
        let steps = json["data"]["steps"].as_array().unwrap();
        assert_eq!(steps[0]["data"]["action_id"], "action-1");
        assert_eq!(steps[1]["data"]["task_id"], "task-1");
        assert_eq!(steps[4]["data"]["status"], "completed");
        assert_eq!(steps[5]["data"]["task_id"], "task-1");
        assert_eq!(steps[6]["data"]["tasks"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn runtime_script_does_not_preserve_tasks_across_cli_invocations() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("getter-data");
        let create_script = temp.path().join("create-runtime-task.json");
        fs::write(
            &create_script,
            serde_json::to_vec_pretty(&json!({
                "steps": [
                    {
                        "operation": "issue_action",
                        "plan": {
                            "package_id": "android/org.fdroid.fdroid",
                            "actions": [],
                            "lua_object": {
                                "object_id": "debug:android/org.fdroid.fdroid",
                                "dependency_digest": "debug-digest"
                            }
                        }
                    },
                    { "operation": "submit_action" }
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let list_script = temp.path().join("list-runtime-tasks.json");
        fs::write(
            &list_script,
            serde_json::to_vec_pretty(&json!({
                "steps": [{ "operation": "task_list" }]
            }))
            .unwrap(),
        )
        .unwrap();

        let create = run([
            "getter".to_owned(),
            "--data-dir".to_owned(),
            data_dir.to_string_lossy().to_string(),
            "runtime".to_owned(),
            "script".to_owned(),
            "--script".to_owned(),
            create_script.to_string_lossy().to_string(),
        ]);
        assert_eq!(create.exit_code, ExitCode::Success);

        let list = run([
            "getter".to_owned(),
            "--data-dir".to_owned(),
            data_dir.to_string_lossy().to_string(),
            "runtime".to_owned(),
            "script".to_owned(),
            "--script".to_owned(),
            list_script.to_string_lossy().to_string(),
        ]);
        assert_eq!(list.exit_code, ExitCode::Success);
        let json: Value = serde_json::from_str(&list.stdout).unwrap();
        assert_eq!(
            json["data"]["steps"][0]["data"]["tasks"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
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
    fn run_version_pin_and_unpin_mutates_tracked_package_pin_version() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("getter-data");

        let pin = run([
            "getter".to_owned(),
            "--data-dir".to_owned(),
            data_dir.to_string_lossy().to_string(),
            "version".to_owned(),
            "pin".to_owned(),
            "android/org.fdroid.fdroid".to_owned(),
            "1.2.3".to_owned(),
        ]);
        assert_eq!(pin.exit_code, ExitCode::Success);
        let pin_json: Value = serde_json::from_str(&pin.stdout).unwrap();
        assert_eq!(pin_json["data"]["package"]["pin_version"], "1.2.3");

        let unpin = run([
            "getter".to_owned(),
            "--data-dir".to_owned(),
            data_dir.to_string_lossy().to_string(),
            "version".to_owned(),
            "unpin".to_owned(),
            "android/org.fdroid.fdroid".to_owned(),
        ]);
        assert_eq!(unpin.exit_code, ExitCode::Success);
        let unpin_json: Value = serde_json::from_str(&unpin.stdout).unwrap();
        assert!(unpin_json["data"]["package"]["pin_version"].is_null());
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
