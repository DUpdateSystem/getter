//! SQLite storage skeleton for getter main/cache databases.

pub mod legacy_room;

use getter_core::repository::RepositoryMetadata;
use getter_core::task::{
    DownloadTaskRequest, DownloadTaskStatus, DownloadTaskSummary, InstallHandoffStatus,
    InstallHandoffSummary, TaskCancelResult, TaskEvent, TaskEventKind, TaskEventPage,
    TaskModelError,
};
use getter_core::{PackageId, RepositoryId, RepositoryPriority, UpdateAction};
use rusqlite::{params, Connection, OptionalExtension, Params, Transaction};
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
    #[error("invalid task state in database: {0}")]
    TaskState(String),
    #[error("download task not found: {0}")]
    TaskNotFound(String),
    #[error("install handoff not found: {0}")]
    InstallHandoffNotFound(String),
    #[error("invalid install handoff transition for {handoff_id}: {reason}")]
    InvalidInstallHandoffTransition { handoff_id: String, reason: String },
    #[error("invalid task transition for {task_id}: {reason}")]
    InvalidTaskTransition { task_id: String, reason: String },
    #[error("invalid task request: {0}")]
    InvalidTaskRequest(String),
    #[error("storage invariant failed: {0}")]
    Invariant(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
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
    pin_version TEXT,
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

CREATE TABLE IF NOT EXISTS download_tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT UNIQUE,
    package_id TEXT NOT NULL,
    status TEXT NOT NULL,
    executor TEXT NOT NULL,
    actions_json TEXT NOT NULL,
    download_file_name TEXT NOT NULL,
    downloaded_file TEXT,
    failure_message TEXT,
    install_handoff_id TEXT,
    created_at_unix INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at_unix INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS task_events (
    cursor INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    status TEXT,
    message TEXT,
    created_at_unix INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS install_handoffs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    handoff_id TEXT UNIQUE,
    task_id TEXT NOT NULL,
    package_id TEXT NOT NULL,
    installer TEXT NOT NULL,
    file TEXT NOT NULL,
    status TEXT NOT NULL,
    message TEXT,
    created_at_unix INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at_unix INTEGER NOT NULL DEFAULT (unixepoch())
);
"#,
        )?;
        self.ensure_column("tracked_packages", "pin_version", "TEXT")?;
        self.migrate_ignored_version_to_pin_version()?;
        self.ensure_column(
            "tracked_packages",
            "package_resolution",
            "TEXT NOT NULL DEFAULT 'missing_package_definition'",
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_migrations(id) VALUES ('main-v1')",
            [],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_migrations(id) VALUES ('main-task-v1')",
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

    fn migrate_ignored_version_to_pin_version(&self) -> Result<(), StorageError> {
        let columns = self.table_columns("tracked_packages")?;
        if columns.iter().any(|existing| existing == "ignored_version") {
            self.conn.execute(
                r#"
UPDATE tracked_packages
SET pin_version = ignored_version
WHERE pin_version IS NULL
  AND ignored_version IS NOT NULL
"#,
                [],
            )?;
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

    pub fn set_tracked_package_pin_version(
        &self,
        package_id: &PackageId,
        pin_version: Option<&str>,
    ) -> Result<StoredTrackedPackage, StorageError> {
        self.conn.execute(
            r#"
INSERT INTO tracked_packages(
    package_id,
    enabled,
    favorite,
    pin_version,
    repository_id,
    package_resolution
)
VALUES (?1, 1, 0, ?2, NULL, ?3)
ON CONFLICT(package_id) DO UPDATE SET
    pin_version = excluded.pin_version
"#,
            params![
                package_id.to_string(),
                pin_version,
                StoredPackageResolution::MissingPackageDefinition.as_str(),
            ],
        )?;
        self.tracked_package(package_id)?.ok_or_else(|| {
            StorageError::Invariant("tracked package upsert did not create a row".to_owned())
        })
    }

    pub fn tracked_package(
        &self,
        package_id: &PackageId,
    ) -> Result<Option<StoredTrackedPackage>, StorageError> {
        Ok(self
            .tracked_packages()?
            .into_iter()
            .find(|package| &package.package_id == package_id))
    }

    pub fn upsert_generated_tracked_package_preserving_user_state(
        &self,
        package_id: &PackageId,
        repository_id: &RepositoryId,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            r#"
INSERT INTO tracked_packages(
    package_id,
    enabled,
    favorite,
    pin_version,
    repository_id,
    package_resolution
)
VALUES (?1, 1, 0, NULL, ?2, ?3)
ON CONFLICT(package_id) DO UPDATE SET
    repository_id = CASE
        WHEN tracked_packages.package_resolution = 'missing_package_definition'
        THEN excluded.repository_id
        ELSE tracked_packages.repository_id
    END,
    package_resolution = CASE
        WHEN tracked_packages.package_resolution = 'missing_package_definition'
        THEN excluded.package_resolution
        ELSE tracked_packages.package_resolution
    END
"#,
            params![
                package_id.to_string(),
                repository_id.as_str(),
                StoredPackageResolution::GenerateLocalPackage.as_str(),
            ],
        )?;
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

    pub fn delete_generated_tracked_package(
        &self,
        package_id: &PackageId,
        repository_id: &RepositoryId,
    ) -> Result<bool, StorageError> {
        let deleted = self.conn.execute(
            r#"
DELETE FROM tracked_packages
WHERE package_id = ?1
  AND repository_id = ?2
  AND package_resolution = ?3
"#,
            params![
                package_id.to_string(),
                repository_id.as_str(),
                StoredPackageResolution::GenerateLocalPackage.as_str(),
            ],
        )?;
        Ok(deleted != 0)
    }

    pub fn tracked_packages(&self) -> Result<Vec<StoredTrackedPackage>, StorageError> {
        let mut stmt = self.conn.prepare(
            r#"
SELECT package_id, enabled, favorite, pin_version, repository_id, package_resolution
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
            let (package_id, enabled, favorite, pin_version, repository_id, resolution) = row?;
            packages.push(StoredTrackedPackage {
                package_id: PackageId::from_str(&package_id)?,
                enabled: enabled != 0,
                favorite: favorite != 0,
                pin_version,
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

    pub fn create_download_task(
        &self,
        request: &DownloadTaskRequest,
    ) -> Result<DownloadTaskSummary, StorageError> {
        let download_file_name = match request.download_action() {
            Some(UpdateAction::Download { file_name, .. }) => file_name.clone(),
            _ => {
                return Err(StorageError::InvalidTaskRequest(
                    "download task request must include a download action".to_owned(),
                ))
            }
        };
        let actions_json = serde_json::to_string(&request.actions)?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            r#"
INSERT INTO download_tasks(package_id, status, executor, actions_json, download_file_name)
VALUES (?1, ?2, ?3, ?4, ?5)
"#,
            params![
                request.package_id.to_string(),
                DownloadTaskStatus::Queued.as_str(),
                request.executor.as_str(),
                actions_json,
                download_file_name,
            ],
        )?;
        let row_id = tx.last_insert_rowid();
        let task_id = format!("task-{row_id}");
        tx.execute(
            "UPDATE download_tasks SET task_id = ?1 WHERE id = ?2",
            params![task_id, row_id],
        )?;
        insert_task_event_with_executor(
            &tx,
            &task_id,
            TaskEventKind::TaskCreated,
            Some(DownloadTaskStatus::Queued),
            Some("Task queued"),
        )?;
        tx.commit()?;
        self.download_task(&task_id)
    }

    pub fn download_task(&self, task_id: &str) -> Result<DownloadTaskSummary, StorageError> {
        let mut stmt = self.conn.prepare(
            r#"
SELECT task_id, package_id, status, executor, actions_json, download_file_name,
       downloaded_file, failure_message, install_handoff_id
FROM download_tasks
WHERE task_id = ?1
"#,
        )?;
        let row = stmt
            .query_row([task_id], |row| {
                Ok(DownloadTaskRow {
                    task_id: row.get(0)?,
                    package_id: row.get(1)?,
                    status: row.get(2)?,
                    executor: row.get(3)?,
                    actions_json: row.get(4)?,
                    download_file_name: row.get(5)?,
                    downloaded_file: row.get(6)?,
                    failure_message: row.get(7)?,
                    install_handoff_id: row.get(8)?,
                })
            })
            .optional()?;
        row.map(download_task_from_row)
            .transpose()?
            .ok_or_else(|| StorageError::TaskNotFound(task_id.to_owned()))
    }

    pub fn download_tasks(&self) -> Result<Vec<DownloadTaskSummary>, StorageError> {
        let mut stmt = self.conn.prepare(
            r#"
SELECT task_id, package_id, status, executor, actions_json, download_file_name,
       downloaded_file, failure_message, install_handoff_id
FROM download_tasks
ORDER BY id ASC
"#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(DownloadTaskRow {
                task_id: row.get(0)?,
                package_id: row.get(1)?,
                status: row.get(2)?,
                executor: row.get(3)?,
                actions_json: row.get(4)?,
                download_file_name: row.get(5)?,
                downloaded_file: row.get(6)?,
                failure_message: row.get(7)?,
                install_handoff_id: row.get(8)?,
            })
        })?;
        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(download_task_from_row(row?)?);
        }
        Ok(tasks)
    }

    pub fn start_download_task(&self, task_id: &str) -> Result<DownloadTaskSummary, StorageError> {
        let task = self.download_task(task_id)?;
        match task.status {
            DownloadTaskStatus::Queued => {
                self.update_download_task_status(
                    task_id,
                    DownloadTaskStatus::Running,
                    None,
                    None,
                    TaskEventKind::TaskStarted,
                    "Task started",
                )?;
                self.download_task(task_id)
            }
            DownloadTaskStatus::Running => Ok(task),
            status => Err(StorageError::InvalidTaskTransition {
                task_id: task_id.to_owned(),
                reason: format!("cannot start task with status {}", status.as_str()),
            }),
        }
    }

    pub fn succeed_download_task(
        &self,
        task_id: &str,
    ) -> Result<DownloadTaskSummary, StorageError> {
        let task = self.download_task(task_id)?;
        match task.status {
            DownloadTaskStatus::Queued | DownloadTaskStatus::Running => {
                self.update_download_task_status(
                    task_id,
                    DownloadTaskStatus::Succeeded,
                    Some(&task.download_file_name),
                    None,
                    TaskEventKind::TaskSucceeded,
                    "Task succeeded",
                )?;
                self.download_task(task_id)
            }
            status => Err(StorageError::InvalidTaskTransition {
                task_id: task_id.to_owned(),
                reason: format!("cannot succeed task with status {}", status.as_str()),
            }),
        }
    }

    pub fn cancel_download_task(&self, task_id: &str) -> Result<TaskCancelResult, StorageError> {
        let task = self.download_task(task_id)?;
        match task.status {
            DownloadTaskStatus::Queued | DownloadTaskStatus::Running => {
                self.update_download_task_status(
                    task_id,
                    DownloadTaskStatus::Canceled,
                    None,
                    None,
                    TaskEventKind::TaskCanceled,
                    "Task canceled",
                )?;
                Ok(TaskCancelResult {
                    task_id: task_id.to_owned(),
                    status: DownloadTaskStatus::Canceled,
                    changed: true,
                })
            }
            DownloadTaskStatus::Canceled => Ok(TaskCancelResult {
                task_id: task_id.to_owned(),
                status: DownloadTaskStatus::Canceled,
                changed: false,
            }),
            status => Err(StorageError::InvalidTaskTransition {
                task_id: task_id.to_owned(),
                reason: format!("cannot cancel task with status {}", status.as_str()),
            }),
        }
    }

    pub fn create_install_handoff_for_task(
        &self,
        task_id: &str,
    ) -> Result<Option<InstallHandoffSummary>, StorageError> {
        let task = self.download_task(task_id)?;
        if task.status != DownloadTaskStatus::Succeeded {
            return Err(StorageError::InvalidTaskTransition {
                task_id: task_id.to_owned(),
                reason: format!(
                    "cannot request install handoff for task with status {}",
                    task.status.as_str()
                ),
            });
        }
        if let Some(existing) = task.install_handoff_id.as_deref() {
            return Ok(Some(self.install_handoff(existing)?));
        }
        let Some(UpdateAction::Install { installer, file }) = task
            .actions
            .iter()
            .find(|action| matches!(action, UpdateAction::Install { .. }))
        else {
            return Ok(None);
        };
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            r#"
INSERT INTO install_handoffs(task_id, package_id, installer, file, status)
VALUES (?1, ?2, ?3, ?4, ?5)
"#,
            params![
                task_id,
                task.package_id.to_string(),
                installer,
                file,
                InstallHandoffStatus::Requested.as_str(),
            ],
        )?;
        let row_id = tx.last_insert_rowid();
        let handoff_id = format!("handoff-{row_id}");
        tx.execute(
            "UPDATE install_handoffs SET handoff_id = ?1 WHERE id = ?2",
            params![handoff_id, row_id],
        )?;
        tx.execute(
            "UPDATE download_tasks SET install_handoff_id = ?1, updated_at_unix = unixepoch() WHERE task_id = ?2",
            params![handoff_id, task_id],
        )?;
        insert_task_event_with_executor(
            &tx,
            task_id,
            TaskEventKind::InstallHandoffRequested,
            Some(task.status),
            Some("Install handoff requested"),
        )?;
        tx.commit()?;
        Ok(Some(self.install_handoff(&handoff_id)?))
    }

    pub fn install_handoff(&self, handoff_id: &str) -> Result<InstallHandoffSummary, StorageError> {
        let mut stmt = self.conn.prepare(
            r#"
SELECT handoff_id, task_id, package_id, installer, file, status, message
FROM install_handoffs
WHERE handoff_id = ?1
"#,
        )?;
        let row = stmt
            .query_row([handoff_id], |row| {
                Ok(InstallHandoffRow {
                    handoff_id: row.get(0)?,
                    task_id: row.get(1)?,
                    package_id: row.get(2)?,
                    installer: row.get(3)?,
                    file: row.get(4)?,
                    status: row.get(5)?,
                    message: row.get(6)?,
                })
            })
            .optional()?;
        row.map(install_handoff_from_row)
            .transpose()?
            .ok_or_else(|| StorageError::InstallHandoffNotFound(handoff_id.to_owned()))
    }

    pub fn record_install_result(
        &self,
        handoff_id: &str,
        status: InstallHandoffStatus,
        message: Option<&str>,
    ) -> Result<InstallHandoffSummary, StorageError> {
        if status == InstallHandoffStatus::Requested {
            return Err(StorageError::InvalidInstallHandoffTransition {
                handoff_id: handoff_id.to_owned(),
                reason: "requested is getter-created state, not a platform install result"
                    .to_owned(),
            });
        }
        let handoff = self.install_handoff(handoff_id)?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            r#"
UPDATE install_handoffs
SET status = ?1, message = ?2, updated_at_unix = unixepoch()
WHERE handoff_id = ?3
"#,
            params![status.as_str(), message, handoff_id],
        )?;
        insert_task_event_with_executor(
            &tx,
            &handoff.task_id,
            TaskEventKind::InstallResultRecorded,
            None,
            Some("Install result recorded"),
        )?;
        tx.commit()?;
        self.install_handoff(handoff_id)
    }

    pub fn task_events_after(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<TaskEventPage, StorageError> {
        let fetch_limit = limit.saturating_add(1) as i64;
        let mut stmt = self.conn.prepare(
            r#"
SELECT cursor, task_id, kind, status, message
FROM task_events
WHERE cursor > ?1
ORDER BY cursor ASC
LIMIT ?2
"#,
        )?;
        let rows = stmt.query_map(params![after as i64, fetch_limit], |row| {
            Ok(TaskEventRow {
                cursor: row.get(0)?,
                task_id: row.get(1)?,
                kind: row.get(2)?,
                status: row.get(3)?,
                message: row.get(4)?,
            })
        })?;
        let mut events = Vec::new();
        for row in rows {
            events.push(task_event_from_row(row?)?);
        }
        let has_more = events.len() > limit;
        if has_more {
            events.truncate(limit);
        }
        let next_cursor = events.last().map(|event| event.cursor).unwrap_or(after);
        Ok(TaskEventPage {
            events,
            next_cursor,
            has_more,
        })
    }

    fn update_download_task_status(
        &self,
        task_id: &str,
        status: DownloadTaskStatus,
        downloaded_file: Option<&str>,
        failure_message: Option<&str>,
        event_kind: TaskEventKind,
        event_message: &str,
    ) -> Result<(), StorageError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            r#"
UPDATE download_tasks
SET status = ?1,
    downloaded_file = COALESCE(?2, downloaded_file),
    failure_message = ?3,
    updated_at_unix = unixepoch()
WHERE task_id = ?4
"#,
            params![status.as_str(), downloaded_file, failure_message, task_id],
        )?;
        insert_task_event_with_executor(
            &tx,
            task_id,
            event_kind,
            Some(status),
            Some(event_message),
        )?;
        tx.commit()?;
        Ok(())
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
    pub pin_version: Option<String>,
    pub repository_id: Option<RepositoryId>,
    pub package_resolution: StoredPackageResolution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTrackedPackage {
    pub package_id: PackageId,
    pub enabled: bool,
    pub favorite: bool,
    pub pin_version: Option<String>,
    pub repository_id: Option<RepositoryId>,
    pub package_resolution: StoredPackageResolution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMigrationRecord {
    pub id: String,
    pub source: String,
    pub report_json: String,
}

struct DownloadTaskRow {
    task_id: String,
    package_id: String,
    status: String,
    executor: String,
    actions_json: String,
    download_file_name: String,
    downloaded_file: Option<String>,
    failure_message: Option<String>,
    install_handoff_id: Option<String>,
}

struct TaskEventRow {
    cursor: i64,
    task_id: String,
    kind: String,
    status: Option<String>,
    message: Option<String>,
}

struct InstallHandoffRow {
    handoff_id: String,
    task_id: String,
    package_id: String,
    installer: String,
    file: String,
    status: String,
    message: Option<String>,
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
    pin_version,
    repository_id,
    package_resolution
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(package_id) DO UPDATE SET
    enabled = excluded.enabled,
    favorite = excluded.favorite,
    pin_version = excluded.pin_version,
    repository_id = excluded.repository_id,
    package_resolution = excluded.package_resolution
"#,
        params![
            package.package_id.to_string(),
            bool_to_i64(package.enabled),
            bool_to_i64(package.favorite),
            package.pin_version.as_deref(),
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

fn download_task_from_row(row: DownloadTaskRow) -> Result<DownloadTaskSummary, StorageError> {
    Ok(DownloadTaskSummary {
        id: row.task_id,
        package_id: row.package_id.parse()?,
        status: row
            .status
            .parse()
            .map_err(|error: TaskModelError| StorageError::TaskState(error.to_string()))?,
        executor: row
            .executor
            .parse()
            .map_err(|error: TaskModelError| StorageError::TaskState(error.to_string()))?,
        actions: serde_json::from_str(&row.actions_json)?,
        download_file_name: row.download_file_name,
        downloaded_file: row.downloaded_file,
        failure_message: row.failure_message,
        install_handoff_id: row.install_handoff_id,
    })
}

fn task_event_from_row(row: TaskEventRow) -> Result<TaskEvent, StorageError> {
    Ok(TaskEvent {
        cursor: row.cursor as u64,
        task_id: row.task_id,
        kind: row
            .kind
            .parse()
            .map_err(|error: TaskModelError| StorageError::TaskState(error.to_string()))?,
        status: row
            .status
            .map(|status| status.parse())
            .transpose()
            .map_err(|error: TaskModelError| StorageError::TaskState(error.to_string()))?,
        message: row.message,
    })
}

fn install_handoff_from_row(row: InstallHandoffRow) -> Result<InstallHandoffSummary, StorageError> {
    Ok(InstallHandoffSummary {
        id: row.handoff_id,
        task_id: row.task_id,
        package_id: row.package_id.parse()?,
        installer: row.installer,
        file: row.file,
        status: row
            .status
            .parse()
            .map_err(|error: TaskModelError| StorageError::TaskState(error.to_string()))?,
        message: row.message,
    })
}

fn insert_task_event_with_executor(
    conn: &impl SqlExecutor,
    task_id: &str,
    kind: TaskEventKind,
    status: Option<DownloadTaskStatus>,
    message: Option<&str>,
) -> Result<usize, rusqlite::Error> {
    conn.execute_statement(
        r#"
INSERT INTO task_events(task_id, kind, status, message)
VALUES (?1, ?2, ?3, ?4)
"#,
        params![
            task_id,
            kind.as_str(),
            status.map(DownloadTaskStatus::as_str),
            message,
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use getter_core::repository::REPO_API_VERSION_V1;
    use getter_core::task::TaskExecutor;

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
            pin_version: Some("1.2.3".to_owned()),
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
        assert_eq!(packages[0].pin_version.as_deref(), Some("1.2.3"));
        assert_eq!(
            packages[0].package_resolution,
            StoredPackageResolution::OfficialRepositoryPackage
        );
    }

    #[test]
    fn main_db_pins_and_unpins_tracked_package_version() {
        let db = MainDb::open_in_memory().unwrap();
        let package_id: PackageId = "android/org.fdroid.fdroid".parse().unwrap();

        let pinned = db
            .set_tracked_package_pin_version(&package_id, Some("1.2.3"))
            .unwrap();
        assert_eq!(pinned.pin_version.as_deref(), Some("1.2.3"));
        assert_eq!(
            pinned.package_resolution,
            StoredPackageResolution::MissingPackageDefinition
        );

        let unpinned = db
            .set_tracked_package_pin_version(&package_id, None)
            .unwrap();
        assert_eq!(unpinned.pin_version, None);
    }

    #[test]
    fn main_db_migrates_legacy_ignored_version_column_to_pin_version() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
CREATE TABLE tracked_packages (
    package_id TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL DEFAULT 1,
    favorite INTEGER NOT NULL DEFAULT 0,
    ignored_version TEXT,
    repository_id TEXT,
    package_resolution TEXT NOT NULL DEFAULT 'missing_package_definition'
);
INSERT INTO tracked_packages(package_id, ignored_version)
VALUES ('android/org.fdroid.fdroid', '1.2.3');
"#,
        )
        .unwrap();
        let db = MainDb { conn };
        db.migrate().unwrap();

        let packages = db.tracked_packages().unwrap();
        assert_eq!(packages[0].pin_version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn generated_tracking_preserves_existing_user_state_on_conflict() {
        let db = MainDb::open_in_memory().unwrap();
        let package_id: PackageId = "android/org.fdroid.fdroid".parse().unwrap();
        let autogen = RepositoryId::new("autogen").unwrap();
        db.upsert_tracked_package(&TrackedPackageUpsert {
            package_id: package_id.clone(),
            enabled: false,
            favorite: true,
            pin_version: Some("9.9.9".to_owned()),
            repository_id: None,
            package_resolution: StoredPackageResolution::OfficialRepositoryPackage,
        })
        .unwrap();

        db.upsert_generated_tracked_package_preserving_user_state(&package_id, &autogen)
            .unwrap();

        let packages = db.tracked_packages().unwrap();
        assert_eq!(packages.len(), 1);
        assert!(!packages[0].enabled);
        assert!(packages[0].favorite);
        assert_eq!(packages[0].pin_version.as_deref(), Some("9.9.9"));
        assert_eq!(packages[0].repository_id, None);
        assert_eq!(
            packages[0].package_resolution,
            StoredPackageResolution::OfficialRepositoryPackage
        );
    }

    #[test]
    fn generated_tracking_fills_unresolved_tracking_metadata() {
        let db = MainDb::open_in_memory().unwrap();
        let package_id: PackageId = "android/org.fdroid.fdroid".parse().unwrap();
        let autogen = insert_autogen_repo(&db);
        db.upsert_tracked_package(&TrackedPackageUpsert {
            package_id: package_id.clone(),
            enabled: false,
            favorite: true,
            pin_version: Some("9.9.9".to_owned()),
            repository_id: None,
            package_resolution: StoredPackageResolution::MissingPackageDefinition,
        })
        .unwrap();

        db.upsert_generated_tracked_package_preserving_user_state(&package_id, &autogen)
            .unwrap();

        let packages = db.tracked_packages().unwrap();
        assert_eq!(packages.len(), 1);
        assert!(!packages[0].enabled);
        assert!(packages[0].favorite);
        assert_eq!(packages[0].pin_version.as_deref(), Some("9.9.9"));
        assert_eq!(packages[0].repository_id.as_ref(), Some(&autogen));
        assert_eq!(
            packages[0].package_resolution,
            StoredPackageResolution::GenerateLocalPackage
        );
    }

    #[test]
    fn generated_tracking_delete_is_guarded_by_repo_and_resolution() {
        let db = MainDb::open_in_memory().unwrap();
        let package_id: PackageId = "android/org.fdroid.fdroid".parse().unwrap();
        let autogen = insert_autogen_repo(&db);
        db.upsert_tracked_package(&TrackedPackageUpsert {
            package_id: package_id.clone(),
            enabled: true,
            favorite: true,
            pin_version: Some("9.9.9".to_owned()),
            repository_id: None,
            package_resolution: StoredPackageResolution::OfficialRepositoryPackage,
        })
        .unwrap();

        assert!(!db
            .delete_generated_tracked_package(&package_id, &autogen)
            .unwrap());
        assert_eq!(db.tracked_packages().unwrap().len(), 1);

        db.upsert_tracked_package(&TrackedPackageUpsert {
            package_id: package_id.clone(),
            enabled: true,
            favorite: true,
            pin_version: Some("9.9.9".to_owned()),
            repository_id: None,
            package_resolution: StoredPackageResolution::MissingPackageDefinition,
        })
        .unwrap();
        db.upsert_generated_tracked_package_preserving_user_state(&package_id, &autogen)
            .unwrap();
        assert!(db
            .delete_generated_tracked_package(&package_id, &autogen)
            .unwrap());
        assert!(db.tracked_packages().unwrap().is_empty());
    }

    fn insert_autogen_repo(db: &MainDb) -> RepositoryId {
        let autogen = RepositoryId::new("autogen").unwrap();
        db.upsert_repository(
            &RepositoryMetadata {
                id: autogen.clone(),
                name: "Autogen".to_owned(),
                priority: RepositoryPriority::GENERATED_FALLBACK,
                api_version: REPO_API_VERSION_V1.to_owned(),
            },
            None,
            None,
        )
        .unwrap();
        autogen
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
    fn task_lifecycle_persists_across_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("main.db");
        {
            let db = MainDb::open(&db_path).unwrap();
            let task = db.create_download_task(&download_request()).unwrap();
            assert_eq!(task.id, "task-1");
            assert_eq!(task.status, DownloadTaskStatus::Queued);
        }

        let db = MainDb::open(&db_path).unwrap();
        let tasks = db.download_tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "task-1");
        assert_eq!(tasks[0].package_id.to_string(), "android/org.fdroid.fdroid");
        assert_eq!(tasks[0].status, DownloadTaskStatus::Queued);
    }

    #[test]
    fn task_cancel_is_persistent_and_idempotent_before_terminal_state() {
        let db = MainDb::open_in_memory().unwrap();
        let task = db.create_download_task(&download_request()).unwrap();

        let first = db.cancel_download_task(&task.id).unwrap();
        assert!(first.changed);
        assert_eq!(first.status, DownloadTaskStatus::Canceled);
        let second = db.cancel_download_task(&task.id).unwrap();
        assert!(!second.changed);
        assert_eq!(
            db.download_task(&task.id).unwrap().status,
            DownloadTaskStatus::Canceled
        );
    }

    #[test]
    fn task_cancel_after_success_is_rejected() {
        let db = MainDb::open_in_memory().unwrap();
        let task = db.create_download_task(&download_request()).unwrap();
        db.start_download_task(&task.id).unwrap();
        db.succeed_download_task(&task.id).unwrap();

        let error = db.cancel_download_task(&task.id).unwrap_err();
        assert!(matches!(error, StorageError::InvalidTaskTransition { .. }));
    }

    #[test]
    fn task_events_are_pollable_with_cursor_and_limit() {
        let db = MainDb::open_in_memory().unwrap();
        let task = db.create_download_task(&download_request()).unwrap();
        db.start_download_task(&task.id).unwrap();
        db.succeed_download_task(&task.id).unwrap();

        let first_page = db.task_events_after(0, 2).unwrap();
        assert_eq!(first_page.events.len(), 2);
        assert!(first_page.has_more);
        assert_eq!(first_page.events[0].kind, TaskEventKind::TaskCreated);
        assert_eq!(first_page.events[1].kind, TaskEventKind::TaskStarted);

        let second_page = db.task_events_after(first_page.next_cursor, 2).unwrap();
        assert_eq!(second_page.events.len(), 1);
        assert!(!second_page.has_more);
        assert_eq!(second_page.events[0].kind, TaskEventKind::TaskSucceeded);
    }

    #[test]
    fn install_handoff_result_is_recorded() {
        let db = MainDb::open_in_memory().unwrap();
        let task = db.create_download_task(&download_request()).unwrap();
        db.start_download_task(&task.id).unwrap();
        db.succeed_download_task(&task.id).unwrap();
        let handoff = db
            .create_install_handoff_for_task(&task.id)
            .unwrap()
            .unwrap();
        assert_eq!(handoff.id, "handoff-1");
        assert_eq!(handoff.status, InstallHandoffStatus::Requested);

        let updated = db
            .record_install_result(&handoff.id, InstallHandoffStatus::Succeeded, Some("ok"))
            .unwrap();
        assert_eq!(updated.status, InstallHandoffStatus::Succeeded);
        assert_eq!(updated.message.as_deref(), Some("ok"));
    }

    #[test]
    fn install_result_rejects_getter_created_requested_state() {
        let db = MainDb::open_in_memory().unwrap();
        let task = db.create_download_task(&download_request()).unwrap();
        db.start_download_task(&task.id).unwrap();
        db.succeed_download_task(&task.id).unwrap();
        let handoff = db
            .create_install_handoff_for_task(&task.id)
            .unwrap()
            .unwrap();

        let error = db
            .record_install_result(&handoff.id, InstallHandoffStatus::Requested, None)
            .unwrap_err();
        assert!(matches!(
            error,
            StorageError::InvalidInstallHandoffTransition { .. }
        ));
    }

    #[test]
    fn task_request_requires_download_action() {
        let db = MainDb::open_in_memory().unwrap();
        let mut request = download_request();
        request.actions = vec![UpdateAction::Install {
            installer: "android_package".to_owned(),
            file: "app.apk".to_owned(),
        }];

        let error = db.create_download_task(&request).unwrap_err();
        assert!(matches!(error, StorageError::InvalidTaskRequest(_)));
    }

    fn download_request() -> DownloadTaskRequest {
        DownloadTaskRequest {
            format: getter_core::task::DOWNLOAD_REQUEST_FORMAT.to_owned(),
            version: getter_core::task::DOWNLOAD_REQUEST_VERSION,
            package_id: "android/org.fdroid.fdroid".parse().unwrap(),
            executor: TaskExecutor::Fake,
            actions: vec![
                UpdateAction::Download {
                    url: "https://example.invalid/app.apk".to_owned(),
                    file_name: "app.apk".to_owned(),
                },
                UpdateAction::Install {
                    installer: "android_package".to_owned(),
                    file: "app.apk".to_owned(),
                },
            ],
        }
    }

    #[test]
    fn cache_db_migrates_schema() {
        let _db = CacheDb::open_in_memory().unwrap();
    }
}
