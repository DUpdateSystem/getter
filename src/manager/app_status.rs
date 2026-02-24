use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppStatus {
    /// App exists only in hub's auto-discovery; not in user's saved list
    AppInactive,
    /// Version data is being fetched
    AppPending,
    /// Network request failed
    NetworkError,
    /// Local version is up to date
    AppLatest,
    /// A newer version is available
    AppOutdated,
    /// No local version found (e.g. not installed)
    AppNoLocal,
}
