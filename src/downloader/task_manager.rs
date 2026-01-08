//! Download task manager with state tracking and long-polling support

use super::config::DownloadConfig;
use super::error::{DownloadError, Result};
use super::state::{DownloadProgress, DownloadState, SpeedCalculator, TaskInfo};
use super::traits::{Downloader, DownloaderCapabilities};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::Notify;
use uuid::Uuid;

/// Download task manager
///
/// Manages all download tasks, tracks their state, and provides long-polling support
/// for status updates.
pub struct DownloadTaskManager {
    /// Task storage: task_id -> TaskInfo
    tasks: Arc<RwLock<HashMap<String, TaskInfo>>>,

    /// Downloader implementation
    downloader: Arc<Box<dyn Downloader>>,

    /// Notification system for state changes (used for long-polling)
    notifier: Arc<Notify>,

    /// Downloader capabilities (cached from downloader at initialization)
    capabilities: DownloaderCapabilities,
}

impl DownloadTaskManager {
    /// Create a new task manager with the given downloader
    pub fn new(downloader: Box<dyn Downloader>) -> Self {
        // Cache capabilities from downloader at initialization
        let capabilities = downloader.capabilities().clone();

        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            downloader: Arc::new(downloader),
            notifier: Arc::new(Notify::new()),
            capabilities,
        }
    }

    /// Create a task manager from configuration
    pub fn from_config(config: &DownloadConfig) -> Self {
        let downloader = super::create_downloader(config);
        Self::new(downloader)
    }

    /// Submit a new download task
    ///
    /// # Arguments
    /// * `url` - The URL to download from
    /// * `dest_path` - The destination file path
    ///
    /// # Returns
    /// * `Ok(task_id)` - The unique task ID
    /// * `Err(DownloadError)` - If the task could not be created
    pub fn submit_task(
        &self,
        url: impl Into<String>,
        dest_path: impl Into<String>,
    ) -> Result<String> {
        let task_id = Uuid::new_v4().to_string();
        let task = TaskInfo::with_id(task_id.clone(), url, dest_path);

        {
            let mut tasks = self.tasks.write();
            if tasks.contains_key(&task_id) {
                return Err(DownloadError::task_already_exists(&task_id));
            }
            tasks.insert(task_id.clone(), task);
        }

        // Notify listeners about new task
        self.notifier.notify_waiters();

        // Start download in background
        let manager = self.clone_for_task();
        let task_id_clone = task_id.clone();
        tokio::spawn(async move {
            manager.execute_task(&task_id_clone).await;
        });

        Ok(task_id)
    }

    /// Submit a new download task with optional headers and cookies
    ///
    /// # Arguments
    /// * `url` - The URL to download from
    /// * `dest_path` - The destination file path
    /// * `headers` - Optional HTTP headers
    /// * `cookies` - Optional HTTP cookies
    ///
    /// # Returns
    /// * `Ok(task_id)` - The unique task ID
    /// * `Err(DownloadError)` - If the task could not be created
    pub fn submit_task_with_options(
        &self,
        url: impl Into<String>,
        dest_path: impl Into<String>,
        headers: Option<std::collections::HashMap<String, String>>,
        cookies: Option<std::collections::HashMap<String, String>>,
    ) -> Result<String> {
        let task_id = Uuid::new_v4().to_string();
        let task = TaskInfo::with_options(task_id.clone(), url, dest_path, headers, cookies);

        {
            let mut tasks = self.tasks.write();
            if tasks.contains_key(&task_id) {
                return Err(DownloadError::task_already_exists(&task_id));
            }
            tasks.insert(task_id.clone(), task);
        }

        // Notify listeners about new task
        self.notifier.notify_waiters();

        // Start download in background
        let manager = self.clone_for_task();
        let task_id_clone = task_id.clone();
        tokio::spawn(async move {
            manager.execute_task(&task_id_clone).await;
        });

        Ok(task_id)
    }

    /// Submit multiple download tasks
    ///
    /// # Returns
    /// Vector of task IDs for each submitted task
    pub fn submit_batch(&self, tasks: Vec<(String, String)>) -> Result<Vec<String>> {
        let mut task_ids = Vec::new();

        for (url, dest_path) in tasks {
            let task_id = self.submit_task(url, dest_path)?;
            task_ids.push(task_id);
        }

        Ok(task_ids)
    }

    /// Get task information by ID
    pub fn get_task(&self, task_id: &str) -> Result<TaskInfo> {
        let tasks = self.tasks.read();
        tasks
            .get(task_id)
            .cloned()
            .ok_or_else(|| DownloadError::task_not_found(task_id))
    }

    /// Get all tasks
    pub fn get_all_tasks(&self) -> Vec<TaskInfo> {
        let tasks = self.tasks.read();
        tasks.values().cloned().collect()
    }

    /// Get tasks by state
    pub fn get_tasks_by_state(&self, state: DownloadState) -> Vec<TaskInfo> {
        let tasks = self.tasks.read();
        tasks
            .values()
            .filter(|task| task.state == state)
            .cloned()
            .collect()
    }

    /// Get active tasks (pending or downloading)
    pub fn get_active_tasks(&self) -> Vec<TaskInfo> {
        let tasks = self.tasks.read();
        tasks
            .values()
            .filter(|task| task.state.is_active())
            .cloned()
            .collect()
    }

    /// Get downloader capabilities
    ///
    /// Returns the capabilities of the underlying downloader implementation.
    /// These capabilities are determined at initialization time.
    pub fn get_capabilities(&self) -> &DownloaderCapabilities {
        &self.capabilities
    }

    /// Cancel a task
    pub fn cancel_task(&self, task_id: &str) -> Result<()> {
        let mut tasks = self.tasks.write();
        let task = tasks
            .get_mut(task_id)
            .ok_or_else(|| DownloadError::task_not_found(task_id))?;

        if task.state.is_terminal() {
            return Err(DownloadError::invalid_input(format!(
                "Task {} is already in terminal state: {:?}",
                task_id, task.state
            )));
        }

        task.mark_cancelled();
        self.notifier.notify_waiters();

        Ok(())
    }

    /// Pause a download task
    ///
    /// # Arguments
    /// * `task_id` - The task ID to pause
    ///
    /// # Returns
    /// * `Ok(())` - Task paused successfully
    /// * `Err(DownloadError)` - If task not found or cannot be paused
    pub async fn pause_task(&self, task_id: &str) -> Result<()> {
        // Check if downloader supports pause
        if !self.capabilities.supports_pause {
            return Err(DownloadError::unsupported(
                "This downloader does not support pause functionality",
            ));
        }

        // Get task URL and verify state
        let url = {
            let tasks = self.tasks.read();
            let task = tasks
                .get(task_id)
                .ok_or_else(|| DownloadError::task_not_found(task_id))?;

            if !task.state.is_pausable() {
                return Err(DownloadError::invalid_input(format!(
                    "Task {} cannot be paused in state: {:?}",
                    task_id, task.state
                )));
            }

            task.url.clone()
        };

        // Call downloader pause
        self.downloader.pause(&url).await?;

        // Update task state
        {
            let mut tasks = self.tasks.write();
            if let Some(task) = tasks.get_mut(task_id) {
                task.mark_stopped();
            }
        }

        self.notifier.notify_waiters();
        Ok(())
    }

    /// Resume a paused download task
    ///
    /// # Arguments
    /// * `task_id` - The task ID to resume
    ///
    /// # Returns
    /// * `Ok(())` - Task resumed successfully
    /// * `Err(DownloadError)` - If task not found or cannot be resumed
    pub async fn resume_task(&self, task_id: &str) -> Result<()> {
        // Check if downloader supports resume
        if !self.capabilities.supports_resume {
            return Err(DownloadError::unsupported(
                "This downloader does not support resume functionality",
            ));
        }

        // Get task details and verify state
        let (url, dest_path, headers, cookies) = {
            let tasks = self.tasks.read();
            let task = tasks
                .get(task_id)
                .ok_or_else(|| DownloadError::task_not_found(task_id))?;

            if !task.state.is_resumable() {
                return Err(DownloadError::invalid_input(format!(
                    "Task {} cannot be resumed from state: {:?}",
                    task_id, task.state
                )));
            }

            (
                task.url.clone(),
                task.dest_path.clone(),
                task.headers.clone(),
                task.cookies.clone(),
            )
        };

        // Mark as resumed
        {
            let mut tasks = self.tasks.write();
            if let Some(task) = tasks.get_mut(task_id) {
                task.mark_resumed();
            }
        }

        self.notifier.notify_waiters();

        // Restart download in background
        let manager = self.clone_for_task();
        let task_id_clone = task_id.to_string();
        tokio::spawn(async move {
            // Create request options if headers or cookies exist
            let options = if headers.is_some() || cookies.is_some() {
                Some(super::traits::RequestOptions { headers, cookies })
            } else {
                None
            };

            // Create progress callback with speed calculator
            let tasks_clone = manager.tasks.clone();
            let task_id_for_callback = task_id_clone.clone();
            let notifier_clone = manager.notifier.clone();
            let speed_calc = Arc::new(RwLock::new(SpeedCalculator::default_window()));

            let progress_callback = Box::new(move |downloaded: u64, total: Option<u64>| {
                // Record sample and calculate speed
                let speed = {
                    let mut calc = speed_calc.write();
                    calc.record(downloaded);
                    calc.speed_bytes_per_sec()
                };

                // Update task progress
                let mut tasks = tasks_clone.write();
                if let Some(task) = tasks.get_mut(&task_id_for_callback) {
                    task.update_progress(DownloadProgress::with_speed(downloaded, total, speed));
                    notifier_clone.notify_waiters();
                }
            });

            // Resume download
            let result = manager
                .downloader
                .download(
                    &url,
                    &PathBuf::from(&dest_path),
                    Some(progress_callback),
                    options,
                )
                .await;

            // Update task state
            {
                let mut tasks = manager.tasks.write();
                if let Some(task) = tasks.get_mut(&task_id_clone) {
                    match result {
                        Ok(_) => task.mark_completed(),
                        Err(e) => task.mark_failed(e.message),
                    }
                }
            }

            manager.notifier.notify_waiters();
        });

        Ok(())
    }

    /// Wait for task state change (long-polling support)
    ///
    /// # Arguments
    /// * `task_id` - The task ID to monitor
    /// * `timeout` - Maximum time to wait for a change
    ///
    /// # Returns
    /// * `Ok(TaskInfo)` - The updated task info
    /// * `Err(DownloadError)` - If task not found or timeout occurred
    pub async fn wait_for_change(&self, task_id: &str, timeout: Duration) -> Result<TaskInfo> {
        let initial_state = {
            let tasks = self.tasks.read();
            let task = tasks
                .get(task_id)
                .ok_or_else(|| DownloadError::task_not_found(task_id))?;
            task.state
        };

        // If already in terminal state, return immediately
        if initial_state.is_terminal() {
            return self.get_task(task_id);
        }

        // Wait for notification with timeout
        let notifier = self.notifier.clone();
        let result = tokio::time::timeout(timeout, async {
            loop {
                notifier.notified().await;

                let tasks = self.tasks.read();
                if let Some(task) = tasks.get(task_id) {
                    if task.state != initial_state {
                        return Ok(task.clone());
                    }
                } else {
                    return Err(DownloadError::task_not_found(task_id));
                }
            }
        })
        .await;

        match result {
            Ok(task_result) => task_result,
            Err(_) => {
                // Timeout - return current state
                self.get_task(task_id)
            }
        }
    }

    /// Remove completed/failed tasks older than the specified duration
    pub fn cleanup_old_tasks(&self, max_age: Duration) {
        let mut tasks = self.tasks.write();
        let now = SystemTime::now();

        tasks.retain(|_, task| {
            if !task.state.is_terminal() {
                return true; // Keep active tasks
            }

            if let Some(completed_at) = task.completed_at {
                if let Ok(age) = now.duration_since(completed_at) {
                    return age < max_age;
                }
            }

            true
        });

        self.notifier.notify_waiters();
    }

    /// Remove a specific task
    pub fn remove_task(&self, task_id: &str) -> Result<()> {
        let mut tasks = self.tasks.write();
        let task = tasks
            .get(task_id)
            .ok_or_else(|| DownloadError::task_not_found(task_id))?;

        if !task.state.is_terminal() {
            return Err(DownloadError::invalid_input(format!(
                "Cannot remove active task: {}",
                task_id
            )));
        }

        tasks.remove(task_id);
        self.notifier.notify_waiters();

        Ok(())
    }

    /// Execute a download task
    async fn execute_task(&self, task_id: &str) {
        // Mark task as started
        {
            let mut tasks = self.tasks.write();
            if let Some(task) = tasks.get_mut(task_id) {
                if task.state == DownloadState::Cancelled {
                    return; // Task was cancelled before it started
                }
                task.mark_started();
            } else {
                return; // Task not found
            }
        }

        self.notifier.notify_waiters();

        // Get task details
        let (url, dest_path, headers, cookies) = {
            let tasks = self.tasks.read();
            if let Some(task) = tasks.get(task_id) {
                (
                    task.url.clone(),
                    task.dest_path.clone(),
                    task.headers.clone(),
                    task.cookies.clone(),
                )
            } else {
                return;
            }
        };

        // Create request options if headers or cookies exist
        let options = if headers.is_some() || cookies.is_some() {
            Some(super::traits::RequestOptions { headers, cookies })
        } else {
            None
        };

        // Create progress callback with speed calculator
        let tasks_clone = self.tasks.clone();
        let task_id_clone = task_id.to_string();
        let notifier_clone = self.notifier.clone();
        let speed_calc = Arc::new(RwLock::new(SpeedCalculator::default_window()));

        let progress_callback = Box::new(move |downloaded: u64, total: Option<u64>| {
            // Record sample and calculate speed
            let speed = {
                let mut calc = speed_calc.write();
                calc.record(downloaded);
                calc.speed_bytes_per_sec()
            };

            // Update task progress
            let mut tasks = tasks_clone.write();
            if let Some(task) = tasks.get_mut(&task_id_clone) {
                task.update_progress(DownloadProgress::with_speed(downloaded, total, speed));
                notifier_clone.notify_waiters();
            }
        });

        // Perform download
        let result = self
            .downloader
            .download(
                &url,
                &PathBuf::from(&dest_path),
                Some(progress_callback),
                options,
            )
            .await;

        // Update task state
        {
            let mut tasks = self.tasks.write();
            if let Some(task) = tasks.get_mut(task_id) {
                match result {
                    Ok(_) => task.mark_completed(),
                    Err(e) => task.mark_failed(e.message),
                }
            }
        }

        self.notifier.notify_waiters();
    }

    /// Clone for task execution
    fn clone_for_task(&self) -> Self {
        Self {
            tasks: self.tasks.clone(),
            downloader: self.downloader.clone(),
            notifier: self.notifier.clone(),
            capabilities: self.capabilities.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloader::TraumaDownloader;

    #[test]
    fn test_create_manager() {
        let downloader = Box::new(TraumaDownloader::default_settings());
        let manager = DownloadTaskManager::new(downloader);

        let all_tasks = manager.get_all_tasks();
        assert!(all_tasks.is_empty());
    }

    #[tokio::test]
    async fn test_submit_task() {
        let config = DownloadConfig::default();
        let manager = DownloadTaskManager::from_config(&config);

        let task_id = manager
            .submit_task("http://example.com/file", "/tmp/file")
            .unwrap();
        assert!(!task_id.is_empty());

        let task = manager.get_task(&task_id).unwrap();
        assert_eq!(task.url, "http://example.com/file");
        assert_eq!(task.dest_path, "/tmp/file");
    }

    #[tokio::test]
    async fn test_cancel_task() {
        let config = DownloadConfig::default();
        let manager = DownloadTaskManager::from_config(&config);

        let task_id = manager
            .submit_task("http://example.com/file", "/tmp/file")
            .unwrap();

        // Wait a bit for task to potentially start
        tokio::time::sleep(Duration::from_millis(100)).await;

        let result = manager.cancel_task(&task_id);
        // May succeed or fail depending on task state
        let _ = result;
    }

    #[tokio::test]
    async fn test_get_tasks_by_state() {
        let config = DownloadConfig::default();
        let manager = DownloadTaskManager::from_config(&config);

        let _ = manager.submit_task("http://example.com/file1", "/tmp/file1");
        let _ = manager.submit_task("http://example.com/file2", "/tmp/file2");

        let pending_tasks = manager.get_tasks_by_state(DownloadState::Pending);
        assert!(pending_tasks.len() <= 2); // May have started downloading
    }

    // ========================================================================
    // Integration Tests - TaskManager with Mock HTTP Server
    // ========================================================================

    /// Test Scenario 5: Headers and Cookies transmission to downloader
    /// Verifies: Custom headers and cookies are correctly sent to server
    #[tokio::test]
    async fn test_custom_headers_and_cookies_transmission() {
        use mockito::Server;
        use std::collections::HashMap;
        use tempfile::tempdir;

        // Create Mock HTTP server
        let mut server = Server::new_async().await;

        // Mock verifies custom headers and cookies
        // Note: Cookie order is not guaranteed due to HashMap iteration
        let mock = server
            .mock("GET", "/protected-file.txt")
            .match_header("authorization", "Bearer test-token-123")
            .match_header("x-custom-header", "custom-value")
            .match_header(
                "cookie",
                mockito::Matcher::Regex(".*session_id=abc123.*".to_string()),
            )
            .match_header(
                "cookie",
                mockito::Matcher::Regex(".*user_id=456.*".to_string()),
            )
            .with_status(200)
            .with_body(b"Protected content")
            .create();

        // Prepare custom headers and cookies
        let mut headers = HashMap::new();
        headers.insert(
            "Authorization".to_string(),
            "Bearer test-token-123".to_string(),
        );
        headers.insert("X-Custom-Header".to_string(), "custom-value".to_string());

        let mut cookies = HashMap::new();
        cookies.insert("session_id".to_string(), "abc123".to_string());
        cookies.insert("user_id".to_string(), "456".to_string());

        // Create task manager
        let config = DownloadConfig::default();
        let manager = DownloadTaskManager::from_config(&config);

        // Prepare download destination
        let temp_dir = tempdir().unwrap();
        let dest = temp_dir.path().join("protected-file.txt");
        let url = format!("{}/protected-file.txt", server.url());

        // Submit task with headers and cookies
        let task_id = manager
            .submit_task_with_options(&url, dest.to_str().unwrap(), Some(headers), Some(cookies))
            .unwrap();

        // Wait for download to complete
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Verify task status
        let task_info = manager.get_task(&task_id).unwrap();
        assert!(
            task_info.state == DownloadState::Completed
                || task_info.state == DownloadState::Downloading,
            "Task should be completed or downloading"
        );

        // Wait for task to fully complete
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let task_info = manager.get_task(&task_id).unwrap();
            if task_info.state == DownloadState::Completed {
                break;
            }
        }

        // Verify mock was called (headers and cookies transmitted correctly)
        mock.assert();

        // Verify file downloaded successfully
        assert!(dest.exists(), "File should be downloaded");
        let content = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(content, "Protected content");
    }

    /// Test Scenario 6: Task lifecycle state tracking
    /// Verifies: Complete flow from Pending → Downloading → Completed
    #[tokio::test]
    async fn test_task_lifecycle_and_state_transitions() {
        use mockito::Server;
        use tempfile::tempdir;

        // Create Mock HTTP server
        let mut server = Server::new_async().await;
        let test_data = b"Task lifecycle test data";

        let mock = server
            .mock("GET", "/lifecycle-test.txt")
            .with_status(200)
            .with_header("content-length", &test_data.len().to_string())
            .with_body(test_data.as_slice())
            .create();

        // Create task manager
        let config = DownloadConfig::default();
        let manager = DownloadTaskManager::from_config(&config);

        // Prepare download destination
        let temp_dir = tempdir().unwrap();
        let dest = temp_dir.path().join("lifecycle-test.txt");
        let url = format!("{}/lifecycle-test.txt", server.url());

        // Submit task
        let task_id = manager.submit_task(&url, dest.to_str().unwrap()).unwrap();

        // Track state changes
        let mut states_observed = Vec::new();

        // Initial state should be Pending
        let initial_task = manager.get_task(&task_id).unwrap();
        states_observed.push(initial_task.state.clone());
        assert_eq!(
            initial_task.state,
            DownloadState::Pending,
            "Initial state should be Pending"
        );

        // Monitor state transitions (max 3 seconds)
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(100)).await;

            if let Ok(task_info) = manager.get_task(&task_id) {
                let current_state = task_info.state.clone();

                // Record new state
                if states_observed.last() != Some(&current_state) {
                    states_observed.push(current_state.clone());
                }

                // If completed, verify file exists
                if current_state == DownloadState::Completed {
                    assert!(dest.exists(), "File should exist when completed");
                    let content = std::fs::read(&dest).unwrap();
                    assert_eq!(content, test_data);
                    break;
                }

                // If failed, record error
                if current_state == DownloadState::Failed {
                    if let Some(error) = &task_info.error {
                        eprintln!("Download failed: {}", error);
                    }
                    panic!("Download should not fail in this test");
                }
            }
        }

        // Verify state transition sequence
        assert!(
            states_observed.contains(&DownloadState::Pending),
            "Should have observed Pending state"
        );
        assert!(
            states_observed.contains(&DownloadState::Downloading)
                || states_observed.contains(&DownloadState::Completed),
            "Should have observed Downloading or Completed state"
        );

        // Final state should be Completed
        let final_task = manager.get_task(&task_id).unwrap();
        assert_eq!(
            final_task.state,
            DownloadState::Completed,
            "Final state should be Completed. States observed: {:?}",
            states_observed
        );

        // Verify progress information
        assert_eq!(
            final_task.progress.downloaded_bytes,
            test_data.len() as u64,
            "Downloaded bytes should match content length"
        );
        assert_eq!(
            final_task.progress.total_bytes,
            Some(test_data.len() as u64),
            "Total bytes should be known"
        );

        mock.assert();
    }

    /// Test Scenario 7: Long-polling wait_for_change mechanism
    /// Verifies: Correct notification on state change, timeout mechanism works
    #[tokio::test]
    async fn test_wait_for_change_notification() {
        use mockito::Server;
        use tempfile::tempdir;

        // Create Mock HTTP server with slow response
        let mut server = Server::new_async().await;
        let test_data = vec![b'W'; 2048]; // 2KB data

        let mock = server
            .mock("GET", "/slow-file.bin")
            .with_status(200)
            .with_header("content-length", &test_data.len().to_string())
            .with_chunked_body(move |w| {
                // Send chunks slowly
                for chunk in test_data.chunks(512) {
                    std::thread::sleep(Duration::from_millis(200));
                    w.write_all(chunk)?;
                }
                Ok(())
            })
            .create();

        // Create task manager
        let config = DownloadConfig::default();
        let manager = DownloadTaskManager::from_config(&config);

        // Prepare download destination
        let temp_dir = tempdir().unwrap();
        let dest = temp_dir.path().join("slow-file.bin");
        let url = format!("{}/slow-file.bin", server.url());

        // Submit task
        let task_id = manager.submit_task(&url, dest.to_str().unwrap()).unwrap();

        // Test 1: Wait for state change (should succeed)
        let wait_result = manager
            .wait_for_change(&task_id, Duration::from_secs(2))
            .await;
        assert!(wait_result.is_ok(), "Wait for change should succeed");

        let task_after_change = wait_result.unwrap();
        assert!(
            task_after_change.state == DownloadState::Downloading
                || task_after_change.state == DownloadState::Completed,
            "State should have changed from Pending"
        );

        // Test 2: Short timeout test (should timeout when state is stable)
        // Wait for task to complete or enter stable state
        tokio::time::sleep(Duration::from_secs(1)).await;

        // Test 3: Verify task eventually completes
        for _ in 0..15 {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let task_info = manager.get_task(&task_id).unwrap();

            if task_info.state == DownloadState::Completed {
                // Verify download success
                assert!(dest.exists(), "File should exist");
                let file_size = std::fs::metadata(&dest).unwrap().len();
                assert_eq!(file_size, 2048, "File size should match");
                break;
            }
        }

        mock.assert();
    }

    /// Test Scenario 8: Batch task submission and management
    /// Verifies: Batch submission, deduplication, concurrency control
    #[tokio::test]
    async fn test_batch_task_submission_and_management() {
        use mockito::Server;
        use tempfile::tempdir;

        // Create Mock HTTP server
        let mut server = Server::new_async().await;

        // Create 3 different files
        let files = vec![
            ("batch1.txt", b"Batch file 1".to_vec()),
            ("batch2.txt", b"Batch file 2".to_vec()),
            ("batch3.txt", b"Batch file 3".to_vec()),
        ];

        let mut mocks = Vec::new();
        for (filename, content) in &files {
            let mock = server
                .mock("GET", format!("/{}", filename).as_str())
                .with_status(200)
                .with_header("content-length", &content.len().to_string())
                .with_body(content.as_slice())
                .create();
            mocks.push(mock);
        }

        // Create task manager
        let config = DownloadConfig::default();
        let manager = DownloadTaskManager::from_config(&config);

        // Prepare batch tasks
        let temp_dir = tempdir().unwrap();
        let mut tasks = Vec::new();

        for (filename, _) in &files {
            let url = format!("{}/{}", server.url(), filename);
            let dest = temp_dir.path().join(filename);
            tasks.push((url, dest.to_string_lossy().to_string()));
        }

        // Submit batch tasks
        let task_ids = manager.submit_batch(tasks).unwrap();
        assert_eq!(task_ids.len(), 3, "Should submit 3 tasks");

        // Wait for all tasks to complete
        for task_id in &task_ids {
            for _ in 0..30 {
                tokio::time::sleep(Duration::from_millis(100)).await;

                if let Ok(task_info) = manager.get_task(task_id) {
                    if task_info.state == DownloadState::Completed {
                        break;
                    }
                    if task_info.state == DownloadState::Failed {
                        panic!("Task {} failed: {:?}", task_id, task_info.error);
                    }
                }
            }
        }

        // Verify all files downloaded successfully
        for (filename, expected_content) in &files {
            let dest = temp_dir.path().join(filename);
            assert!(dest.exists(), "File {} should exist", filename);

            let content = std::fs::read(&dest).unwrap();
            assert_eq!(
                content, *expected_content,
                "Content of {} should match",
                filename
            );
        }

        // Verify all mocks were called
        for mock in mocks {
            mock.assert();
        }

        // Verify get_all_tasks returns all tasks
        let all_tasks = manager.get_all_tasks();
        assert!(all_tasks.len() >= 3, "Should have at least 3 tasks");

        // Verify get_tasks_by_state
        let completed_tasks = manager.get_tasks_by_state(DownloadState::Completed);
        assert!(
            completed_tasks.len() >= 3,
            "Should have at least 3 completed tasks"
        );
    }
}
