//! Getter-owned legacy Room migration operations.
//!
//! Platform layers may prepare/copy/checkpoint a legacy SQLite database and pass
//! its path to getter, but this module owns reading Room rows, mapping legacy app
//! records, writing tracked package state, recording migration completion, and
//! producing sanitized migration reports.

use getter_storage::legacy_room::{
    map_legacy_app, read_legacy_room_database, LegacyPackageResolution, LegacyRoomDbImport,
    LegacyRoomImportWarning, LegacyRoomReadError,
};
use getter_storage::{
    CacheDb, MainDb, MigrationRecordUpsert, StorageError, StoredPackageResolution,
    StoredTrackedPackage, TrackedPackageUpsert,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const MAIN_DB_FILE: &str = "main.db";
const CACHE_DB_FILE: &str = "cache.db";
pub const MIGRATION_REPORTS_DIR: &str = "migration-reports";
pub const LEGACY_ROOM_MIGRATION_ID: &str = "legacy-room-v17";

#[derive(Debug, thiserror::Error)]
pub enum LegacyRoomOperationError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("unsupported legacy Room database")]
    UnsupportedDb { report_path: PathBuf },
    #[error("invalid legacy Room database")]
    InvalidDb { report_path: PathBuf },
}

impl LegacyRoomOperationError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Storage(_) => "storage.error",
            Self::UnsupportedDb { .. } => "migration.unsupported_db",
            Self::InvalidDb { .. } => "migration.invalid_db",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::Storage(_) => "Getter storage operation failed",
            Self::UnsupportedDb { .. } => "Legacy Room database version is not supported",
            Self::InvalidDb { .. } => "Legacy Room database is invalid",
        }
    }

    pub fn detail(&self) -> Option<String> {
        match self {
            Self::Storage(detail) => Some(detail.clone()),
            Self::UnsupportedDb { .. } | Self::InvalidDb { .. } => None,
        }
    }

    pub fn report_path(&self) -> Option<&Path> {
        match self {
            Self::UnsupportedDb { report_path } | Self::InvalidDb { report_path } => {
                Some(report_path.as_path())
            }
            Self::Storage(_) => None,
        }
    }
}

impl From<StorageError> for LegacyRoomOperationError {
    fn from(source: StorageError) -> Self {
        Self::Storage(source.to_string())
    }
}

pub type LegacyRoomOperationResult<T> = Result<T, LegacyRoomOperationError>;

pub fn import_room_db_json(data_dir: &Path, legacy_db: &Path) -> LegacyRoomOperationResult<Value> {
    let db = open_main_db(data_dir)?;
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

    let import = read_legacy_room_database(legacy_db)
        .map_err(|source| legacy_db_import_error(data_dir, legacy_db, source))?;
    let imported_records = import.apps.len();
    let source_counts = source_counts_json(&import);
    let warnings = import_warnings_json(&import.warnings);
    if import.source_counts.app_rows > 0 && imported_records == 0 {
        let report_path = create_migration_report_with_source_counts(
            data_dir,
            legacy_db,
            "migration.invalid_db",
            "Legacy Room database has app rows but no importable app rows",
            0,
            0,
            &warnings,
            Some(&source_counts),
        )?;
        return Err(LegacyRoomOperationError::InvalidDb { report_path });
    }

    import_legacy_room_db(&db, &import)?;
    let report_path = create_migration_report_with_source_counts(
        data_dir,
        legacy_db,
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

pub fn report_list_json(data_dir: &Path) -> LegacyRoomOperationResult<Value> {
    open_initialized_storage(data_dir)?;
    Ok(json!({ "reports": list_migration_reports(data_dir)? }))
}

fn initialize_storage(data_dir: &Path) -> LegacyRoomOperationResult<()> {
    fs::create_dir_all(data_dir).map_err(|source| {
        LegacyRoomOperationError::Storage(format!("failed to create data directory: {source}"))
    })?;
    MainDb::open(data_dir.join(MAIN_DB_FILE))?;
    CacheDb::open(data_dir.join(CACHE_DB_FILE))?;
    Ok(())
}

fn open_initialized_storage(data_dir: &Path) -> LegacyRoomOperationResult<()> {
    initialize_storage(data_dir)
}

fn open_main_db(data_dir: &Path) -> LegacyRoomOperationResult<MainDb> {
    initialize_storage(data_dir)?;
    Ok(MainDb::open(data_dir.join(MAIN_DB_FILE))?)
}

fn import_legacy_room_db(
    db: &MainDb,
    import: &LegacyRoomDbImport,
) -> LegacyRoomOperationResult<()> {
    let mut packages = Vec::new();
    for app in &import.apps {
        let mapping = map_legacy_app(&app.app, Some(&app.user_state))
            .map_err(|source| LegacyRoomOperationError::Storage(source.to_string()))?;
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
) -> LegacyRoomOperationError {
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
        Ok(report_path) if unsupported => LegacyRoomOperationError::UnsupportedDb { report_path },
        Ok(report_path) => LegacyRoomOperationError::InvalidDb { report_path },
        Err(error) => error,
    }
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
) -> LegacyRoomOperationResult<PathBuf> {
    let reports_dir = data_dir.join(MIGRATION_REPORTS_DIR);
    fs::create_dir_all(&reports_dir).map_err(|source| {
        LegacyRoomOperationError::Storage(format!(
            "failed to create migration report directory: {source}"
        ))
    })?;
    let report_path = reports_dir.join(report_file_name(code));
    let report = MigrationReport {
        ok: code == "migration.imported",
        code,
        message: match code {
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
    let bytes = serde_json::to_vec_pretty(&report).map_err(|source| {
        LegacyRoomOperationError::Storage(format!("failed to serialize report: {source}"))
    })?;
    fs::write(&report_path, bytes).map_err(|source| {
        LegacyRoomOperationError::Storage(format!("failed to write report: {source}"))
    })?;
    Ok(report_path)
}

fn report_file_name(code: &str) -> String {
    format!("{}.json", code.replace('.', "-"))
}

fn list_migration_reports(data_dir: &Path) -> LegacyRoomOperationResult<Vec<Value>> {
    let reports_dir = data_dir.join(MIGRATION_REPORTS_DIR);
    if !reports_dir.exists() {
        return Ok(Vec::new());
    }

    let mut report_paths = fs::read_dir(&reports_dir)
        .map_err(|source| {
            LegacyRoomOperationError::Storage(format!("failed to read migration reports: {source}"))
        })?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| {
            LegacyRoomOperationError::Storage(format!(
                "failed to read migration report entry: {source}"
            ))
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
                LegacyRoomOperationError::Storage(format!(
                    "failed to read migration report '{}': {source}",
                    path.display()
                ))
            })?;
            let report: Value = serde_json::from_slice(&bytes).map_err(|source| {
                LegacyRoomOperationError::Storage(format!(
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn import_room_db_rejects_malformed_db_with_report() {
        let temp = temp_dir("malformed");
        fs::create_dir_all(&temp).unwrap();
        let db_path = temp.join("legacy.db");
        fs::write(&db_path, b"not sqlite").unwrap();

        let error = import_room_db_json(&temp.join("getter"), &db_path).unwrap_err();

        assert!(matches!(error, LegacyRoomOperationError::InvalidDb { .. }));
        assert_eq!(error.code(), "migration.invalid_db");
        assert!(error.report_path().is_some_and(Path::exists));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("upgradeall-legacy-room-op-{name}-{nanos}"))
    }
}
