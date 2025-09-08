pub mod app_status;
pub mod extended_manager;
pub mod manager;
pub mod manager_v2;
pub mod observer;
pub mod star_manager;
pub mod status_tracker;
pub mod version_ignore;

use once_cell::sync::Lazy;

pub use app_status::AppStatus;
pub use extended_manager::ExtendedAppManager;
pub use manager::*;
pub use observer::{AppManagerObserver, ObserverManager, UpdateNotification};
pub use star_manager::StarManager;
pub use status_tracker::{AppStatusInfo, StatusTracker};
pub use version_ignore::VersionIgnoreManager;

// Global instance with lazy initialization
static GLOBAL_MANAGER: Lazy<AppManager> = Lazy::new(AppManager::new);

/// Get global app manager instance
pub fn get_app_manager() -> &'static AppManager {
    &GLOBAL_MANAGER
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_global_manager() {
        let manager = get_app_manager();
        let result = manager.list_apps().await;
        assert!(result.is_ok());
    }
}
