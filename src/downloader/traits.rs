//! Core trait definitions for the downloader system

use super::error::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Progress callback function type
/// Parameters: (downloaded_bytes, total_bytes_optional)
pub type ProgressCallback = Box<dyn Fn(u64, Option<u64>) + Send + Sync>;

/// HTTP request options for downloads
#[derive(Debug, Clone, Default)]
pub struct RequestOptions {
    /// HTTP headers to include in the request
    pub headers: Option<HashMap<String, String>>,
    /// HTTP cookies to include in the request
    pub cookies: Option<HashMap<String, String>>,
}

/// Downloader capability information
///
/// This struct defines what features a downloader implementation supports.
/// These capabilities are determined at initialization time and remain constant
/// throughout the downloader's lifetime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DownloaderCapabilities {
    /// Whether the downloader supports pausing downloads
    pub supports_pause: bool,
    /// Whether the downloader supports resuming paused downloads
    pub supports_resume: bool,
    /// Whether the downloader supports cancelling downloads
    pub supports_cancellation: bool,
    /// Whether the downloader supports HTTP Range requests for breakpoint resume
    pub supports_range_requests: bool,
    /// Whether the downloader supports batch download operations
    pub supports_batch_download: bool,
}

impl Default for DownloaderCapabilities {
    fn default() -> Self {
        Self {
            supports_pause: false,
            supports_resume: false,
            supports_cancellation: false,
            supports_range_requests: false,
            supports_batch_download: false,
        }
    }
}

impl DownloaderCapabilities {
    /// Create a new DownloaderCapabilities with all features enabled
    pub fn all_enabled() -> Self {
        Self {
            supports_pause: true,
            supports_resume: true,
            supports_cancellation: true,
            supports_range_requests: true,
            supports_batch_download: true,
        }
    }

    /// Create a new DownloaderCapabilities with all features disabled
    pub fn all_disabled() -> Self {
        Self::default()
    }
}

/// Core downloader trait - all downloader implementations must implement this
///
/// This trait provides a pluggable interface for different download backends,
/// allowing easy switching between implementations (trauma, reqwest, CLI tools, etc.)
#[async_trait]
pub trait Downloader: Send + Sync {
    /// Download a single file from URL to destination path
    ///
    /// # Arguments
    /// * `url` - The URL to download from
    /// * `dest` - The destination file path
    /// * `progress` - Optional progress callback (downloaded_bytes, total_bytes)
    /// * `options` - Optional request options (headers, cookies)
    ///
    /// # Returns
    /// * `Ok(())` - Download completed successfully
    /// * `Err(DownloadError)` - Download failed
    async fn download(
        &self,
        url: &str,
        dest: &Path,
        progress: Option<ProgressCallback>,
        options: Option<RequestOptions>,
    ) -> Result<()>;

    /// Download multiple files concurrently
    ///
    /// # Arguments
    /// * `tasks` - Vector of (url, destination_path) tuples
    ///
    /// # Returns
    /// * `Ok(Vec<Result<()>>)` - Vector of results for each download task
    async fn download_batch(&self, tasks: Vec<(String, std::path::PathBuf)>) -> Vec<Result<()>>;

    /// Get the name of this downloader implementation
    fn name(&self) -> &str;

    /// Get the capabilities of this downloader implementation
    ///
    /// This method returns a reference to the capability information that was
    /// determined at initialization time. Capabilities define what features
    /// the downloader supports (pause, resume, cancellation, etc.).
    ///
    /// # Returns
    /// A reference to the DownloaderCapabilities struct
    fn capabilities(&self) -> &DownloaderCapabilities;

    /// Cancel an ongoing download (if supported by the implementation)
    ///
    /// Default implementation does nothing and returns Ok
    async fn cancel(&self, _url: &str) -> Result<()> {
        Ok(())
    }

    /// Pause an ongoing download (if supported by the implementation)
    ///
    /// # Arguments
    /// * `url` - The URL of the download to pause
    ///
    /// # Returns
    /// * `Ok(())` - Download paused successfully
    /// * `Err(DownloadError)` - Failed to pause download
    ///
    /// Default implementation returns an error
    async fn pause(&self, _url: &str) -> Result<()> {
        Err(super::error::DownloadError::unsupported(
            "Pause not supported by this downloader",
        ))
    }

    /// Resume a paused download (if supported by the implementation)
    ///
    /// # Arguments
    /// * `url` - The URL of the download to resume
    ///
    /// # Returns
    /// * `Ok(())` - Download resumed successfully
    /// * `Err(DownloadError)` - Failed to resume download
    ///
    /// Default implementation returns an error
    async fn resume(&self, _url: &str) -> Result<()> {
        Err(super::error::DownloadError::unsupported(
            "Resume not supported by this downloader",
        ))
    }

    /// Check if this downloader supports cancellation
    ///
    /// # Deprecated
    /// Use `capabilities().supports_cancellation` instead.
    /// This method will be removed in a future version.
    #[deprecated(
        since = "0.2.0",
        note = "Use capabilities().supports_cancellation instead"
    )]
    fn supports_cancellation(&self) -> bool {
        self.capabilities().supports_cancellation
    }

    /// Check if this downloader supports pause/resume
    ///
    /// # Deprecated
    /// Use `capabilities().supports_pause` instead.
    /// This method will be removed in a future version.
    #[deprecated(since = "0.2.0", note = "Use capabilities().supports_pause instead")]
    fn supports_pause(&self) -> bool {
        self.capabilities().supports_pause
    }

    /// Check if this downloader supports resume/partial downloads
    ///
    /// # Deprecated
    /// Use `capabilities().supports_resume` instead.
    /// This method will be removed in a future version.
    #[deprecated(since = "0.2.0", note = "Use capabilities().supports_resume instead")]
    fn supports_resume(&self) -> bool {
        self.capabilities().supports_resume
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // Mock downloader for testing
    struct MockDownloader {
        capabilities: DownloaderCapabilities,
    }

    impl MockDownloader {
        fn new() -> Self {
            Self {
                capabilities: DownloaderCapabilities::all_disabled(),
            }
        }
    }

    #[async_trait]
    impl Downloader for MockDownloader {
        async fn download(
            &self,
            _url: &str,
            _dest: &Path,
            _progress: Option<ProgressCallback>,
            _options: Option<RequestOptions>,
        ) -> Result<()> {
            Ok(())
        }

        async fn download_batch(&self, tasks: Vec<(String, PathBuf)>) -> Vec<Result<()>> {
            tasks.into_iter().map(|_| Ok(())).collect()
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn capabilities(&self) -> &DownloaderCapabilities {
            &self.capabilities
        }
    }

    #[tokio::test]
    async fn test_mock_downloader() {
        let downloader = MockDownloader::new();
        assert_eq!(downloader.name(), "mock");

        // Test new capabilities() method
        let caps = downloader.capabilities();
        assert!(!caps.supports_cancellation);
        assert!(!caps.supports_pause);
        assert!(!caps.supports_resume);
        assert!(!caps.supports_range_requests);
        assert!(!caps.supports_batch_download);

        // Test deprecated methods (should still work)
        #[allow(deprecated)]
        {
            assert!(!downloader.supports_cancellation());
            assert!(!downloader.supports_pause());
            assert!(!downloader.supports_resume());
        }

        let result = downloader
            .download(
                "http://example.com/file",
                Path::new("/tmp/file"),
                None,
                None,
            )
            .await;
        assert!(result.is_ok());

        // Test default pause/resume implementations
        let pause_result = downloader.pause("http://example.com/file").await;
        assert!(pause_result.is_err());

        let resume_result = downloader.resume("http://example.com/file").await;
        assert!(resume_result.is_err());
    }
}
