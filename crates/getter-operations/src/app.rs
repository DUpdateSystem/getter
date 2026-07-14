//! Shared app inspection and explicit update-check operations.

#[cfg(feature = "lua")]
use crate::github_releases::GithubReleaseTransport;
use crate::startup::{self, StartupAppSummary};
use getter_core::autogen::{validate_installed_inventory, InstalledInventory};
#[cfg(feature = "lua")]
use getter_core::runtime::GetterRuntime;
use getter_core::runtime::IssuedAction;
use getter_core::PackageId;
use getter_storage::{MainDb, StorageError, StoredTrackedPackage};
use serde::{Deserialize, Serialize};
use std::path::Path;
#[cfg(feature = "lua")]
use std::rc::Rc;

#[derive(Debug, thiserror::Error)]
pub enum AppOperationError {
    #[error("app is not tracked: {0}")]
    Untracked(PackageId),
    #[error("app is disabled: {0}")]
    Disabled(PackageId),
    #[error("invalid installed inventory: {0}")]
    Inventory(String),
    #[error("app storage failed: {0}")]
    Storage(#[from] StorageError),
    #[error("app startup model failed: {0}")]
    Startup(#[from] startup::StartupError),
    #[cfg(feature = "lua")]
    #[error("app update check failed: {0}")]
    Runtime(#[from] crate::runtime::RuntimeOperationError),
}

impl AppOperationError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Untracked(_) => "app.untracked",
            Self::Disabled(_) => "app.disabled",
            Self::Inventory(_) => "inventory.invalid",
            Self::Storage(_) => "storage.error",
            Self::Startup(error) => match error {
                startup::StartupError::Io(_) => "startup.io_error",
                startup::StartupError::Storage(_) => "storage.error",
                startup::StartupError::Inventory(_) => "inventory.invalid",
            },
            #[cfg(feature = "lua")]
            Self::Runtime(error) => error.code(),
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::Untracked(_) => "Getter app is not tracked",
            Self::Disabled(_) => "Getter app is disabled",
            Self::Inventory(_) => "Installed inventory is invalid",
            Self::Storage(_) => "Getter storage operation failed",
            Self::Startup(_) => "Getter startup operation failed",
            #[cfg(feature = "lua")]
            Self::Runtime(error) => error.message(),
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Self::Untracked(id) | Self::Disabled(id) => id.to_string(),
            Self::Inventory(detail) => detail.clone(),
            Self::Storage(error) => error.to_string(),
            Self::Startup(error) => error.to_string(),
            #[cfg(feature = "lua")]
            Self::Runtime(error) => error.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppCheckResult {
    pub app: StartupAppSummary,
    /// Getter-issued action valid only in the runtime that performed this check.
    pub action: Option<IssuedAction>,
}

fn tracked_package(
    db: &MainDb,
    package_id: &PackageId,
) -> Result<StoredTrackedPackage, AppOperationError> {
    let tracked = db
        .tracked_packages()?
        .into_iter()
        .find(|package| &package.package_id == package_id)
        .ok_or_else(|| AppOperationError::Untracked(package_id.clone()))?;
    if !tracked.enabled {
        return Err(AppOperationError::Disabled(package_id.clone()));
    }
    Ok(tracked)
}

pub fn inspect_app(
    data_dir: &Path,
    inventory: InstalledInventory,
    package_id: &PackageId,
) -> Result<StartupAppSummary, AppOperationError> {
    validate_installed_inventory(&inventory)
        .map_err(|error| AppOperationError::Inventory(error.to_string()))?;
    startup::bootstrap_data_dir(data_dir)?;
    let db = MainDb::open(data_dir.join("main.db"))?;
    let tracked = tracked_package(&db, package_id)?;
    let repositories = db.repositories()?;
    Ok(startup::app_summary(
        data_dir,
        &repositories,
        &inventory,
        tracked,
    ))
}

#[cfg(feature = "lua")]
pub fn check_app(
    runtime: &mut GetterRuntime,
    data_dir: &Path,
    inventory: InstalledInventory,
    package_id: &PackageId,
) -> Result<AppCheckResult, AppOperationError> {
    check_app_with_github_transport(
        runtime,
        data_dir,
        inventory,
        package_id,
        Some(Rc::new(
            crate::github_releases::UreqGithubReleaseTransport::new(),
        )),
    )
}

#[cfg(feature = "lua")]
pub fn check_app_with_github_transport(
    runtime: &mut GetterRuntime,
    data_dir: &Path,
    inventory: InstalledInventory,
    package_id: &PackageId,
    github_release_transport: Option<Rc<dyn GithubReleaseTransport>>,
) -> Result<AppCheckResult, AppOperationError> {
    validate_installed_inventory(&inventory)
        .map_err(|error| AppOperationError::Inventory(error.to_string()))?;
    let db = MainDb::open(data_dir.join("main.db"))?;
    let tracked = tracked_package(&db, package_id)?;
    let repository_id = tracked.repository_id.clone();
    let mut request = crate::runtime::RegisteredPackageUpdateActionRequest {
        package_id: package_id.clone(),
        repository_id,
        installed_version: None,
        pin_version: tracked.pin_version.clone(),
    };
    let evaluated = crate::runtime::evaluate_registered_package(
        data_dir,
        &db,
        &request,
        crate::provider_cache::ProviderCacheMode::ForceRefresh,
        github_release_transport,
    )?;
    let mut app = startup::app_summary_from_resolved(
        tracked,
        Some(evaluated.package.repository.to_string()),
        &inventory,
        evaluated.package.clone(),
    );
    app.diagnostics
        .extend(
            evaluated
                .diagnostics
                .iter()
                .map(|diagnostic| startup::StartupDiagnostic {
                    code: diagnostic.code.clone(),
                    message: diagnostic.message.clone(),
                }),
        );
    let installed_version = app.installed_version.clone().ok_or_else(|| {
        AppOperationError::Inventory(format!("package {package_id} is not installed"))
    })?;
    request.installed_version = Some(installed_version);
    let checked =
        crate::runtime::issue_action_from_registered_evaluation(runtime, request, evaluated)?;
    Ok(AppCheckResult {
        app,
        action: checked.action,
    })
}
