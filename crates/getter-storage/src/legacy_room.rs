//! Legacy Room migration mapping tests and pure mapping helpers.
//!
//! This module owns Rust-side interpretation of legacy Room data. Android and
//! Flutter adapters may locate/checkpoint/copy the old SQLite files, but package
//! identity, user-state mapping, warnings, and target semantics live here.

use getter_core::PackageId;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

pub const LEGACY_ROOM_SUPPORTED_VERSION: u32 = 17;
const LEGACY_ANDROID_APP_TYPE: &str = "android_app_package";
const LEGACY_ANDROID_MAGISK_MODULE_TYPE: &str = "android_magisk_module";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyAppKind {
    Android,
    Magisk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyAppRecord {
    pub kind: LegacyAppKind,
    pub installed_id: String,
    pub official_package_available: bool,
    pub common_conversion_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyExtraAppRecord {
    pub ignored_version: Option<String>,
    pub favorite: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyAppMapping {
    pub package_id: PackageId,
    pub package_resolution: LegacyPackageResolution,
    pub user_state: LegacyUserStateMapping,
    pub warnings: Vec<LegacyMigrationWarning>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyPackageResolution {
    OfficialRepositoryPackage,
    GenerateLocalPackage,
    MissingPackageDefinition,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyUserStateMapping {
    pub pin_version: Option<String>,
    pub favorite: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyMigrationWarning {
    MissingPackageDefinition { package_id: PackageId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyRoomDbImport {
    pub version: u32,
    pub source_counts: LegacyRoomSourceCounts,
    pub apps: Vec<LegacyRoomImportedApp>,
    pub warnings: Vec<LegacyRoomImportWarning>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyRoomSourceCounts {
    pub app_rows: u64,
    pub extra_app_rows: u64,
    pub hub_rows: u64,
    pub extra_hub_rows: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyRoomImportedApp {
    pub app: LegacyAppRecord,
    pub user_state: LegacyExtraAppRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyRoomImportWarning {
    SkippedApp { row_id: i64, reason: String },
    SkippedExtraApp { row_id: i64, reason: String },
    DroppedHubRows { rows: u64 },
    DroppedExtraHubRows { rows: u64 },
}

impl LegacyRoomImportWarning {
    pub fn code(&self) -> &'static str {
        match self {
            Self::SkippedApp { .. } => "legacy.skipped_app",
            Self::SkippedExtraApp { .. } => "legacy.skipped_extra_app",
            Self::DroppedHubRows { .. } => "legacy.dropped_hub_rows",
            Self::DroppedExtraHubRows { .. } => "legacy.dropped_extra_hub_rows",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::SkippedApp { row_id, reason } => {
                format!("Skipped legacy app row {row_id}: {reason}")
            }
            Self::SkippedExtraApp { row_id, reason } => {
                format!("Skipped legacy extra_app row {row_id}: {reason}")
            }
            Self::DroppedHubRows { rows } => {
                format!("Legacy hub rows are not imported in this slice: {rows}")
            }
            Self::DroppedExtraHubRows { rows } => {
                format!("Legacy extra_hub rows are not imported in this slice: {rows}")
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LegacyRoomMappingError {
    #[error("legacy installed id is empty")]
    EmptyInstalledId,
    #[error("failed to construct target package id: {0}")]
    PackageId(#[from] getter_core::PackageIdError),
}

#[derive(Debug, thiserror::Error)]
pub enum LegacyRoomReadError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("unsupported legacy Room database version {found}; expected {expected}")]
    UnsupportedVersion { found: u32, expected: u32 },
    #[error("legacy Room database is missing required table '{0}'")]
    MissingRequiredTable(&'static str),
}

pub fn map_legacy_app(
    app: &LegacyAppRecord,
    extra: Option<&LegacyExtraAppRecord>,
) -> Result<LegacyAppMapping, LegacyRoomMappingError> {
    let package_id = map_legacy_package_id(app.kind, &app.installed_id)?;
    let package_resolution = if app.official_package_available {
        LegacyPackageResolution::OfficialRepositoryPackage
    } else if app.common_conversion_available {
        LegacyPackageResolution::GenerateLocalPackage
    } else {
        LegacyPackageResolution::MissingPackageDefinition
    };
    let warnings = match package_resolution {
        LegacyPackageResolution::MissingPackageDefinition => {
            vec![LegacyMigrationWarning::MissingPackageDefinition {
                package_id: package_id.clone(),
            }]
        }
        _ => Vec::new(),
    };
    let user_state = LegacyUserStateMapping {
        pin_version: extra.and_then(|extra| extra.ignored_version.clone()),
        favorite: extra.is_some_and(|extra| extra.favorite),
    };
    Ok(LegacyAppMapping {
        package_id,
        package_resolution,
        user_state,
        warnings,
    })
}

pub fn map_legacy_package_id(
    kind: LegacyAppKind,
    installed_id: &str,
) -> Result<PackageId, LegacyRoomMappingError> {
    if installed_id.is_empty() {
        return Err(LegacyRoomMappingError::EmptyInstalledId);
    }
    let prefix = match kind {
        LegacyAppKind::Android => "android",
        LegacyAppKind::Magisk => "magisk",
    };
    Ok(format!("{prefix}/{installed_id}").parse()?)
}

pub fn read_legacy_room_database(path: &Path) -> Result<LegacyRoomDbImport, LegacyRoomReadError> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let version = legacy_user_version(&conn)?;
    if version != LEGACY_ROOM_SUPPORTED_VERSION {
        return Err(LegacyRoomReadError::UnsupportedVersion {
            found: version,
            expected: LEGACY_ROOM_SUPPORTED_VERSION,
        });
    }
    if !table_exists(&conn, "app")? {
        return Err(LegacyRoomReadError::MissingRequiredTable("app"));
    }

    let mut warnings = Vec::new();
    let extra_apps = read_extra_apps(&conn, &mut warnings)?;
    let apps = read_apps(&conn, &extra_apps, &mut warnings)?;
    let source_counts = LegacyRoomSourceCounts {
        app_rows: table_row_count(&conn, "app")?.unwrap_or(0),
        extra_app_rows: table_row_count(&conn, "extra_app")?.unwrap_or(0),
        hub_rows: table_row_count(&conn, "hub")?.unwrap_or(0),
        extra_hub_rows: table_row_count(&conn, "extra_hub")?.unwrap_or(0),
    };
    if source_counts.hub_rows > 0 {
        warnings.push(LegacyRoomImportWarning::DroppedHubRows {
            rows: source_counts.hub_rows,
        });
    }
    if source_counts.extra_hub_rows > 0 {
        warnings.push(LegacyRoomImportWarning::DroppedExtraHubRows {
            rows: source_counts.extra_hub_rows,
        });
    }

    Ok(LegacyRoomDbImport {
        version,
        source_counts,
        apps,
        warnings,
    })
}

fn legacy_user_version(conn: &Connection) -> Result<u32, LegacyRoomReadError> {
    let version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    Ok(version)
}

fn read_apps(
    conn: &Connection,
    extra_apps: &HashMap<String, LegacyExtraAppRecord>,
    warnings: &mut Vec<LegacyRoomImportWarning>,
) -> Result<Vec<LegacyRoomImportedApp>, LegacyRoomReadError> {
    let mut stmt = conn.prepare(
        r#"
SELECT id, app_id, ignore_version_number, star
FROM app
ORDER BY id ASC
"#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<i64>>(3)?,
        ))
    })?;

    let mut apps = Vec::new();
    for row in rows {
        let (row_id, app_id_json, ignored_version, star) = row?;
        let app_id = match parse_app_id_map(&app_id_json) {
            Ok(app_id) => app_id,
            Err(reason) => {
                warnings.push(LegacyRoomImportWarning::SkippedApp { row_id, reason });
                continue;
            }
        };
        let (kind, installed_id) = match installed_id_from_app_id(&app_id) {
            Ok(target) => target,
            Err(reason) => {
                warnings.push(LegacyRoomImportWarning::SkippedApp { row_id, reason });
                continue;
            }
        };
        let package_key = match map_legacy_package_id(kind, &installed_id) {
            Ok(package_id) => package_id.to_string(),
            Err(error) => {
                warnings.push(LegacyRoomImportWarning::SkippedApp {
                    row_id,
                    reason: error.to_string(),
                });
                continue;
            }
        };
        let extra = extra_apps.get(&package_key);
        apps.push(LegacyRoomImportedApp {
            app: LegacyAppRecord {
                kind,
                installed_id,
                official_package_available: false,
                common_conversion_available: false,
            },
            user_state: LegacyExtraAppRecord {
                ignored_version: extra
                    .and_then(|extra| extra.ignored_version.clone())
                    .or(ignored_version),
                favorite: star.unwrap_or(0) != 0,
            },
        });
    }
    Ok(apps)
}

fn read_extra_apps(
    conn: &Connection,
    warnings: &mut Vec<LegacyRoomImportWarning>,
) -> Result<HashMap<String, LegacyExtraAppRecord>, LegacyRoomReadError> {
    if !table_exists(conn, "extra_app")? {
        return Ok(HashMap::new());
    }
    let mut stmt = conn.prepare(
        r#"
SELECT id, app_id, mark_version_number
FROM extra_app
ORDER BY id ASC
"#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;

    let mut extras = HashMap::new();
    for row in rows {
        let (row_id, app_id_json, ignored_version) = row?;
        let app_id = match parse_app_id_map(&app_id_json) {
            Ok(app_id) => app_id,
            Err(reason) => {
                warnings.push(LegacyRoomImportWarning::SkippedExtraApp { row_id, reason });
                continue;
            }
        };
        let (kind, installed_id) = match installed_id_from_app_id(&app_id) {
            Ok(target) => target,
            Err(reason) => {
                warnings.push(LegacyRoomImportWarning::SkippedExtraApp { row_id, reason });
                continue;
            }
        };
        let package_id = match map_legacy_package_id(kind, &installed_id) {
            Ok(package_id) => package_id,
            Err(error) => {
                warnings.push(LegacyRoomImportWarning::SkippedExtraApp {
                    row_id,
                    reason: error.to_string(),
                });
                continue;
            }
        };
        extras.insert(
            package_id.to_string(),
            LegacyExtraAppRecord {
                ignored_version,
                favorite: false,
            },
        );
    }
    Ok(extras)
}

fn parse_app_id_map(value: &str) -> Result<HashMap<String, String>, String> {
    let value: Value = serde_json::from_str(value).map_err(|error| error.to_string())?;
    let object = value
        .as_object()
        .ok_or_else(|| "app_id must be a JSON object".to_owned())?;
    let mut app_id = HashMap::new();
    for (key, value) in object {
        if let Some(value) = value.as_str().filter(|value| !value.is_empty()) {
            app_id.insert(key.clone(), value.to_owned());
        }
    }
    Ok(app_id)
}

fn installed_id_from_app_id(
    app_id: &HashMap<String, String>,
) -> Result<(LegacyAppKind, String), String> {
    if let Some(module_id) = app_id.get(LEGACY_ANDROID_MAGISK_MODULE_TYPE) {
        return Ok((LegacyAppKind::Magisk, module_id.clone()));
    }
    if let Some(package_name) = app_id.get(LEGACY_ANDROID_APP_TYPE) {
        return Ok((LegacyAppKind::Android, package_name.clone()));
    }
    Err(format!(
        "app_id has no supported '{}' or '{}' value",
        LEGACY_ANDROID_APP_TYPE, LEGACY_ANDROID_MAGISK_MODULE_TYPE
    ))
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, LegacyRoomReadError> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get(0),
    )?;
    Ok(exists != 0)
}

fn table_row_count(conn: &Connection, table: &str) -> Result<Option<u64>, LegacyRoomReadError> {
    if !table_exists(conn, table)? {
        return Ok(None);
    }
    let count: u64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })?;
    Ok(Some(count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_legacy_app_maps_to_readable_android_package_id() {
        let mapping = map_legacy_app(
            &LegacyAppRecord {
                kind: LegacyAppKind::Android,
                installed_id: "org.fdroid.fdroid".to_owned(),
                official_package_available: true,
                common_conversion_available: false,
            },
            None,
        )
        .unwrap();

        assert_eq!(mapping.package_id.to_string(), "android/org.fdroid.fdroid");
        assert_eq!(
            mapping.package_resolution,
            LegacyPackageResolution::OfficialRepositoryPackage
        );
        assert!(mapping.warnings.is_empty());
    }

    #[test]
    fn magisk_legacy_app_maps_to_readable_magisk_package_id() {
        let mapping = map_legacy_app(
            &LegacyAppRecord {
                kind: LegacyAppKind::Magisk,
                installed_id: "zygisk-next".to_owned(),
                official_package_available: true,
                common_conversion_available: false,
            },
            None,
        )
        .unwrap();

        assert_eq!(mapping.package_id.to_string(), "magisk/zygisk-next");
    }

    #[test]
    fn common_unofficial_conversion_generates_local_package() {
        let mapping = map_legacy_app(
            &LegacyAppRecord {
                kind: LegacyAppKind::Android,
                installed_id: "com.example.private".to_owned(),
                official_package_available: false,
                common_conversion_available: true,
            },
            None,
        )
        .unwrap();

        assert_eq!(
            mapping.package_resolution,
            LegacyPackageResolution::GenerateLocalPackage
        );
        assert!(mapping.warnings.is_empty());
    }

    #[test]
    fn unmapped_complex_app_preserves_id_and_records_missing_definition_warning() {
        let mapping = map_legacy_app(
            &LegacyAppRecord {
                kind: LegacyAppKind::Android,
                installed_id: "com.example.unmapped".to_owned(),
                official_package_available: false,
                common_conversion_available: false,
            },
            None,
        )
        .unwrap();

        assert_eq!(
            mapping.package_resolution,
            LegacyPackageResolution::MissingPackageDefinition
        );
        assert_eq!(
            mapping.warnings,
            vec![LegacyMigrationWarning::MissingPackageDefinition {
                package_id: "android/com.example.unmapped".parse().unwrap(),
            }]
        );
    }

    #[test]
    fn extra_app_ignore_and_favorite_state_are_preserved_when_present() {
        let mapping = map_legacy_app(
            &LegacyAppRecord {
                kind: LegacyAppKind::Android,
                installed_id: "org.fdroid.fdroid".to_owned(),
                official_package_available: true,
                common_conversion_available: false,
            },
            Some(&LegacyExtraAppRecord {
                ignored_version: Some("1.2.3".to_owned()),
                favorite: true,
            }),
        )
        .unwrap();

        assert_eq!(mapping.user_state.pin_version.as_deref(), Some("1.2.3"));
        assert!(mapping.user_state.favorite);
    }

    #[test]
    fn direct_room_reader_imports_app_rows_and_extra_app_state() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("legacy.db");
        create_fixture_legacy_db(&db_path, LEGACY_ROOM_SUPPORTED_VERSION);

        let import = read_legacy_room_database(&db_path).unwrap();

        assert_eq!(import.version, LEGACY_ROOM_SUPPORTED_VERSION);
        assert_eq!(import.apps.len(), 1);
        assert_eq!(import.source_counts.app_rows, 1);
        assert_eq!(import.source_counts.extra_app_rows, 1);
        assert_eq!(import.source_counts.hub_rows, 1);
        assert_eq!(import.source_counts.extra_hub_rows, 1);
        assert_eq!(import.apps[0].app.kind, LegacyAppKind::Android);
        assert_eq!(import.apps[0].app.installed_id, "org.fdroid.fdroid");
        assert_eq!(
            import.apps[0].user_state.ignored_version.as_deref(),
            Some("1.20.0")
        );
        assert!(import.apps[0].user_state.favorite);
        assert!(import
            .warnings
            .iter()
            .any(|warning| matches!(warning, LegacyRoomImportWarning::DroppedHubRows { rows: 1 })));
        assert!(import.warnings.iter().any(|warning| matches!(
            warning,
            LegacyRoomImportWarning::DroppedExtraHubRows { rows: 1 }
        )));
    }

    #[test]
    fn direct_room_reader_maps_magisk_app_id_rows() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("legacy.db");
        create_fixture_legacy_db(&db_path, LEGACY_ROOM_SUPPORTED_VERSION);
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("DELETE FROM app", []).unwrap();
        conn.execute("DELETE FROM extra_app", []).unwrap();
        conn.execute(
            "INSERT INTO app(id, name, app_id, ignore_version_number, star) VALUES (1, 'Zygisk Next', ?1, NULL, 0)",
            [r#"{"android_magisk_module":"zygisk-next"}"#],
        )
        .unwrap();
        drop(conn);

        let import = read_legacy_room_database(&db_path).unwrap();

        assert_eq!(import.apps.len(), 1);
        assert_eq!(import.apps[0].app.kind, LegacyAppKind::Magisk);
        assert_eq!(import.apps[0].app.installed_id, "zygisk-next");
        let mapping =
            map_legacy_app(&import.apps[0].app, Some(&import.apps[0].user_state)).unwrap();
        assert_eq!(mapping.package_id.to_string(), "magisk/zygisk-next");
    }

    #[test]
    fn direct_room_reader_skips_invalid_app_rows_without_dropping_valid_apps() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("legacy.db");
        create_fixture_legacy_db(&db_path, LEGACY_ROOM_SUPPORTED_VERSION);
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO app(id, name, app_id, ignore_version_number, star) VALUES (2, 'Broken', 'not-json', NULL, 0)",
            [],
        )
        .unwrap();
        drop(conn);

        let import = read_legacy_room_database(&db_path).unwrap();

        assert_eq!(import.source_counts.app_rows, 2);
        assert_eq!(import.apps.len(), 1);
        assert_eq!(import.apps[0].app.installed_id, "org.fdroid.fdroid");
        assert!(import.warnings.iter().any(|warning| matches!(
            warning,
            LegacyRoomImportWarning::SkippedApp { row_id: 2, .. }
        )));
    }

    #[test]
    fn direct_room_reader_skips_malformed_optional_extra_app_rows() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("legacy.db");
        create_fixture_legacy_db(&db_path, LEGACY_ROOM_SUPPORTED_VERSION);
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO extra_app(id, app_id, mark_version_number) VALUES (2, 'not-json', '9.9.9')",
            [],
        )
        .unwrap();
        drop(conn);

        let import = read_legacy_room_database(&db_path).unwrap();

        assert_eq!(import.apps.len(), 1);
        assert!(import.warnings.iter().any(|warning| {
            matches!(
                warning,
                LegacyRoomImportWarning::SkippedExtraApp { row_id: 2, .. }
            )
        }));
    }

    #[test]
    fn direct_room_reader_rejects_unsupported_schema_version() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("legacy.db");
        create_fixture_legacy_db(&db_path, 16);

        let error = read_legacy_room_database(&db_path).unwrap_err();

        assert!(matches!(
            error,
            LegacyRoomReadError::UnsupportedVersion {
                found: 16,
                expected: LEGACY_ROOM_SUPPORTED_VERSION,
            }
        ));
    }

    fn create_fixture_legacy_db(path: &Path, version: u32) {
        let conn = Connection::open(path).unwrap();
        conn.pragma_update(None, "user_version", version).unwrap();
        conn.execute_batch(
            r#"
CREATE TABLE app (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    app_id TEXT NOT NULL,
    invalid_version_number_field_regex TEXT,
    include_version_number_field_regex TEXT,
    ignore_version_number TEXT,
    cloud_config TEXT,
    enable_hub_list TEXT,
    star INTEGER
);
CREATE TABLE extra_app (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    app_id TEXT NOT NULL,
    mark_version_number TEXT
);
CREATE TABLE hub (
    uuid TEXT PRIMARY KEY,
    hub_config TEXT NOT NULL,
    auth TEXT NOT NULL,
    ignore_app_id_list TEXT NOT NULL,
    applications_mode INTEGER NOT NULL DEFAULT 0,
    user_ignore_app_id_list TEXT NOT NULL,
    sort_point INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE extra_hub (
    id TEXT PRIMARY KEY,
    enable_global INTEGER NOT NULL DEFAULT 0,
    url_replace_search TEXT,
    url_replace_string TEXT
);
"#,
        )
        .unwrap();
        let app_id = r#"{"android_app_package":"org.fdroid.fdroid"}"#;
        conn.execute(
            "INSERT INTO app(id, name, app_id, ignore_version_number, star) VALUES (1, 'F-Droid', ?1, '1.10.0', 1)",
            [app_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO extra_app(id, app_id, mark_version_number) VALUES (1, ?1, '1.20.0')",
            [app_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO hub(uuid, hub_config, auth, ignore_app_id_list, user_ignore_app_id_list) VALUES ('legacy-hub', '{}', '{}', '[]', '[]')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO extra_hub(id, enable_global, url_replace_search, url_replace_string) VALUES ('GLOBAL', 1, 'http://', 'https://')",
            [],
        )
        .unwrap();
    }
}
