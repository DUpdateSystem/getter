//! Downloader module providing pluggable download functionality
//!
//! This module implements a flexible, trait-based downloader system that supports:
//! - Multiple backend implementations (trauma, reqwest, custom CLI, etc.)
//! - Task state management with progress tracking
//! - Long-polling for download status updates
//! - Configuration-driven backend selection
//! - JSON-RPC integration for remote control

mod config;
mod error;
mod state;
mod task_manager;
mod traits;
mod trauma_impl;

pub use config::{DownloadConfig, DownloaderBackend};
pub use error::{DownloadError, Result};
pub use state::{DownloadProgress, DownloadState, SpeedCalculator, TaskInfo};
pub use task_manager::DownloadTaskManager;
pub use traits::Downloader;
pub use trauma_impl::TraumaDownloader;

/// Create a downloader instance based on the provided configuration
pub fn create_downloader(config: &DownloadConfig) -> Box<dyn Downloader> {
    match config.backend {
        DownloaderBackend::Trauma => Box::new(TraumaDownloader::new(
            config.max_concurrent,
            config.retries,
            config.timeout_seconds,
        )),
        // Future implementations can be added here
        // DownloaderBackend::Reqwest => Box::new(ReqwestDownloader::new(config)),
        // DownloaderBackend::Custom => Box::new(CliDownloader::new(config)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_trauma_downloader() {
        let config = DownloadConfig::default();
        let downloader = create_downloader(&config);
        assert_eq!(downloader.name(), "reqwest");

        // Test that capabilities are accessible
        let caps = downloader.capabilities();
        assert!(caps.supports_pause);
        assert!(caps.supports_resume);
        assert!(caps.supports_cancellation);
    }
}
