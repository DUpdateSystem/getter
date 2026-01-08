//! Download state and progress tracking types

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::VecDeque;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Speed calculator using sliding window for smooth speed measurement
///
/// Keeps track of download samples over a time window and calculates
/// average speed to avoid fluctuations.
#[derive(Debug, Clone)]
pub struct SpeedCalculator {
    /// Samples: (timestamp, downloaded_bytes)
    samples: VecDeque<(Instant, u64)>,
    /// Window size in seconds
    window_secs: u64,
    /// Maximum number of samples to keep
    max_samples: usize,
    /// Start time for calculating speed when only one sample exists
    start_time: Option<Instant>,
}

impl SpeedCalculator {
    /// Create a new speed calculator with specified window size
    pub fn new(window_secs: u64) -> Self {
        Self {
            samples: VecDeque::with_capacity(64),
            window_secs,
            max_samples: 64,
            start_time: None,
        }
    }

    /// Create with default 5-second window
    pub fn default_window() -> Self {
        Self::new(5)
    }

    /// Record a new sample
    pub fn record(&mut self, downloaded_bytes: u64) {
        let now = Instant::now();

        // Initialize start time on first record
        if self.start_time.is_none() {
            self.start_time = Some(now);
        }

        // Remove samples outside the window
        let cutoff = now - std::time::Duration::from_secs(self.window_secs);
        while let Some((time, _)) = self.samples.front() {
            if *time < cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }

        // Add new sample
        self.samples.push_back((now, downloaded_bytes));

        // Limit max samples
        while self.samples.len() > self.max_samples {
            self.samples.pop_front();
        }
    }

    /// Calculate average speed in bytes per second
    pub fn speed_bytes_per_sec(&self) -> Option<u64> {
        let (last_time, last_bytes) = self.samples.back()?;

        // Use sliding window if we have multiple samples
        if self.samples.len() >= 2 {
            let (first_time, first_bytes) = self.samples.front()?;
            let duration = last_time.duration_since(*first_time);
            let duration_secs = duration.as_secs_f64().max(0.001);
            let bytes_diff = last_bytes.saturating_sub(*first_bytes);
            return Some((bytes_diff as f64 / duration_secs) as u64);
        }

        // For single sample, calculate from start time
        if let Some(start) = self.start_time {
            let duration = last_time.duration_since(start);
            let duration_secs = duration.as_secs_f64().max(0.001);
            return Some((*last_bytes as f64 / duration_secs) as u64);
        }

        None
    }

    /// Reset the calculator
    pub fn reset(&mut self) {
        self.samples.clear();
        self.start_time = None;
    }
}

/// Serialize SystemTime as Unix timestamp in milliseconds
fn serialize_system_time<S>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let millis = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    serializer.serialize_i64(millis)
}

/// Serialize Option<SystemTime> as Unix timestamp in milliseconds
fn serialize_option_system_time<S>(
    time: &Option<SystemTime>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match time {
        Some(t) => {
            let millis = t
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            serializer.serialize_some(&millis)
        }
        None => serializer.serialize_none(),
    }
}

/// Deserialize Unix timestamp in milliseconds to SystemTime
fn deserialize_system_time<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
where
    D: Deserializer<'de>,
{
    let millis = i64::deserialize(deserializer)?;
    Ok(UNIX_EPOCH + std::time::Duration::from_millis(millis as u64))
}

/// Deserialize Unix timestamp in milliseconds to Option<SystemTime>
fn deserialize_option_system_time<'de, D>(deserializer: D) -> Result<Option<SystemTime>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<i64> = Option::deserialize(deserializer)?;
    Ok(opt.map(|millis| UNIX_EPOCH + std::time::Duration::from_millis(millis as u64)))
}

/// Download task state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadState {
    /// Task is queued but not started yet
    Pending,
    /// Download is in progress
    Downloading,
    /// Download is paused (can be resumed)
    Stopped,
    /// Download completed successfully
    Completed,
    /// Download failed
    Failed,
    /// Download was cancelled
    Cancelled,
}

impl DownloadState {
    /// Check if the task is in a terminal state (completed, failed, or cancelled)
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            DownloadState::Completed | DownloadState::Failed | DownloadState::Cancelled
        )
    }

    /// Check if the task is active (pending, downloading, or stopped/paused)
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            DownloadState::Pending | DownloadState::Downloading | DownloadState::Stopped
        )
    }

    /// Check if the task can be resumed
    pub fn is_resumable(&self) -> bool {
        matches!(self, DownloadState::Stopped | DownloadState::Failed)
    }

    /// Check if the task can be paused
    pub fn is_pausable(&self) -> bool {
        matches!(self, DownloadState::Downloading)
    }
}

/// Download progress information
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DownloadProgress {
    /// Number of bytes downloaded so far
    pub downloaded_bytes: u64,

    /// Total size in bytes (None if unknown)
    pub total_bytes: Option<u64>,

    /// Download speed in bytes per second (average)
    pub speed_bytes_per_sec: Option<u64>,

    /// Estimated time remaining in seconds (None if unknown)
    pub eta_seconds: Option<u64>,
}

impl DownloadProgress {
    /// Create a new progress with downloaded bytes
    pub fn new(downloaded_bytes: u64, total_bytes: Option<u64>) -> Self {
        Self {
            downloaded_bytes,
            total_bytes,
            speed_bytes_per_sec: None,
            eta_seconds: None,
        }
    }

    /// Create a new progress with speed and ETA
    pub fn with_speed(
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
        speed_bytes_per_sec: Option<u64>,
    ) -> Self {
        let eta_seconds = match (total_bytes, speed_bytes_per_sec) {
            (Some(total), Some(speed)) if speed > 0 && total > downloaded_bytes => {
                Some((total - downloaded_bytes) / speed)
            }
            _ => None,
        };

        Self {
            downloaded_bytes,
            total_bytes,
            speed_bytes_per_sec,
            eta_seconds,
        }
    }

    /// Calculate progress percentage (0-100)
    pub fn percentage(&self) -> Option<f64> {
        self.total_bytes.map(|total| {
            if total == 0 {
                0.0
            } else {
                (self.downloaded_bytes as f64 / total as f64) * 100.0
            }
        })
    }

    /// Check if download is complete
    pub fn is_complete(&self) -> bool {
        if let Some(total) = self.total_bytes {
            self.downloaded_bytes >= total
        } else {
            false
        }
    }
}

/// Complete information about a download task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    /// Unique task identifier
    pub task_id: String,

    /// Source URL
    pub url: String,

    /// Destination file path
    pub dest_path: String,

    /// Current state
    pub state: DownloadState,

    /// Progress information
    pub progress: DownloadProgress,

    /// Resume offset in bytes (for resuming paused downloads)
    pub resume_offset: u64,

    /// Whether the server supports HTTP Range requests (None if not tested yet)
    pub supports_range: Option<bool>,

    /// Error message (if state is Failed)
    pub error: Option<String>,

    /// Task creation timestamp (Unix timestamp in milliseconds)
    #[serde(
        serialize_with = "serialize_system_time",
        deserialize_with = "deserialize_system_time"
    )]
    pub created_at: SystemTime,

    /// Task start timestamp (Unix timestamp in milliseconds, None if not started yet)
    #[serde(
        serialize_with = "serialize_option_system_time",
        deserialize_with = "deserialize_option_system_time"
    )]
    pub started_at: Option<SystemTime>,

    /// Task completion timestamp (Unix timestamp in milliseconds, None if not completed)
    #[serde(
        serialize_with = "serialize_option_system_time",
        deserialize_with = "deserialize_option_system_time"
    )]
    pub completed_at: Option<SystemTime>,

    /// Task last paused timestamp (Unix timestamp in milliseconds, None if never paused)
    #[serde(
        serialize_with = "serialize_option_system_time",
        deserialize_with = "deserialize_option_system_time"
    )]
    pub paused_at: Option<SystemTime>,

    /// HTTP headers to include in requests
    #[serde(default)]
    pub headers: Option<std::collections::HashMap<String, String>>,

    /// HTTP cookies to include in requests
    #[serde(default)]
    pub cookies: Option<std::collections::HashMap<String, String>>,
}

impl TaskInfo {
    /// Create a new task info
    pub fn new(url: impl Into<String>, dest_path: impl Into<String>) -> Self {
        Self {
            task_id: Uuid::new_v4().to_string(),
            url: url.into(),
            dest_path: dest_path.into(),
            state: DownloadState::Pending,
            progress: DownloadProgress::default(),
            resume_offset: 0,
            supports_range: None,
            error: None,
            created_at: SystemTime::now(),
            started_at: None,
            completed_at: None,
            paused_at: None,
            headers: None,
            cookies: None,
        }
    }

    /// Create with a specific task ID
    pub fn with_id(
        task_id: impl Into<String>,
        url: impl Into<String>,
        dest_path: impl Into<String>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            url: url.into(),
            dest_path: dest_path.into(),
            state: DownloadState::Pending,
            progress: DownloadProgress::default(),
            resume_offset: 0,
            supports_range: None,
            error: None,
            created_at: SystemTime::now(),
            started_at: None,
            completed_at: None,
            paused_at: None,
            headers: None,
            cookies: None,
        }
    }

    /// Create with headers and cookies
    pub fn with_options(
        task_id: impl Into<String>,
        url: impl Into<String>,
        dest_path: impl Into<String>,
        headers: Option<std::collections::HashMap<String, String>>,
        cookies: Option<std::collections::HashMap<String, String>>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            url: url.into(),
            dest_path: dest_path.into(),
            state: DownloadState::Pending,
            progress: DownloadProgress::default(),
            resume_offset: 0,
            supports_range: None,
            error: None,
            created_at: SystemTime::now(),
            started_at: None,
            completed_at: None,
            paused_at: None,
            headers,
            cookies,
        }
    }

    /// Mark task as started
    pub fn mark_started(&mut self) {
        self.state = DownloadState::Downloading;
        self.started_at = Some(SystemTime::now());
    }

    /// Mark task as completed
    pub fn mark_completed(&mut self) {
        self.state = DownloadState::Completed;
        self.completed_at = Some(SystemTime::now());
    }

    /// Mark task as failed with error message
    pub fn mark_failed(&mut self, error: impl Into<String>) {
        self.state = DownloadState::Failed;
        self.error = Some(error.into());
        self.completed_at = Some(SystemTime::now());
    }

    /// Mark task as cancelled
    pub fn mark_cancelled(&mut self) {
        self.state = DownloadState::Cancelled;
        self.completed_at = Some(SystemTime::now());
    }

    /// Mark task as stopped/paused
    pub fn mark_stopped(&mut self) {
        self.state = DownloadState::Stopped;
        self.resume_offset = self.progress.downloaded_bytes;
        self.paused_at = Some(SystemTime::now());
    }

    /// Mark task as resumed from stopped state
    pub fn mark_resumed(&mut self) {
        self.state = DownloadState::Downloading;
        self.paused_at = None;
    }

    /// Update progress
    pub fn update_progress(&mut self, progress: DownloadProgress) {
        self.progress = progress;
    }

    /// Set whether the server supports Range requests
    pub fn set_range_support(&mut self, supports: bool) {
        self.supports_range = Some(supports);
    }

    /// Calculate elapsed time in seconds
    pub fn elapsed_seconds(&self) -> Option<u64> {
        self.started_at.and_then(|start| {
            SystemTime::now()
                .duration_since(start)
                .ok()
                .map(|d| d.as_secs())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_download_state() {
        assert!(DownloadState::Completed.is_terminal());
        assert!(DownloadState::Failed.is_terminal());
        assert!(DownloadState::Cancelled.is_terminal());
        assert!(!DownloadState::Pending.is_terminal());
        assert!(!DownloadState::Downloading.is_terminal());
        assert!(!DownloadState::Stopped.is_terminal());

        assert!(DownloadState::Pending.is_active());
        assert!(DownloadState::Downloading.is_active());
        assert!(DownloadState::Stopped.is_active());
        assert!(!DownloadState::Completed.is_active());

        assert!(DownloadState::Downloading.is_pausable());
        assert!(!DownloadState::Stopped.is_pausable());
        assert!(!DownloadState::Completed.is_pausable());

        assert!(DownloadState::Stopped.is_resumable());
        assert!(DownloadState::Failed.is_resumable());
        assert!(!DownloadState::Downloading.is_resumable());
        assert!(!DownloadState::Completed.is_resumable());
    }

    #[test]
    fn test_download_progress() {
        let mut progress = DownloadProgress::new(50, Some(100));
        assert_eq!(progress.percentage(), Some(50.0));
        assert!(!progress.is_complete());

        progress.downloaded_bytes = 100;
        assert_eq!(progress.percentage(), Some(100.0));
        assert!(progress.is_complete());
    }

    #[test]
    fn test_task_info() {
        let mut task = TaskInfo::new("http://example.com/file", "/tmp/file");
        assert_eq!(task.state, DownloadState::Pending);
        assert!(task.started_at.is_none());
        assert!(task.completed_at.is_none());

        task.mark_started();
        assert_eq!(task.state, DownloadState::Downloading);
        assert!(task.started_at.is_some());

        task.mark_completed();
        assert_eq!(task.state, DownloadState::Completed);
        assert!(task.completed_at.is_some());
    }

    #[test]
    fn test_task_info_failure() {
        let mut task = TaskInfo::new("http://example.com/file", "/tmp/file");
        task.mark_failed("Network error");

        assert_eq!(task.state, DownloadState::Failed);
        assert_eq!(task.error, Some("Network error".to_string()));
        assert!(task.completed_at.is_some());
    }
}
