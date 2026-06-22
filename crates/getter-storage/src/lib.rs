//! SQLite storage skeleton for getter main/cache databases.

pub mod legacy_room;

use getter_core::repository::RepositoryMetadata;
use getter_core::{PackageId, RepositoryId, RepositoryPriority};
use rusqlite::{params, Connection, Params, Transaction};
use std::path::Path;
use std::str::FromStr;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("invalid package id in database: {0}")]
    PackageId(#[from] getter_core::PackageIdError),
    #[error("invalid repository id in database: {0}")]
    RepositoryId(#[from] getter_core::RepositoryIdError),
    #[error("invalid package resolution in database: {0}")]
    PackageResolution(String),
}

pub struct MainDb {
    conn: Connection,
}

pub struct CacheDb {
    conn: Connection,
}

impl MainDb {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<(), StorageError> {
        self.conn.execute_batch(
            r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_migrations (
    id TEXT PRIMARY KEY,
    applied_at_unix INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS repositories (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    priority INTEGER NOT NULL,
    api_version TEXT NOT NULL,
    path TEXT,
    revision TEXT
);

CREATE TABLE IF NOT EXISTS tracked_packages (
    package_id TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL DEFAULT 1,
    favorite INTEGER NOT NULL DEFAULT 0,
    ignored_version TEXT,
    repository_id TEXT,
    package_resolution TEXT NOT NULL DEFAULT 'missing_package_definition',
    FOREIGN KEY(repository_id) REFERENCES repositories(id)
);

CREATE TABLE IF NOT EXISTS migration_records (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    completed_at_unix INTEGER NOT NULL DEFAULT (unixepoch()),
    report_json TEXT NOT NULL DEFAULT '{}'
);
"#,
        )?;
        self.ensure_column("tracked_packages", "ignored_version", "TEXT")?;
        self.ensure_column(
            "tracked_packages",
            "package_resolution",
            "TEXT NOT NULL DEFAULT 'missing_package_definition'",
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_migrations(id) VALUES ('main-v1')",
            [],
        )?;
        Ok(())
    }

    fn ensure_column(
        &self,
        table: &'static str,
        column: &'static str,
        definition: &'static str,
    ) -> Result<(), StorageError> {
        let columns = self.table_columns(table)?;
        if !columns.iter().any(|existing| existing == column) {
            self.conn.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {column} {definition};"
            ))?;
        }
        Ok(())
    }

    fn table_columns(&self, table: &'static str) -> Result<Vec<String>, StorageError> {
        let mut stmt = self.conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let mut columns = Vec::new();
        for row in rows {
            columns.push(row?);
        }
        Ok(columns)
    }

    pub fn upsert_repository(
        &self,
        metadata: &RepositoryMetadata,
        path: Option<&Path>,
        revision: Option<&str>,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            r#"
INSERT INTO repositories(id, name, priority, api_version, path, revision)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(id) DO UPDATE SET
    name = excluded.name,
    priority = excluded.priority,
    api_version = excluded.api_version,
    path = excluded.path,
    revision = excluded.revision
"#,
            params![
                metadata.id.as_str(),
                metadata.name,
                metadata.priority.value(),
                metadata.api_version,
                path.map(|p| p.to_string_lossy().to_string()),
                revision,
            ],
        )?;
        Ok(())
    }

    pub fn repositories(&self) -> Result<Vec<StoredRepository>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, priority, api_version, path, revision FROM repositories ORDER BY priority DESC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i32>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        let mut repositories = Vec::new();
        for row in rows {
            let (id, name, priority, api_version, path, revision) = row?;
            repositories.push(StoredRepository {
                id: RepositoryId::new(id)?,
                name,
                priority: RepositoryPriority::new(priority),
                api_version,
                path,
                revision,
            });
        }
        Ok(repositories)
    }

    pub fn upsert_tracked_package(
        &self,
        package: &TrackedPackageUpsert,
    ) -> Result<(), StorageError> {
        execute_tracked_package_upsert(&self.conn, package)?;
        Ok(())
    }

    pub fn import_tracked_packages_with_migration_record(
        &self,
        packages: &[TrackedPackageUpsert],
        record: &MigrationRecordUpsert<'_>,
    ) -> Result<(), StorageError> {
        let tx = self.conn.unchecked_transaction()?;
        for package in packages {
            execute_tracked_package_upsert(&tx, package)?;
        }
        tx.execute(
            r#"
INSERT INTO migration_records(id, source, report_json)
VALUES (?1, ?2, ?3)
ON CONFLICT(id) DO UPDATE SET
    source = excluded.source,
    completed_at_unix = unixepoch(),
    report_json = excluded.report_json
"#,
            params![record.id, record.source, record.report_json],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn tracked_packages(&self) -> Result<Vec<StoredTrackedPackage>, StorageError> {
        let mut stmt = self.conn.prepare(
            r#"
SELECT package_id, enabled, favorite, ignored_version, repository_id, package_resolution
FROM tracked_packages
ORDER BY package_id ASC
"#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let mut packages = Vec::new();
        for row in rows {
            let (package_id, enabled, favorite, ignored_version, repository_id, resolution) = row?;
            packages.push(StoredTrackedPackage {
                package_id: PackageId::from_str(&package_id)?,
                enabled: enabled != 0,
                favorite: favorite != 0,
                ignored_version,
                repository_id: repository_id.map(RepositoryId::new).transpose()?,
                package_resolution: StoredPackageResolution::from_str(&resolution)?,
            });
        }
        Ok(packages)
    }

    pub fn insert_migration_record(
        &self,
        id: &str,
        source: &str,
        report_json: &str,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            r#"
INSERT INTO migration_records(id, source, report_json)
VALUES (?1, ?2, ?3)
ON CONFLICT(id) DO UPDATE SET
    source = excluded.source,
    completed_at_unix = unixepoch(),
    report_json = excluded.report_json
"#,
            params![id, source, report_json],
        )?;
        Ok(())
    }

    pub fn migration_record_exists(&self, id: &str) -> Result<bool, StorageError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM migration_records WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;
        Ok(count != 0)
    }

    pub fn migration_records(&self) -> Result<Vec<StoredMigrationRecord>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source, report_json FROM migration_records ORDER BY completed_at_unix ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(StoredMigrationRecord {
                id: row.get(0)?,
                source: row.get(1)?,
                report_json: row.get(2)?,
            })
        })?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }
}

impl CacheDb {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<(), StorageError> {
        self.conn.execute_batch(
            r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_migrations (
    id TEXT PRIMARY KEY,
    applied_at_unix INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS evaluated_packages (
    cache_key TEXT PRIMARY KEY,
    repository_id TEXT NOT NULL,
    package_id TEXT NOT NULL,
    package_file_hash TEXT NOT NULL,
    schema_version TEXT NOT NULL,
    evaluated_json TEXT NOT NULL,
    evaluated_at_unix INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS provider_responses (
    cache_key TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    response_json TEXT NOT NULL,
    fetched_at_unix INTEGER NOT NULL DEFAULT (unixepoch())
);
"#,
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_migrations(id) VALUES ('cache-v1')",
            [],
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRepository {
    pub id: RepositoryId,
    pub name: String,
    pub priority: RepositoryPriority,
    pub api_version: String,
    pub path: Option<String>,
    pub revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedPackageUpsert {
    pub package_id: PackageId,
    pub enabled: bool,
    pub favorite: bool,
    pub ignored_version: Option<String>,
    pub repository_id: Option<RepositoryId>,
    pub package_resolution: StoredPackageResolution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTrackedPackage {
    pub package_id: PackageId,
    pub enabled: bool,
    pub favorite: bool,
    pub ignored_version: Option<String>,
    pub repository_id: Option<RepositoryId>,
    pub package_resolution: StoredPackageResolution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMigrationRecord {
    pub id: String,
    pub source: String,
    pub report_json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrationRecordUpsert<'a> {
    pub id: &'a str,
    pub source: &'a str,
    pub report_json: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredPackageResolution {
    OfficialRepositoryPackage,
    GenerateLocalPackage,
    MissingPackageDefinition,
}

impl StoredPackageResolution {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OfficialRepositoryPackage => "official_repository_package",
            Self::GenerateLocalPackage => "generate_local_package",
            Self::MissingPackageDefinition => "missing_package_definition",
        }
    }
}

impl FromStr for StoredPackageResolution {
    type Err = StorageError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "official_repository_package" => Ok(Self::OfficialRepositoryPackage),
            "generate_local_package" => Ok(Self::GenerateLocalPackage),
            "missing_package_definition" => Ok(Self::MissingPackageDefinition),
            other => Err(StorageError::PackageResolution(other.to_owned())),
        }
    }
}

trait SqlExecutor {
    fn execute_statement<P: Params>(&self, sql: &str, params: P) -> Result<usize, rusqlite::Error>;
}

impl SqlExecutor for Connection {
    fn execute_statement<P: Params>(&self, sql: &str, params: P) -> Result<usize, rusqlite::Error> {
        self.execute(sql, params)
    }
}

impl SqlExecutor for Transaction<'_> {
    fn execute_statement<P: Params>(&self, sql: &str, params: P) -> Result<usize, rusqlite::Error> {
        self.execute(sql, params)
    }
}

fn execute_tracked_package_upsert(
    conn: &impl SqlExecutor,
    package: &TrackedPackageUpsert,
) -> Result<usize, rusqlite::Error> {
    conn.execute_statement(
        r#"
INSERT INTO tracked_packages(
    package_id,
    enabled,
    favorite,
    ignored_version,
    repository_id,
    package_resolution
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(package_id) DO UPDATE SET
    enabled = excluded.enabled,
    favorite = excluded.favorite,
    ignored_version = excluded.ignored_version,
    repository_id = excluded.repository_id,
    package_resolution = excluded.package_resolution
"#,
        params![
            package.package_id.to_string(),
            bool_to_i64(package.enabled),
            bool_to_i64(package.favorite),
            package.ignored_version.as_deref(),
            package.repository_id.as_ref().map(RepositoryId::as_str),
            package.package_resolution.as_str(),
        ],
    )
}

fn bool_to_i64(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use getter_core::repository::REPO_API_VERSION_V1;

    #[test]
    fn main_db_stores_repository_registry_ordered_by_priority() {
        let db = MainDb::open_in_memory().unwrap();
        db.upsert_repository(
            &RepositoryMetadata {
                id: RepositoryId::new("official").unwrap(),
                name: "Official".to_owned(),
                priority: RepositoryPriority::DEFAULT,
                api_version: REPO_API_VERSION_V1.to_owned(),
            },
            None,
            Some("rev1"),
        )
        .unwrap();
        db.upsert_repository(
            &RepositoryMetadata {
                id: RepositoryId::new("local").unwrap(),
                name: "Local".to_owned(),
                priority: RepositoryPriority::LOCAL,
                api_version: REPO_API_VERSION_V1.to_owned(),
            },
            None,
            None,
        )
        .unwrap();

        let repositories = db.repositories().unwrap();
        assert_eq!(repositories[0].id.as_str(), "local");
        assert_eq!(repositories[1].id.as_str(), "official");
    }

    #[test]
    fn main_db_stores_tracked_package_user_state() {
        let db = MainDb::open_in_memory().unwrap();
        db.upsert_tracked_package(&TrackedPackageUpsert {
            package_id: "android/org.fdroid.fdroid".parse().unwrap(),
            enabled: true,
            favorite: true,
            ignored_version: Some("1.2.3".to_owned()),
            repository_id: None,
            package_resolution: StoredPackageResolution::OfficialRepositoryPackage,
        })
        .unwrap();

        let packages = db.tracked_packages().unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(
            packages[0].package_id.to_string(),
            "android/org.fdroid.fdroid"
        );
        assert!(packages[0].enabled);
        assert!(packages[0].favorite);
        assert_eq!(packages[0].ignored_version.as_deref(), Some("1.2.3"));
        assert_eq!(
            packages[0].package_resolution,
            StoredPackageResolution::OfficialRepositoryPackage
        );
    }

    #[test]
    fn main_db_records_migration_completion() {
        let db = MainDb::open_in_memory().unwrap();
        assert!(!db.migration_record_exists("legacy-room-v17").unwrap());
        db.insert_migration_record("legacy-room-v17", "legacy-room-bundle", r#"{"ok":true}"#)
            .unwrap();

        let records = db.migration_records().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "legacy-room-v17");
        assert_eq!(records[0].source, "legacy-room-bundle");
        assert!(db.migration_record_exists("legacy-room-v17").unwrap());
    }

    #[test]
    fn cache_db_migrates_schema() {
        let _db = CacheDb::open_in_memory().unwrap();
    }
}
