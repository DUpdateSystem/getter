pub mod app_status;
pub mod manager;
pub mod status_tracker;

use once_cell::sync::Lazy;

pub use app_status::AppStatus;
pub use manager::*;
pub use status_tracker::{AppStatusInfo, StatusTracker};

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
