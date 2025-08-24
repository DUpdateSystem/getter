use serde::{Deserialize, Serialize};

/// Application status indicating current state relative to available updates
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AppStatus {
    /// App is not being tracked/managed
    AppInactive,
    /// App is currently being checked for updates
    AppPending,
    /// Network error occurred while checking for updates
    NetworkError,
    /// App is up to date with the latest version
    AppLatest,
    /// App has updates available
    AppOutdated,
    /// App is tracked but no local version is known
    AppNoLocal,
}

impl AppStatus {
    /// Check if the app is in an active tracking state
    pub fn is_active(&self) -> bool {
        !matches!(self, AppStatus::AppInactive)
    }

    /// Check if the app is up to date
    pub fn is_latest(&self) -> bool {
        matches!(self, AppStatus::AppLatest)
    }

    /// Check if the app has updates available
    pub fn has_updates(&self) -> bool {
        matches!(self, AppStatus::AppOutdated)
    }

    /// Check if the app is in a pending state
    pub fn is_pending(&self) -> bool {
        matches!(self, AppStatus::AppPending)
    }

    /// Check if there was an error getting status
    pub fn has_error(&self) -> bool {
        matches!(self, AppStatus::NetworkError)
    }

    /// Get a human-readable description of the status
    pub fn description(&self) -> &'static str {
        match self {
            AppStatus::AppInactive => "Not tracked",
            AppStatus::AppPending => "Checking for updates...",
            AppStatus::NetworkError => "Network error",
            AppStatus::AppLatest => "Up to date",
            AppStatus::AppOutdated => "Updates available",
            AppStatus::AppNoLocal => "No local version",
        }
    }
}

impl Default for AppStatus {
    fn default() -> Self {
        AppStatus::AppInactive
    }
}

impl std::fmt::Display for AppStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.description())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_status_methods() {
        assert!(AppStatus::AppLatest.is_active());
        assert!(!AppStatus::AppInactive.is_active());

        assert!(AppStatus::AppLatest.is_latest());
        assert!(!AppStatus::AppOutdated.is_latest());

        assert!(AppStatus::AppOutdated.has_updates());
        assert!(!AppStatus::AppLatest.has_updates());

        assert!(AppStatus::AppPending.is_pending());
        assert!(!AppStatus::AppLatest.is_pending());

        assert!(AppStatus::NetworkError.has_error());
        assert!(!AppStatus::AppLatest.has_error());
    }

    #[test]
    fn test_app_status_descriptions() {
        assert_eq!(AppStatus::AppInactive.description(), "Not tracked");
        assert_eq!(
            AppStatus::AppPending.description(),
            "Checking for updates..."
        );
        assert_eq!(AppStatus::NetworkError.description(), "Network error");
        assert_eq!(AppStatus::AppLatest.description(), "Up to date");
        assert_eq!(AppStatus::AppOutdated.description(), "Updates available");
        assert_eq!(AppStatus::AppNoLocal.description(), "No local version");
    }

    #[test]
    fn test_app_status_display() {
        assert_eq!(AppStatus::AppLatest.to_string(), "Up to date");
        assert_eq!(AppStatus::AppOutdated.to_string(), "Updates available");
    }

    #[test]
    fn test_app_status_serialization() {
        let status = AppStatus::AppLatest;
        let serialized = serde_json::to_string(&status).unwrap();
        let deserialized: AppStatus = serde_json::from_str(&serialized).unwrap();
        assert_eq!(status, deserialized);
    }

    #[test]
    fn test_app_status_default() {
        assert_eq!(AppStatus::default(), AppStatus::AppInactive);
    }
}
