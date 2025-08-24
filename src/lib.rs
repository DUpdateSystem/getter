pub mod api;
mod cache;
pub mod core;
mod error;
mod locale;
pub mod rpc;
pub mod utils;
pub mod websdk;

// Re-export core API functions for easy access
pub use api::{
    // App lifecycle management
    add_app,
    // Release checking (equivalent to "renew")
    check_app_available,
    // JSON APIs for cross-language compatibility
    check_app_available_json,
    get_all_app_statuses,
    get_all_app_statuses_json,
    // Status tracking
    get_app_status,
    get_app_status_json,
    get_latest_release,
    get_latest_release_json,
    // Manager access for advanced operations
    get_manager,
    get_outdated_apps,

    get_outdated_apps_json,

    get_releases,

    get_releases_json,
    // Initialization
    init,

    list_apps,
    remove_app,
    update_app,
};

// Re-export status tracking types and functionality
pub use core::{
    app_manager::AppManager,
    app_status::AppStatus,
    status_tracker::{AppStatusInfo, StatusTracker},
};

// Re-export data types for release information
pub use websdk::repo::data::release::ReleaseData;

// rustls-platform-verifier
#[cfg(feature = "rustls-platform-verifier-android")]
pub use rustls_platform_verifier;
