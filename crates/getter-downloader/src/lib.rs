//! Deterministic offline downloader lifecycle for the getter rewrite.
//!
//! This crate intentionally does not perform live network I/O. It proves the
//! getter-owned task state, cancellation, pollable events, and install-handoff
//! contract with an explicit fake executor.

use getter_core::task::{
    DownloadTaskRequest, InstallHandoffStatus, InstallResultRecord, TaskCancelResult,
    TaskRunResult, TaskSubmitResult, DOWNLOAD_REQUEST_FORMAT, DOWNLOAD_REQUEST_VERSION,
};
use getter_storage::{MainDb, StorageError};

pub use getter_core as core;

#[derive(Debug, thiserror::Error)]
pub enum DownloaderError {
    #[error("unsupported download request format '{0}'")]
    UnsupportedFormat(String),
    #[error("unsupported download request version {found}; expected {expected}")]
    UnsupportedVersion { found: u32, expected: u32 },
    #[error("download request must use the fake executor for this offline slice")]
    UnsupportedExecutor,
    #[error("download request must include a download action")]
    MissingDownloadAction,
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
}

pub fn submit_fake_download_task(
    db: &MainDb,
    request: DownloadTaskRequest,
) -> Result<TaskSubmitResult, DownloaderError> {
    validate_fake_request(&request)?;
    let task = db.create_download_task(&request)?;
    Ok(TaskSubmitResult { task })
}

pub fn run_fake_download_task(
    db: &MainDb,
    task_id: &str,
) -> Result<TaskRunResult, DownloaderError> {
    db.start_download_task(task_id)?;
    let task = db.succeed_download_task(task_id)?;
    let install_handoff = db.create_install_handoff_for_task(task_id)?;
    Ok(TaskRunResult {
        task,
        install_handoff,
    })
}

pub fn cancel_download_task(
    db: &MainDb,
    task_id: &str,
) -> Result<TaskCancelResult, DownloaderError> {
    Ok(db.cancel_download_task(task_id)?)
}

pub fn record_install_result(
    db: &MainDb,
    handoff_id: &str,
    status: InstallHandoffStatus,
) -> Result<InstallResultRecord, DownloaderError> {
    let handoff = db.record_install_result(handoff_id, status, None)?;
    Ok(InstallResultRecord { handoff })
}

fn validate_fake_request(request: &DownloadTaskRequest) -> Result<(), DownloaderError> {
    if request.format != DOWNLOAD_REQUEST_FORMAT {
        return Err(DownloaderError::UnsupportedFormat(request.format.clone()));
    }
    if request.version != DOWNLOAD_REQUEST_VERSION {
        return Err(DownloaderError::UnsupportedVersion {
            found: request.version,
            expected: DOWNLOAD_REQUEST_VERSION,
        });
    }
    // This match is intentionally explicit so adding a real executor later forces
    // this offline-only validation point to be revisited.
    match request.executor {
        getter_core::task::TaskExecutor::Fake => {}
    }
    if request.download_action().is_none() {
        return Err(DownloaderError::MissingDownloadAction);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use getter_core::task::{DownloadTaskStatus, TaskEventKind, TaskExecutor};
    use getter_core::{PackageId, UpdateAction};

    #[test]
    fn fake_submit_run_records_install_handoff_and_events() {
        let db = MainDb::open_in_memory().unwrap();
        let submitted = submit_fake_download_task(&db, request()).unwrap();
        assert_eq!(submitted.task.id, "task-1");
        assert_eq!(submitted.task.status, DownloadTaskStatus::Queued);

        let result = run_fake_download_task(&db, &submitted.task.id).unwrap();
        assert_eq!(result.task.status, DownloadTaskStatus::Succeeded);
        assert_eq!(
            result.install_handoff.as_ref().unwrap().installer,
            "android_package"
        );

        let events = db.task_events_after(0, 10).unwrap().events;
        assert_eq!(events[0].kind, TaskEventKind::TaskCreated);
        assert_eq!(events[1].kind, TaskEventKind::TaskStarted);
        assert_eq!(events[2].kind, TaskEventKind::TaskSucceeded);
        assert_eq!(events[3].kind, TaskEventKind::InstallHandoffRequested);
    }

    #[test]
    fn fake_cancel_before_run_is_persisted() {
        let db = MainDb::open_in_memory().unwrap();
        let submitted = submit_fake_download_task(&db, request()).unwrap();

        let canceled = cancel_download_task(&db, &submitted.task.id).unwrap();
        assert!(canceled.changed);
        assert_eq!(canceled.status, DownloadTaskStatus::Canceled);
        assert!(run_fake_download_task(&db, &submitted.task.id).is_err());
    }

    #[test]
    fn fake_submit_rejects_wrong_contract() {
        let db = MainDb::open_in_memory().unwrap();
        let mut request = request();
        request.format = "wrong".to_owned();

        let error = submit_fake_download_task(&db, request).unwrap_err();
        assert!(matches!(error, DownloaderError::UnsupportedFormat(_)));
    }

    #[test]
    fn install_result_can_be_recorded_after_handoff() {
        let db = MainDb::open_in_memory().unwrap();
        let submitted = submit_fake_download_task(&db, request()).unwrap();
        let result = run_fake_download_task(&db, &submitted.task.id).unwrap();
        let handoff = result.install_handoff.unwrap();

        let recorded =
            record_install_result(&db, &handoff.id, InstallHandoffStatus::Succeeded).unwrap();
        assert_eq!(recorded.handoff.status, InstallHandoffStatus::Succeeded);
    }

    fn request() -> DownloadTaskRequest {
        DownloadTaskRequest {
            format: DOWNLOAD_REQUEST_FORMAT.to_owned(),
            version: DOWNLOAD_REQUEST_VERSION,
            package_id: PackageId::new(getter_core::PackageKind::Android, "org.fdroid.fdroid")
                .unwrap(),
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
}
