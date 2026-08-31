//! Getter-owned runtime download executor.
//!
//! This is the first real-byte side-effect executor for ADR-0011 tasks. It is
//! deliberately current-process/task-local: it writes bytes for the submitted
//! task and updates the in-memory runtime snapshot, but it does not create a
//! durable task database, background worker, installer, or recovery mechanism.

use getter_core::runtime::{
    DownloadedFile, GetterRuntime, RuntimeError, TaskPhaseCategory, TaskSnapshot,
};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

const RUNTIME_DOWNLOAD_TRANSPORT_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeDownloadTransportRequest<'a> {
    pub url: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeDownloadTransportError {
    #[error("download transport failed: {0}")]
    Transport(String),
    #[error("download transport returned HTTP {status}: {message}")]
    HttpStatus { status: u16, message: String },
    #[error("download sink failed: {0}")]
    Sink(String),
}

pub trait RuntimeDownloadSink {
    fn set_total_bytes(&mut self, total_bytes: Option<u64>) -> Result<(), String>;
    fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), String>;
}

pub trait RuntimeDownloadTransport {
    fn fetch(
        &self,
        request: &RuntimeDownloadTransportRequest<'_>,
        sink: &mut dyn RuntimeDownloadSink,
    ) -> Result<(), RuntimeDownloadTransportError>;
}

pub struct UreqRuntimeDownloadTransport {
    agent: ureq::Agent,
}

impl Default for UreqRuntimeDownloadTransport {
    fn default() -> Self {
        #[cfg(feature = "rustls-platform-verifier")]
        {
            return Self {
                agent: ureq::Agent::config_builder()
                    .timeout_global(Some(RUNTIME_DOWNLOAD_TRANSPORT_TIMEOUT))
                    .tls_config(
                        ureq::tls::TlsConfig::builder()
                            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                            .build(),
                    )
                    .build()
                    .new_agent(),
            };
        }

        #[cfg(not(feature = "rustls-platform-verifier"))]
        {
            Self {
                agent: ureq::Agent::config_builder()
                    .timeout_global(Some(RUNTIME_DOWNLOAD_TRANSPORT_TIMEOUT))
                    .build()
                    .new_agent(),
            }
        }
    }
}

impl UreqRuntimeDownloadTransport {
    pub fn new() -> Self {
        Self::default()
    }
}

impl RuntimeDownloadTransport for UreqRuntimeDownloadTransport {
    fn fetch(
        &self,
        request: &RuntimeDownloadTransportRequest<'_>,
        sink: &mut dyn RuntimeDownloadSink,
    ) -> Result<(), RuntimeDownloadTransportError> {
        let mut response = self
            .agent
            .get(request.url)
            .header("Accept", "application/octet-stream, */*")
            .header("User-Agent", "UpgradeAll-getter")
            .call()
            .map_err(download_transport_error)?;
        sink.set_total_bytes(response.body().content_length())
            .map_err(RuntimeDownloadTransportError::Sink)?;
        let mut reader = response.body_mut().as_reader();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|source| RuntimeDownloadTransportError::Transport(source.to_string()))?;
            if read == 0 {
                break;
            }
            sink.write_chunk(&buffer[..read])
                .map_err(RuntimeDownloadTransportError::Sink)?;
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeDownloadExecutionError {
    #[error("task has no download action")]
    MissingDownloadAction,
    #[error("failed to prepare download file: {0}")]
    PrepareFile(std::io::Error),
    #[error("download failed: {0}")]
    Transport(#[from] RuntimeDownloadTransportError),
    #[error("failed to finish download file: {0}")]
    FinishFile(std::io::Error),
    #[error("runtime error: {0}")]
    Runtime(#[from] RuntimeError),
}

impl RuntimeDownloadExecutionError {
    pub fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::MissingDownloadAction => "download.action_missing",
            Self::PrepareFile(_) | Self::FinishFile(_) => "download.file_error",
            Self::Transport(RuntimeDownloadTransportError::HttpStatus { .. }) => {
                "download.http_status"
            }
            Self::Transport(_) => "download.transport_error",
            Self::Runtime(error) => error.code(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeDownloadOperationError {
    #[error("runtime error: {0}")]
    Runtime(#[from] RuntimeError),
}

pub fn submit_action_and_download<T>(
    runtime: &mut GetterRuntime,
    data_dir: &Path,
    action_id: &str,
    transport: &T,
) -> Result<TaskSnapshot, RuntimeDownloadOperationError>
where
    T: RuntimeDownloadTransport + ?Sized,
{
    let submitted = runtime.submit_action(action_id)?;
    let task_id = submitted.task_id.clone();
    match execute_download_task(runtime, data_dir, &task_id, transport) {
        Ok(task) => Ok(task),
        Err(error) => {
            Ok(runtime.fail_download(&task_id, error.diagnostic_code(), error.to_string())?)
        }
    }
}

pub fn retry_download_task<T>(
    runtime: &mut GetterRuntime,
    data_dir: &Path,
    task_id: &str,
    transport: &T,
) -> Result<TaskSnapshot, RuntimeDownloadOperationError>
where
    T: RuntimeDownloadTransport + ?Sized,
{
    let retried = runtime.retry_task(task_id)?;
    if retried.phase.category != TaskPhaseCategory::Download {
        return Ok(retried);
    }
    match execute_download_task(runtime, data_dir, task_id, transport) {
        Ok(task) => Ok(task),
        Err(error) => {
            Ok(runtime.fail_download(task_id, error.diagnostic_code(), error.to_string())?)
        }
    }
}

pub fn execute_download_task<T>(
    runtime: &mut GetterRuntime,
    data_dir: &Path,
    task_id: &str,
    transport: &T,
) -> Result<TaskSnapshot, RuntimeDownloadExecutionError>
where
    T: RuntimeDownloadTransport + ?Sized,
{
    runtime.start_task(task_id)?;
    let download = runtime
        .download_plan(task_id)?
        .ok_or(RuntimeDownloadExecutionError::MissingDownloadAction)?;
    let file_name = safe_download_file_name(&download.file_name);
    let download_dir = data_dir.join("downloads").join(task_id);
    fs::create_dir_all(&download_dir).map_err(RuntimeDownloadExecutionError::PrepareFile)?;
    let final_path = download_dir.join(&file_name);
    let partial_path = download_dir.join(format!(".{file_name}.part"));
    let file = File::create(&partial_path).map_err(RuntimeDownloadExecutionError::PrepareFile)?;

    let mut sink = RuntimeFileDownloadSink::new(runtime, task_id, file);
    let fetch_result = transport.fetch(
        &RuntimeDownloadTransportRequest { url: &download.url },
        &mut sink,
    );
    let write_result = sink.finish();

    if let Err(error) = fetch_result {
        let _ = fs::remove_file(&partial_path);
        return Err(RuntimeDownloadExecutionError::Transport(error));
    }
    let written = write_result.map_err(RuntimeDownloadExecutionError::FinishFile)?;
    fs::rename(&partial_path, &final_path).map_err(RuntimeDownloadExecutionError::FinishFile)?;

    let downloaded_file = DownloadedFile {
        file_name,
        local_path: final_path.to_string_lossy().into_owned(),
        size_bytes: written.size_bytes,
        sha256: written.sha256,
    };
    Ok(runtime.complete_downloaded_file(task_id, downloaded_file)?)
}

struct RuntimeFileDownloadSink<'a> {
    runtime: &'a mut GetterRuntime,
    task_id: String,
    file: File,
    hasher: Sha256,
    size_bytes: u64,
    total_bytes: Option<u64>,
}

impl<'a> RuntimeFileDownloadSink<'a> {
    fn new(runtime: &'a mut GetterRuntime, task_id: &str, file: File) -> Self {
        Self {
            runtime,
            task_id: task_id.to_owned(),
            file,
            hasher: Sha256::new(),
            size_bytes: 0,
            total_bytes: None,
        }
    }

    fn finish(mut self) -> std::io::Result<RuntimeFileDownloadResult> {
        self.file.flush()?;
        self.file.sync_all()?;
        Ok(RuntimeFileDownloadResult {
            size_bytes: self.size_bytes,
            sha256: format!("{:x}", self.hasher.finalize()),
        })
    }

    fn total_bits(&self) -> Option<u64> {
        self.total_bytes.map(|bytes| bytes.saturating_mul(8))
    }

    fn current_bits(&self) -> u64 {
        self.size_bytes.saturating_mul(8)
    }

    fn publish_progress(&mut self) -> Result<(), String> {
        self.runtime
            .set_download_progress(&self.task_id, self.current_bits(), self.total_bits())
            .map(|_| ())
            .map_err(|source| source.to_string())
    }
}

impl RuntimeDownloadSink for RuntimeFileDownloadSink<'_> {
    fn set_total_bytes(&mut self, total_bytes: Option<u64>) -> Result<(), String> {
        self.total_bytes = total_bytes;
        self.publish_progress()
    }

    fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), String> {
        self.file
            .write_all(chunk)
            .map_err(|source| source.to_string())?;
        self.hasher.update(chunk);
        self.size_bytes = self
            .size_bytes
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| "download size overflowed u64".to_owned())?;
        self.publish_progress()
    }
}

struct RuntimeFileDownloadResult {
    size_bytes: u64,
    sha256: String,
}

pub(crate) fn safe_download_file_name(raw: &str) -> String {
    let base = raw
        .trim()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim();
    let sanitized: String = base
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\') {
                '_'
            } else {
                ch
            }
        })
        .collect();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        "artifact.bin".to_owned()
    } else {
        sanitized
    }
}

fn download_transport_error(source: ureq::Error) -> RuntimeDownloadTransportError {
    match source {
        ureq::Error::StatusCode(status) => RuntimeDownloadTransportError::HttpStatus {
            status,
            message: download_http_status_message(status).to_owned(),
        },
        other => RuntimeDownloadTransportError::Transport(other.to_string()),
    }
}

fn download_http_status_message(status: u16) -> &'static str {
    match status {
        401 => "download requires authentication",
        403 => "download is forbidden",
        404 => "download artifact was not found",
        429 => "download was rate limited",
        _ => "download request failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use getter_core::runtime::{PackageVersionLuaObject, RuntimeNotification, SealedActionPlan};
    use getter_core::{PackageId, PackageKind, UpdateAction};
    use std::cell::RefCell;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    struct ChunkedDownloadTransport {
        chunks: Vec<&'static [u8]>,
        total_bytes: Option<u64>,
        requests: RefCell<Vec<String>>,
    }

    impl RuntimeDownloadTransport for ChunkedDownloadTransport {
        fn fetch(
            &self,
            request: &RuntimeDownloadTransportRequest<'_>,
            sink: &mut dyn RuntimeDownloadSink,
        ) -> Result<(), RuntimeDownloadTransportError> {
            self.requests.borrow_mut().push(request.url.to_owned());
            sink.set_total_bytes(self.total_bytes)
                .map_err(RuntimeDownloadTransportError::Sink)?;
            for chunk in &self.chunks {
                sink.write_chunk(chunk)
                    .map_err(RuntimeDownloadTransportError::Sink)?;
            }
            Ok(())
        }
    }

    #[test]
    fn submit_action_downloads_bytes_to_getter_owned_file_and_notifies_progress() {
        let temp = tempfile::tempdir().unwrap();
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let sink_notifications = notifications.clone();
        let mut runtime = GetterRuntime::new();
        runtime.set_notification_sink(move |notification| {
            sink_notifications.lock().unwrap().push(notification);
        });
        let action = runtime.issue_action(download_plan("generic/example", "../app.bin"));
        let transport = ChunkedDownloadTransport {
            chunks: vec![b"hello ", b"getter"],
            total_bytes: Some(12),
            requests: RefCell::new(Vec::new()),
        };

        let task =
            submit_action_and_download(&mut runtime, temp.path(), &action.action_id, &transport)
                .unwrap();

        assert_eq!(
            transport.requests.borrow().as_slice(),
            &["https://example.invalid/app.bin"]
        );
        assert_eq!(
            task.status,
            getter_core::runtime::RuntimeTaskStatus::Completed
        );
        assert_eq!(task.downloaded_file.as_ref().unwrap().file_name, "app.bin");
        assert_eq!(task.downloaded_file.as_ref().unwrap().size_bytes, 12);
        assert_eq!(
            fs::read(&task.downloaded_file.as_ref().unwrap().local_path).unwrap(),
            b"hello getter"
        );
        assert_eq!(
            task.downloaded_file.as_ref().unwrap().sha256,
            "e20360c4483ac234e3233f2f82671d46f67dd09f8d1da753c8c81236aed04690"
        );

        let notifications = notifications.lock().unwrap();
        assert!(notifications.iter().any(|notification| matches!(
            notification,
            RuntimeNotification::TaskChanged { task }
                if task.phase.category == getter_core::runtime::TaskPhaseCategory::Download
                    && task.progress.as_ref().is_some_and(|progress| progress.current == 48 && progress.total == Some(96))
        )));
        assert!(notifications.iter().any(|notification| matches!(
            notification,
            RuntimeNotification::TaskChanged { task }
                if task.status == getter_core::runtime::RuntimeTaskStatus::Completed
                    && task.downloaded_file.is_some()
        )));
    }

    #[test]
    fn transport_failure_marks_existing_task_failed_without_restoring_action() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = GetterRuntime::new();
        let action = runtime.issue_action(download_plan("generic/example", "app.bin"));
        let transport = FailingDownloadTransport;

        let task =
            submit_action_and_download(&mut runtime, temp.path(), &action.action_id, &transport)
                .unwrap();

        assert_eq!(task.status, getter_core::runtime::RuntimeTaskStatus::Failed);
        assert_eq!(
            task.current_diagnostic.as_ref().unwrap().code,
            "download.http_status"
        );
        assert_eq!(
            runtime.submit_action(&action.action_id).unwrap_err().code(),
            "action.not_found"
        );
    }

    #[test]
    fn retry_download_reuses_same_task_and_writes_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = GetterRuntime::new();
        let action = runtime.issue_action(download_plan("generic/example", "app.bin"));
        let failed = submit_action_and_download(
            &mut runtime,
            temp.path(),
            &action.action_id,
            &FailingDownloadTransport,
        )
        .unwrap();
        let transport = ChunkedDownloadTransport {
            chunks: vec![b"retry bytes"],
            total_bytes: Some(11),
            requests: RefCell::new(Vec::new()),
        };

        let retried = retry_download_task(&mut runtime, temp.path(), &failed.task_id, &transport)
            .expect("retry download");

        assert_eq!(retried.task_id, failed.task_id);
        assert_eq!(
            retried.status,
            getter_core::runtime::RuntimeTaskStatus::Completed
        );
        assert_eq!(retried.downloaded_file.as_ref().unwrap().size_bytes, 11);
        assert_eq!(
            fs::read(&retried.downloaded_file.as_ref().unwrap().local_path).unwrap(),
            b"retry bytes"
        );
    }

    struct FailingDownloadTransport;

    impl RuntimeDownloadTransport for FailingDownloadTransport {
        fn fetch(
            &self,
            _: &RuntimeDownloadTransportRequest<'_>,
            _: &mut dyn RuntimeDownloadSink,
        ) -> Result<(), RuntimeDownloadTransportError> {
            Err(RuntimeDownloadTransportError::HttpStatus {
                status: 404,
                message: "download artifact was not found".to_owned(),
            })
        }
    }

    #[test]
    fn default_transport_fetches_bytes_from_mock_http_server() {
        let temp = tempfile::tempdir().unwrap();
        let (url, handle) = serve_one_download_response(b"mock artifact bytes");
        let mut runtime = GetterRuntime::new();
        let action =
            runtime.issue_action(download_plan_with_url("generic/example", &url, "app.bin"));

        let task = submit_action_and_download(
            &mut runtime,
            temp.path(),
            &action.action_id,
            &UreqRuntimeDownloadTransport::new(),
        )
        .unwrap();
        let request = handle.join().unwrap();

        assert!(request.starts_with("GET /artifact.bin HTTP/1.1"));
        assert!(request.contains("accept: application/octet-stream, */*"));
        assert!(request.contains("user-agent: UpgradeAll-getter"));
        assert_eq!(task.downloaded_file.as_ref().unwrap().size_bytes, 19);
        assert_eq!(
            fs::read(&task.downloaded_file.as_ref().unwrap().local_path).unwrap(),
            b"mock artifact bytes"
        );
    }

    fn download_plan(package_id: &str, file_name: &str) -> SealedActionPlan {
        download_plan_with_url(package_id, "https://example.invalid/app.bin", file_name)
    }

    fn download_plan_with_url(package_id: &str, url: &str, file_name: &str) -> SealedActionPlan {
        SealedActionPlan {
            package_id: PackageId::new(PackageKind::Generic, package_id.split('/').nth(1).unwrap())
                .unwrap(),
            lua_object: PackageVersionLuaObject {
                object_id: format!("lua:{package_id}:1.0"),
                dependency_digest: "sha256-test".to_owned(),
            },
            actions: vec![UpdateAction::Download {
                url: url.to_owned(),
                file_name: file_name.to_owned(),
            }],
            android_apk_install: None,
        }
    }

    fn serve_one_download_response(body: &'static [u8]) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = stream.read(&mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (format!("http://{address}/artifact.bin"), handle)
    }
}
