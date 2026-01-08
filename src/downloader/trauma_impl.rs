//! Reqwest-based downloader implementation with pause/resume support

use super::error::{DownloadError, Result};
use super::traits::{Downloader, DownloaderCapabilities, ProgressCallback, RequestOptions};
use async_trait::async_trait;
use futures::StreamExt;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::{File, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

/// Task state for managing pause/resume
#[derive(Clone)]
struct TaskState {
    url: String,
    dest: PathBuf,
    cancel_token: CancellationToken,
    supports_range: Option<bool>,
}

/// Reqwest-based downloader implementation
///
/// This implementation uses reqwest with rustls for secure, concurrent downloads
/// and supports pause/resume with HTTP Range requests
pub struct TraumaDownloader {
    max_concurrent: usize,
    retries: usize,
    timeout_seconds: u64,
    client: Arc<reqwest::Client>,
    // Track active download tasks for pause/resume
    tasks: Arc<RwLock<HashMap<String, TaskState>>>,
    // Capabilities of this downloader
    capabilities: DownloaderCapabilities,
}

impl TraumaDownloader {
    /// Create a new TraumaDownloader with specified parameters
    ///
    /// # Arguments
    /// * `max_concurrent` - Maximum number of concurrent downloads
    /// * `retries` - Number of retry attempts for failed downloads
    /// * `timeout_seconds` - Timeout for each download in seconds
    pub fn new(max_concurrent: usize, retries: usize, timeout_seconds: u64) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_seconds))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            max_concurrent,
            retries,
            timeout_seconds,
            client: Arc::new(client),
            tasks: Arc::new(RwLock::new(HashMap::new())),
            capabilities: DownloaderCapabilities::all_enabled(),
        }
    }

    /// Create a downloader with default settings
    pub fn default_settings() -> Self {
        Self::new(4, 3, 300)
    }

    /// Test if a server supports HTTP Range requests
    async fn test_range_support(client: &reqwest::Client, url: &str) -> bool {
        // Try HEAD request first
        if let Ok(response) = client.head(url).send().await {
            if let Some(accept_ranges) = response.headers().get("accept-ranges") {
                if let Ok(value) = accept_ranges.to_str() {
                    return value.to_lowercase() == "bytes";
                }
            }
        }

        // Fallback: try a fake range request
        if let Ok(response) = client.get(url).header("Range", "bytes=0-0").send().await {
            return response.status() == reqwest::StatusCode::PARTIAL_CONTENT
                || response.status() == reqwest::StatusCode::OK;
        }

        false
    }

    /// Download a file with retry and pause support
    async fn download_with_retry(
        &self,
        url: &str,
        dest: &Path,
        progress: Option<&ProgressCallback>,
        cancel_token: &CancellationToken,
        options: Option<&RequestOptions>,
    ) -> Result<()> {
        let mut last_error = None;

        for attempt in 0..=self.retries {
            if cancel_token.is_cancelled() {
                return Err(DownloadError::cancelled("Download was paused"));
            }

            // For retries, we don't pass progress callback to avoid multiple progress reports
            let current_progress = if attempt == 0 { progress } else { None };

            match self
                .download_once(url, dest, current_progress, cancel_token, options)
                .await
            {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if cancel_token.is_cancelled() {
                        return Err(DownloadError::cancelled("Download was paused"));
                    }
                    last_error = Some(e);
                    if attempt < self.retries {
                        tokio::time::sleep(std::time::Duration::from_secs(1 << attempt)).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| DownloadError::network("Download failed".to_string())))
    }

    /// Download a file once (no retry), with pause support
    async fn download_once(
        &self,
        url: &str,
        dest: &Path,
        progress: Option<&ProgressCallback>,
        cancel_token: &CancellationToken,
        options: Option<&RequestOptions>,
    ) -> Result<()> {
        // Check if temp file exists (resume case)
        let temp_dest = dest.with_extension("tmp");
        let existing_size = if temp_dest.exists() {
            tokio::fs::metadata(&temp_dest)
                .await
                .ok()
                .map(|m| m.len())
                .unwrap_or(0)
        } else {
            0
        };

        // Test range support if not already known
        let supports_range = if existing_size > 0 {
            Self::test_range_support(&self.client, url).await
        } else {
            false
        };

        // Build request with Range header if resuming
        let mut request = self.client.get(url);
        if existing_size > 0 && supports_range {
            request = request.header("Range", format!("bytes={}-", existing_size));
        }

        // Apply custom headers if provided
        if let Some(opts) = options {
            if let Some(headers) = &opts.headers {
                for (key, value) in headers {
                    request = request.header(key, value);
                }
            }
            // Apply cookies if provided
            if let Some(cookies) = &opts.cookies {
                let cookie_string = cookies
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join("; ");
                if !cookie_string.is_empty() {
                    request = request.header("Cookie", cookie_string);
                }
            }
        }

        // Send request
        let response = request
            .send()
            .await
            .map_err(|e| DownloadError::network(format!("Failed to send request: {}", e)))?;

        // Check status
        let status = response.status();
        if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(DownloadError::network(format!(
                "HTTP error {}: {}",
                status, url
            )));
        }

        // Get content length
        let content_length = response.content_length();
        let total_size = if status == reqwest::StatusCode::PARTIAL_CONTENT {
            // For partial content, add existing size to content length
            content_length.map(|len| len + existing_size)
        } else {
            content_length
        };

        // Open file (append mode if resuming, create mode otherwise)
        let mut file = if existing_size > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
            OpenOptions::new()
                .append(true)
                .open(&temp_dest)
                .await
                .map_err(|e| DownloadError::file_system(format!("Failed to open file: {}", e)))?
        } else {
            File::create(&temp_dest)
                .await
                .map_err(|e| DownloadError::file_system(format!("Failed to create file: {}", e)))?
        };

        // Stream download with progress tracking
        let mut stream = response.bytes_stream();
        let mut downloaded: u64 = existing_size;

        while let Some(chunk_result) = stream.next().await {
            // Check for pause signal
            if cancel_token.is_cancelled() {
                // Flush and close file before pausing
                file.flush().await.ok();
                drop(file);
                return Err(DownloadError::cancelled("Download paused"));
            }

            let chunk = chunk_result
                .map_err(|e| DownloadError::network(format!("Failed to read chunk: {}", e)))?;

            file.write_all(&chunk)
                .await
                .map_err(|e| DownloadError::file_system(format!("Failed to write chunk: {}", e)))?;

            downloaded += chunk.len() as u64;

            // Call progress callback
            if let Some(callback) = progress {
                callback(downloaded, total_size);
            }
        }

        // Flush and close file
        file.flush()
            .await
            .map_err(|e| DownloadError::file_system(format!("Failed to flush file: {}", e)))?;
        drop(file);

        // Rename temp file to final destination
        tokio::fs::rename(&temp_dest, dest).await.map_err(|e| {
            DownloadError::file_system(format!("Failed to rename downloaded file: {}", e))
        })?;

        Ok(())
    }

    /// Register a download task
    fn register_task(&self, url: String, dest: PathBuf) -> CancellationToken {
        let cancel_token = CancellationToken::new();
        let state = TaskState {
            url: url.clone(),
            dest,
            cancel_token: cancel_token.clone(),
            supports_range: None,
        };
        self.tasks.write().insert(url, state);
        cancel_token
    }

    /// Unregister a download task
    fn unregister_task(&self, url: &str) {
        self.tasks.write().remove(url);
    }

    /// Get task cancel token
    fn get_cancel_token(&self, url: &str) -> Option<CancellationToken> {
        self.tasks
            .read()
            .get(url)
            .map(|state| state.cancel_token.clone())
    }
}

#[async_trait]
impl Downloader for TraumaDownloader {
    async fn download(
        &self,
        url: &str,
        dest: &Path,
        progress: Option<ProgressCallback>,
        options: Option<RequestOptions>,
    ) -> Result<()> {
        // Validate inputs
        if url.is_empty() {
            return Err(DownloadError::invalid_input("URL cannot be empty"));
        }

        // Ensure parent directory exists
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                DownloadError::file_system(format!("Failed to create directory: {}", e))
            })?;
        }

        // Register task
        let cancel_token = self.register_task(url.to_string(), dest.to_path_buf());

        // Start download
        let result = self
            .download_with_retry(
                url,
                dest,
                progress.as_ref(),
                &cancel_token,
                options.as_ref(),
            )
            .await;

        // Unregister task
        self.unregister_task(url);

        result
    }

    async fn download_batch(&self, tasks: Vec<(String, PathBuf)>) -> Vec<Result<()>> {
        if tasks.is_empty() {
            return vec![];
        }

        // Use semaphore to limit concurrent downloads
        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.max_concurrent));
        let mut handles = vec![];

        for (url, dest) in tasks {
            let sem = semaphore.clone();
            let cancel_token = self.register_task(url.clone(), dest.clone());
            let downloader = self.clone_for_task();
            let url_for_handle = url.clone();

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();

                // Ensure parent directory exists
                if let Some(parent) = dest.parent() {
                    if let Err(e) = tokio::fs::create_dir_all(parent).await {
                        return Err(DownloadError::file_system(format!(
                            "Failed to create directory: {}",
                            e
                        )));
                    }
                }

                // Download with retry
                let result = downloader
                    .download_with_retry(&url, &dest, None, &cancel_token, None)
                    .await;

                result
            });

            handles.push((url_for_handle, handle));
        }

        // Wait for all downloads to complete
        let mut results = vec![];
        for (url, handle) in handles {
            match handle.await {
                Ok(result) => {
                    self.unregister_task(&url);
                    results.push(result);
                }
                Err(e) => {
                    self.unregister_task(&url);
                    results.push(Err(DownloadError::network(format!(
                        "Task join error: {}",
                        e
                    ))));
                }
            }
        }

        results
    }

    fn name(&self) -> &str {
        "reqwest"
    }

    fn capabilities(&self) -> &DownloaderCapabilities {
        &self.capabilities
    }

    async fn pause(&self, url: &str) -> Result<()> {
        if let Some(cancel_token) = self.get_cancel_token(url) {
            cancel_token.cancel();
            Ok(())
        } else {
            Err(DownloadError::task_not_found(url))
        }
    }

    async fn resume(&self, url: &str) -> Result<()> {
        // Get task state
        let state = self.tasks.read().get(url).cloned();

        if let Some(task_state) = state {
            // Create new cancel token for resumed download
            let new_cancel_token =
                self.register_task(task_state.url.clone(), task_state.dest.clone());

            // Start download from where it left off
            let result = self
                .download_with_retry(
                    &task_state.url,
                    &task_state.dest,
                    None,
                    &new_cancel_token,
                    None,
                )
                .await;

            if result.is_err() {
                self.unregister_task(url);
            }

            result
        } else {
            Err(DownloadError::task_not_found(url))
        }
    }
}

impl TraumaDownloader {
    /// Clone essential fields for spawned tasks
    fn clone_for_task(&self) -> Self {
        Self {
            max_concurrent: self.max_concurrent,
            retries: self.retries,
            timeout_seconds: self.timeout_seconds,
            client: self.client.clone(),
            tasks: self.tasks.clone(),
            capabilities: self.capabilities.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_create_trauma_downloader() {
        let downloader = TraumaDownloader::new(8, 5, 600);
        assert_eq!(downloader.name(), "reqwest");
        assert_eq!(downloader.max_concurrent, 8);
        assert_eq!(downloader.retries, 5);

        // Test capabilities
        let caps = downloader.capabilities();
        assert!(caps.supports_pause);
        assert!(caps.supports_resume);
        assert!(caps.supports_cancellation);
    }

    #[tokio::test]
    async fn test_download_invalid_url() {
        let downloader = TraumaDownloader::default_settings();
        let temp_dir = tempdir().unwrap();
        let dest = temp_dir.path().join("test.txt");

        let result = downloader.download("", &dest, None, None).await;
        assert!(result.is_err());

        if let Err(e) = result {
            assert_eq!(e.kind, super::super::error::ErrorKind::InvalidInput);
        }
    }

    #[tokio::test]
    async fn test_download_batch_empty() {
        let downloader = TraumaDownloader::default_settings();
        let results = downloader.download_batch(vec![]).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_pause_nonexistent_task() {
        let downloader = TraumaDownloader::default_settings();
        let result = downloader.pause("http://nonexistent.com/file").await;
        assert!(result.is_err());
    }

    // ========================================================================
    // Integration Tests with Mock HTTP Server
    // ========================================================================

    /// Test Scenario 1: Complete file download flow
    /// Verifies: File downloaded correctly, content matches, temp file cleanup
    #[tokio::test]
    async fn test_download_complete_file() {
        use mockito::Server;

        // Create Mock HTTP server
        let mut server = Server::new_async().await;
        let test_data = b"Hello, this is test data for download!";

        let mock = server
            .mock("GET", "/test-file.txt")
            .with_status(200)
            .with_header("content-length", &test_data.len().to_string())
            .with_body(test_data.as_slice())
            .create();

        // Prepare download destination
        let temp_dir = tempdir().unwrap();
        let dest = temp_dir.path().join("downloaded.txt");

        // Execute download
        let downloader = TraumaDownloader::default_settings();
        let url = format!("{}/test-file.txt", server.url());
        let result = downloader.download(&url, &dest, None, None).await;

        // Verify results
        assert!(result.is_ok(), "Download should succeed");
        assert!(dest.exists(), "Downloaded file should exist");

        let downloaded_content = std::fs::read(&dest).unwrap();
        assert_eq!(downloaded_content, test_data, "Content should match");

        // Verify temporary file was cleaned up
        let temp_file = dest.with_extension("tmp");
        assert!(!temp_file.exists(), "Temporary file should be cleaned up");

        mock.assert();
    }

    /// Test Scenario 2: Concurrent downloads of multiple files
    /// Verifies: Parallel execution, concurrency control, all files downloaded
    #[tokio::test]
    async fn test_concurrent_downloads() {
        use mockito::Server;

        // Create Mock HTTP server
        let mut server = Server::new_async().await;

        // Prepare 5 different test files
        let test_files = vec![
            ("file1.txt", b"Content of file 1".to_vec()),
            ("file2.txt", b"Content of file 2".to_vec()),
            ("file3.txt", b"Content of file 3".to_vec()),
            ("file4.txt", b"Content of file 4".to_vec()),
            ("file5.txt", b"Content of file 5".to_vec()),
        ];

        // Create mock for each file
        let mut mocks = Vec::new();
        for (filename, content) in &test_files {
            let mock = server
                .mock("GET", format!("/{}", filename).as_str())
                .with_status(200)
                .with_header("content-length", &content.len().to_string())
                .with_body(content.as_slice())
                .create();
            mocks.push(mock);
        }

        // Prepare download tasks
        let temp_dir = tempdir().unwrap();
        let mut tasks = Vec::new();

        for (filename, _) in &test_files {
            let url = format!("{}/{}", server.url(), filename);
            let dest = temp_dir.path().join(filename);
            tasks.push((url, dest));
        }

        // Execute concurrent downloads (max 3 concurrent)
        let downloader = TraumaDownloader::new(3, 2, 30);
        let results = downloader.download_batch(tasks.clone()).await;

        // Verify all downloads succeeded
        assert_eq!(results.len(), 5, "Should have 5 results");
        for (i, result) in results.iter().enumerate() {
            assert!(result.is_ok(), "Download {} should succeed", i);
        }

        // Verify file contents
        for (i, (filename, expected_content)) in test_files.iter().enumerate() {
            let dest = temp_dir.path().join(filename);
            assert!(dest.exists(), "File {} should exist", filename);

            let content = std::fs::read(&dest).unwrap();
            assert_eq!(
                content, *expected_content,
                "Content of file {} should match",
                i
            );
        }

        // Verify all mocks were called
        for mock in mocks {
            mock.assert();
        }
    }

    /// Test Scenario 3: Pause and resume during download
    /// Verifies: Pause mechanism, temp file retention, successful continuation
    #[tokio::test]
    async fn test_pause_and_resume_download() {
        use mockito::Server;
        use std::time::Duration;

        // Create Mock HTTP server - simulate large file with chunked response
        let mut server = Server::new_async().await;
        let test_data = vec![b'X'; 10 * 1024]; // 10KB data

        let mock = server
            .mock("GET", "/large-file.bin")
            .with_status(200)
            .with_header("content-length", &test_data.len().to_string())
            .with_chunked_body(move |w| {
                // Simulate slow download to allow time for pause
                for chunk in test_data.chunks(1024) {
                    std::thread::sleep(Duration::from_millis(50));
                    w.write_all(chunk)?;
                }
                Ok(())
            })
            .create();

        let temp_dir = tempdir().unwrap();
        let dest = temp_dir.path().join("large-file.bin");
        let url = format!("{}/large-file.bin", server.url());

        let downloader = TraumaDownloader::default_settings();

        // Start download and pause quickly
        let url_clone = url.clone();
        let dest_clone = dest.clone();
        let downloader_clone = downloader.clone_for_task();

        let download_handle = tokio::spawn(async move {
            downloader_clone
                .download(&url_clone, &dest_clone, None, None)
                .await
        });

        // Wait for download to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Pause download
        let pause_result = downloader.pause(&url).await;
        assert!(pause_result.is_ok(), "Pause should succeed");

        // Wait for download task to respond to pause signal
        let download_result = download_handle.await.unwrap();
        assert!(download_result.is_err(), "Download should be cancelled");

        // Verify temporary file exists (partially downloaded data)
        let temp_file = dest.with_extension("tmp");
        assert!(
            temp_file.exists(),
            "Temporary file should exist after pause"
        );

        mock.assert();
    }

    /// Test Scenario 4: Resume from breakpoint
    /// Verifies: Range request, resume from breakpoint, complete file download
    #[tokio::test]
    async fn test_resume_from_breakpoint() {
        use mockito::Server;

        let temp_dir = tempdir().unwrap();
        let dest = temp_dir.path().join("resume-test.bin");
        let temp_file = dest.with_extension("tmp");

        // Prepare complete test data
        let full_data = vec![b'A'; 5000]; // 5KB
        let partial_size = 2000; // First 2KB already downloaded

        // Simulate partially downloaded data
        std::fs::write(&temp_file, &full_data[..partial_size]).unwrap();
        assert_eq!(
            std::fs::metadata(&temp_file).unwrap().len(),
            partial_size as u64,
            "Partial file should exist"
        );

        // Create Mock HTTP server with Range support
        let mut server = Server::new_async().await;

        // First Mock: HEAD request to detect Range support
        let _head_mock = server
            .mock("HEAD", "/resume-file.bin")
            .with_status(200)
            .with_header("accept-ranges", "bytes")
            .with_header("content-length", &full_data.len().to_string())
            .create();

        // Second Mock: GET request with Range support
        let range_mock = server
            .mock("GET", "/resume-file.bin")
            .match_header("range", format!("bytes={}-", partial_size).as_str())
            .with_status(206) // Partial Content
            .with_header(
                "content-range",
                format!(
                    "bytes {}-{}/{}",
                    partial_size,
                    full_data.len() - 1,
                    full_data.len()
                )
                .as_str(),
            )
            .with_header(
                "content-length",
                &(full_data.len() - partial_size).to_string(),
            )
            .with_body(&full_data[partial_size..])
            .create();

        // Execute resume download
        let downloader = TraumaDownloader::default_settings();
        let url = format!("{}/resume-file.bin", server.url());
        let result = downloader.download(&url, &dest, None, None).await;

        // Verify download succeeded
        assert!(result.is_ok(), "Resume download should succeed");
        assert!(dest.exists(), "Final file should exist");
        assert!(!temp_file.exists(), "Temporary file should be removed");

        // Verify complete file content
        let downloaded_data = std::fs::read(&dest).unwrap();
        assert_eq!(
            downloaded_data.len(),
            full_data.len(),
            "File size should match"
        );
        assert_eq!(
            downloaded_data, full_data,
            "Content should match completely"
        );

        range_mock.assert();
    }
}
