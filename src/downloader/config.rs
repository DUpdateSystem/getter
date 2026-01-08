//! Configuration system for the downloader module

use serde::{Deserialize, Serialize};

/// Downloader backend selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloaderBackend {
    /// Use trauma downloader (default)
    Trauma,
    // Future backends can be added here:
    // /// Use reqwest downloader
    // Reqwest,
    // /// Use custom CLI command
    // Custom,
}

impl Default for DownloaderBackend {
    fn default() -> Self {
        Self::Trauma
    }
}

/// Download configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadConfig {
    /// Downloader backend to use
    pub backend: DownloaderBackend,

    /// Maximum number of concurrent downloads
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,

    /// Number of retry attempts for failed downloads
    #[serde(default = "default_retries")]
    pub retries: usize,

    /// Timeout for each download in seconds
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,

    /// Custom command template for CLI downloader (future use)
    /// Example: "wget -O \"${FILE}\" \"${URI}\""
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_command: Option<String>,
}

fn default_max_concurrent() -> usize {
    4
}

fn default_retries() -> usize {
    3
}

fn default_timeout() -> u64 {
    300 // 5 minutes
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            backend: DownloaderBackend::default(),
            max_concurrent: default_max_concurrent(),
            retries: default_retries(),
            timeout_seconds: default_timeout(),
            custom_command: None,
        }
    }
}

impl DownloadConfig {
    /// Create a new configuration with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the backend
    pub fn with_backend(mut self, backend: DownloaderBackend) -> Self {
        self.backend = backend;
        self
    }

    /// Set max concurrent downloads
    pub fn with_max_concurrent(mut self, max: usize) -> Self {
        self.max_concurrent = max;
        self
    }

    /// Set retry count
    pub fn with_retries(mut self, retries: usize) -> Self {
        self.retries = retries;
        self
    }

    /// Set timeout in seconds
    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = seconds;
        self
    }

    /// Set custom command template
    pub fn with_custom_command(mut self, command: impl Into<String>) -> Self {
        self.custom_command = Some(command.into());
        self
    }

    /// Load configuration from environment variables
    ///
    /// Supported environment variables:
    /// - DOWNLOADER_BACKEND: "trauma" (default)
    /// - DOWNLOADER_MAX_CONCURRENT: number (default: 4)
    /// - DOWNLOADER_RETRIES: number (default: 3)
    /// - DOWNLOADER_TIMEOUT: seconds (default: 300)
    /// - FETCHCOMMAND: custom download command (like Portage's FETCHCOMMAND)
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(backend) = std::env::var("DOWNLOADER_BACKEND") {
            config.backend = match backend.to_lowercase().as_str() {
                "trauma" => DownloaderBackend::Trauma,
                _ => {
                    eprintln!("Unknown DOWNLOADER_BACKEND: {}, using default", backend);
                    DownloaderBackend::Trauma
                }
            };
        }

        if let Ok(max_concurrent) = std::env::var("DOWNLOADER_MAX_CONCURRENT") {
            if let Ok(max) = max_concurrent.parse() {
                config.max_concurrent = max;
            }
        }

        if let Ok(retries) = std::env::var("DOWNLOADER_RETRIES") {
            if let Ok(retry_count) = retries.parse() {
                config.retries = retry_count;
            }
        }

        if let Ok(timeout) = std::env::var("DOWNLOADER_TIMEOUT") {
            if let Ok(timeout_secs) = timeout.parse() {
                config.timeout_seconds = timeout_secs;
            }
        }

        if let Ok(command) = std::env::var("FETCHCOMMAND") {
            config.custom_command = Some(command);
        }

        config
    }

    /// Load configuration from JSON string
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Serialize configuration to JSON string
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = DownloadConfig::default();
        assert_eq!(config.backend, DownloaderBackend::Trauma);
        assert_eq!(config.max_concurrent, 4);
        assert_eq!(config.retries, 3);
        assert_eq!(config.timeout_seconds, 300);
        assert!(config.custom_command.is_none());
    }

    #[test]
    fn test_builder_pattern() {
        let config = DownloadConfig::new()
            .with_backend(DownloaderBackend::Trauma)
            .with_max_concurrent(8)
            .with_retries(5)
            .with_timeout(600)
            .with_custom_command("wget -O \"${FILE}\" \"${URI}\"");

        assert_eq!(config.backend, DownloaderBackend::Trauma);
        assert_eq!(config.max_concurrent, 8);
        assert_eq!(config.retries, 5);
        assert_eq!(config.timeout_seconds, 600);
        assert!(config.custom_command.is_some());
    }

    #[test]
    fn test_json_serialization() {
        let config = DownloadConfig::new().with_max_concurrent(8);
        let json = config.to_json().unwrap();
        let deserialized: DownloadConfig = DownloadConfig::from_json(&json).unwrap();

        assert_eq!(config.backend, deserialized.backend);
        assert_eq!(config.max_concurrent, deserialized.max_concurrent);
    }
}
