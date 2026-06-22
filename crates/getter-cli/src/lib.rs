//! User-facing getter CLI implementation.
//!
//! The CLI is intentionally thin: it owns command parsing and JSON envelopes,
//! while durable state is initialized through `getter-storage` so the command
//! surface exercises the same Rust-owned SQLite direction used by embedders.

use getter_core::lua::evaluate_package_file;
use getter_core::repository::{RepositoryLayout, RepositoryMetadata};
use getter_core::{PackageId, RepositoryId, RepositoryPriority};
use getter_storage::legacy_room::{
    map_legacy_app, LegacyAppKind, LegacyAppRecord, LegacyExtraAppRecord, LegacyPackageResolution,
};
use getter_storage::{
    CacheDb, MainDb, StorageError, StoredPackageResolution, StoredRepository, StoredTrackedPackage,
    TrackedPackageUpsert,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const MAIN_DB_FILE: &str = "main.db";
const CACHE_DB_FILE: &str = "cache.db";
const MIGRATION_REPORTS_DIR: &str = "migration-reports";

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
    PackageEval {
        package_id: PackageId,
        repo_id: Option<RepositoryId>,
    },
    StorageValidate,
    LegacyImportRoomBundle {
        bundle: PathBuf,
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
    #[error("Legacy Room export bundle is invalid")]
    InvalidLegacyBundle { report_path: PathBuf },
    #[error("Legacy Room bundle import is not implemented yet")]
    UnsupportedLegacyBundle { report_path: PathBuf },
}

impl CliError {
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Usage(_) => ExitCode::Usage,
            Self::Storage(_) => ExitCode::Storage,
            Self::Repository(_) | Self::PackageEval(_) => ExitCode::GenericFailure,
            Self::InvalidLegacyBundle { .. } | Self::UnsupportedLegacyBundle { .. } => {
                ExitCode::Migration
            }
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::Usage(_) => "cli.usage",
            Self::Storage(_) => "storage.error",
            Self::Repository(_) => "repository.error",
            Self::PackageEval(_) => "package.eval_error",
            Self::InvalidLegacyBundle { .. } => "migration.invalid_bundle",
            Self::UnsupportedLegacyBundle { .. } => "migration.unsupported_bundle",
        }
    }

    fn message(&self) -> &'static str {
        match self {
            Self::Usage(_) => "Invalid getter CLI usage",
            Self::Storage(_) => "Getter storage operation failed",
            Self::Repository(_) => "Getter repository operation failed",
            Self::PackageEval(_) => "Getter package evaluation failed",
            Self::InvalidLegacyBundle { .. } => "Legacy Room export bundle is invalid",
            Self::UnsupportedLegacyBundle { .. } => {
                "Legacy Room bundle import is not implemented yet"
            }
        }
    }

    fn detail(&self) -> Option<&str> {
        match self {
            Self::Usage(detail)
            | Self::Storage(detail)
            | Self::Repository(detail)
            | Self::PackageEval(detail) => Some(detail.as_str()),
            Self::InvalidLegacyBundle { .. } | Self::UnsupportedLegacyBundle { .. } => None,
        }
    }

    fn report_path(&self) -> Option<&Path> {
        match self {
            Self::InvalidLegacyBundle { report_path }
            | Self::UnsupportedLegacyBundle { report_path } => Some(report_path.as_path()),
            Self::Usage(_) | Self::Storage(_) | Self::Repository(_) | Self::PackageEval(_) => None,
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
        [domain, command, bundle] if domain == "legacy" && command == "import-room-bundle" => {
            CliCommand::LegacyImportRoomBundle {
                bundle: PathBuf::from(bundle),
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
        CliCommand::LegacyImportRoomBundle { bundle } => {
            let db = open_main_db(&invocation.data_dir)?;
            let bytes = fs::read(&bundle).map_err(|source| {
                match create_migration_report(
                    &invocation.data_dir,
                    &bundle,
                    "migration.invalid_bundle",
                    &format!("failed to read bundle: {source}"),
                    0,
                    0,
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
            )?;
            let records = db.tracked_packages()?;
            Ok(json!({
                "report_path": report_path,
                "imported_records": parsed.apps.len(),
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

fn parse_priority(value: &str) -> Result<RepositoryPriority, CliError> {
    value
        .parse::<i32>()
        .map(RepositoryPriority::new)
        .map_err(|source| CliError::Usage(format!("invalid repository priority: {source}")))
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
    for app in &bundle.apps {
        let mapping = map_legacy_app(
            &LegacyAppRecord {
                kind: app.kind.to_legacy_kind()?,
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
        db.upsert_tracked_package(&TrackedPackageUpsert {
            package_id: mapping.package_id,
            enabled: true,
            favorite: mapping.user_state.favorite,
            ignored_version: mapping.user_state.ignored_version,
            repository_id: None,
            package_resolution: stored_resolution(mapping.package_resolution),
        })?;
    }

    let report_json = json!({
        "ok": true,
        "source": "legacy-room-bundle",
        "imported_records": bundle.apps.len(),
    })
    .to_string();
    db.insert_migration_record("legacy-room-v17", "legacy-room-bundle", &report_json)?;
    Ok(())
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
    bundle: &Path,
    code: &str,
    detail: &str,
    imported_records: u64,
    tracked_records: u64,
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
            "migration.imported" => "Legacy Room bundle imported",
            _ => "Legacy migration failed",
        },
        bundle_file_name: bundle.file_name().and_then(|name| name.to_str()),
        detail,
        imported_records,
        tracked_records,
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
                "bundle_file_name": report.get("bundle_file_name").and_then(Value::as_str),
                "imported_records": report.get("imported_records").and_then(Value::as_u64).unwrap_or(0),
                "tracked_records": report.get("tracked_records").and_then(Value::as_u64).unwrap_or(0),
            }))
        })
        .collect()
}

#[derive(Debug, Serialize)]
struct MigrationReport<'a> {
    ok: bool,
    code: &'a str,
    message: &'a str,
    bundle_file_name: Option<&'a str>,
    detail: &'a str,
    imported_records: u64,
    tracked_records: u64,
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
    "Usage: getter --data-dir <path> <init|app list|repo list|repo add <repo-id> <path> [--priority <n>]|repo eval <repo-id>|package eval <package-id> [--repo <repo-id>]|storage validate|hub list|legacy import-room-bundle <bundle.json>>\n".to_owned()
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
    fn to_legacy_kind(&self) -> Result<LegacyAppKind, CliError> {
        Ok(match self {
            Self::Android => LegacyAppKind::Android,
            Self::Magisk => LegacyAppKind::Magisk,
        })
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
            Self::PackageEval { .. } => "package eval",
            Self::StorageValidate => "storage validate",
            Self::LegacyImportRoomBundle { .. } => "legacy import-room-bundle",
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
