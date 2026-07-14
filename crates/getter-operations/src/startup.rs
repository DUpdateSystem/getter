//! Idempotent getter startup and unified app/update read model.

#[cfg(feature = "lua")]
use crate::lua_provider_host::{evaluate_provider_backed_package, ProviderBackedPackageEvalConfig};
#[cfg(feature = "lua")]
use crate::provider_cache::{ProviderCacheMode, CACHE_ONLY_MISS};
use getter_core::autogen::{
    validate_installed_inventory, InstalledInventory, InstalledInventoryItem,
};
#[cfg(feature = "lua")]
use getter_core::repository::RepositoryPackageDirectoryLayout;
use getter_core::repository::{GetterDataDirLayout, REPOSITORY_ROOT_METADATA_FILE};
use getter_core::update::{
    check_updates_offline, compare_versions, UpdateCheckStatus, UpdateSelectionPolicy,
};
#[cfg(feature = "lua")]
use getter_core::InstalledTarget;
#[cfg(feature = "lua")]
use getter_storage::StoredPackageResolution;
use getter_storage::{
    CacheDb, MainDb, StorageError, StoredRepository, StoredTrackedPackage,
    CACHE_STORAGE_CONTRACT_VERSION, MAIN_STORAGE_CONTRACT_VERSION,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const REPOSITORY_ROOT_METADATA_STARTER: &str = r#"{
  "version": 1,
  // Autogen writes to "autogen" by default. Uncomment and change this
  // if generated packages should target another existing repository alias.
  // "generated_repository": "autogen",
  "priority": {
    "local": 100,
    "official": 0,
    "autogen": -1
  }
}
"#;
pub const STARTUP_SNAPSHOT_FORMAT: &str = "getter-startup-snapshot";
pub const STARTUP_SNAPSHOT_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("startup I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("startup storage failed: {0}")]
    Storage(#[from] StorageError),
    #[error("invalid installed inventory: {0}")]
    Inventory(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupSnapshot {
    pub format: String,
    pub version: u32,
    pub bootstrap: BootstrapStatus,
    pub repositories: Vec<StartupRepository>,
    pub apps: Vec<StartupAppSummary>,
    pub update_count: usize,
    pub diagnostics: Vec<StartupDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapStatus {
    pub lifecycle: BootstrapLifecycle,
    pub data_dir: PathBuf,
    pub main_db: DatabaseStatus,
    pub cache_db: DatabaseStatus,
    pub repo: PathBuf,
    pub rc: PathBuf,
    pub repo_metadata: PathBuf,
    pub diagnostics: Vec<StartupDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapLifecycle {
    Initialized,
    AlreadyInitialized,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseStatus {
    pub path: PathBuf,
    pub contract_version: u32,
    pub created_this_call: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupRepository {
    pub id: String,
    pub name: String,
    pub priority: i32,
    pub api_version: String,
    pub path: Option<String>,
    pub revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupAppSummary {
    pub package_id: String,
    pub repository_id: Option<String>,
    pub name: Option<String>,
    pub favorite: bool,
    pub pin_version: Option<String>,
    pub installed_target: Option<StartupInstalledTarget>,
    pub installed_version: Option<String>,
    pub effective_installed_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_status: UpdateStatus,
    pub warning: StartupWarning,
    pub diagnostics: Vec<StartupDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupInstalledTarget {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupWarning {
    pub free_network: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupDiagnostic {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStatus {
    Available,
    UpToDate,
    NoCandidates,
    NotInstalled,
}

pub fn bootstrap_data_dir(data_dir: &Path) -> Result<BootstrapStatus, StartupError> {
    let layout = GetterDataDirLayout::new(data_dir);
    let main_created = !layout.main_db.exists();
    let cache_created = !layout.cache_db.exists();
    let repo_created = !layout.repository_root.is_dir();
    let rc_created = !layout.runtime_config_root.is_dir();
    fs::create_dir_all(&layout.repository_root)?;
    fs::create_dir_all(&layout.runtime_config_root)?;
    let repo_metadata = layout.repository_root.join(REPOSITORY_ROOT_METADATA_FILE);
    let metadata_created = !repo_metadata.is_file();
    if metadata_created {
        fs::write(&repo_metadata, REPOSITORY_ROOT_METADATA_STARTER)?;
    }
    let (_, main_migrated) = MainDb::open_with_migration_status(&layout.main_db)?;
    let (_, cache_migrated) = CacheDb::open_with_migration_status(&layout.cache_db)?;
    let mut diagnostics = Vec::new();
    if main_migrated {
        diagnostics.push(diagnostic(
            "storage.main_migrations_applied",
            "Main storage schema migrations were applied",
        ));
    }
    if cache_migrated {
        diagnostics.push(diagnostic(
            "storage.cache_migrations_applied",
            "Cache storage schema migrations were applied",
        ));
    }
    Ok(BootstrapStatus {
        lifecycle: if !main_created
            && !cache_created
            && !repo_created
            && !rc_created
            && !metadata_created
            && !main_migrated
            && !cache_migrated
        {
            BootstrapLifecycle::AlreadyInitialized
        } else {
            BootstrapLifecycle::Initialized
        },
        data_dir: layout.root,
        main_db: DatabaseStatus {
            path: layout.main_db,
            contract_version: MAIN_STORAGE_CONTRACT_VERSION,
            created_this_call: main_created,
        },
        cache_db: DatabaseStatus {
            path: layout.cache_db,
            contract_version: CACHE_STORAGE_CONTRACT_VERSION,
            created_this_call: cache_created,
        },
        repo: layout.repository_root,
        rc: layout.runtime_config_root,
        repo_metadata,
        diagnostics,
    })
}

pub fn startup(
    data_dir: &Path,
    inventory: InstalledInventory,
) -> Result<StartupSnapshot, StartupError> {
    validate_installed_inventory(&inventory)
        .map_err(|error| StartupError::Inventory(error.to_string()))?;
    let bootstrap = bootstrap_data_dir(data_dir)?;
    let db = MainDb::open(&bootstrap.main_db.path)?;
    let stored_repositories = db.repositories()?;
    let repositories = stored_repositories.iter().map(repository_dto).collect();
    let apps: Vec<_> = db
        .tracked_packages()?
        .into_iter()
        .filter(|p| p.enabled)
        .map(|package| app_summary(data_dir, &stored_repositories, &inventory, package))
        .collect();
    let update_count = apps
        .iter()
        .filter(|app| app.update_status == UpdateStatus::Available)
        .count();
    Ok(StartupSnapshot {
        format: STARTUP_SNAPSHOT_FORMAT.to_owned(),
        version: STARTUP_SNAPSHOT_VERSION,
        bootstrap,
        repositories,
        apps,
        update_count,
        diagnostics: vec![],
    })
}

fn repository_dto(repo: &StoredRepository) -> StartupRepository {
    StartupRepository {
        id: repo.id.to_string(),
        name: repo.name.clone(),
        priority: repo.priority.value(),
        api_version: repo.api_version.clone(),
        path: repo.path.clone(),
        revision: repo.revision.clone(),
    }
}

fn diagnostic(code: &str, message: impl Into<String>) -> StartupDiagnostic {
    StartupDiagnostic {
        code: code.into(),
        message: message.into(),
    }
}

#[cfg_attr(not(feature = "lua"), allow(unused_mut, unused_variables))]
fn app_summary(
    data_dir: &Path,
    repositories: &[StoredRepository],
    inventory: &InstalledInventory,
    tracked: StoredTrackedPackage,
) -> StartupAppSummary {
    let mut diagnostics = Vec::new();
    #[cfg(feature = "lua")]
    let repository = match tracked.repository_id.as_ref() {
        Some(id) => repositories.iter().find(|repo| &repo.id == id),
        None if tracked.package_resolution
            == StoredPackageResolution::OfficialRepositoryPackage =>
        {
            repositories.iter().find(|repository| {
                let root = repository
                    .path
                    .as_ref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| data_dir.join("repo").join(repository.id.as_str()));
                RepositoryPackageDirectoryLayout::load(&root)
                    .ok()
                    .is_some_and(|layout| layout.package(&tracked.package_id).is_some())
            })
        }
        None => None,
    };
    #[cfg(not(feature = "lua"))]
    let repository = tracked
        .repository_id
        .as_ref()
        .and_then(|id| repositories.iter().find(|repo| &repo.id == id));
    let mut name = None;
    let mut installed_target = None;
    let mut warning = StartupWarning::default();
    let mut candidates = Vec::new();

    #[cfg(feature = "lua")]
    if tracked.package_resolution == StoredPackageResolution::OfficialRepositoryPackage {
        if let Some(repository) = repository {
            let root = repository
                .path
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_else(|| data_dir.join("repo").join(repository.id.as_str()));
            let evaluated = (|| {
                let layout =
                    RepositoryPackageDirectoryLayout::load(&root).map_err(|e| e.to_string())?;
                let package = layout.package(&tracked.package_id).ok_or_else(|| {
                    format!("package {} is absent from repository", tracked.package_id)
                })?;
                let metadata = layout
                    .package_metadata(package)
                    .map_err(|e| e.to_string())?;
                let script = layout
                    .unambiguous_version_script(package)
                    .map_err(|e| e.to_string())?;
                evaluate_provider_backed_package(
                    data_dir,
                    &repository.id,
                    package,
                    &metadata,
                    script,
                    ProviderBackedPackageEvalConfig {
                        mode: ProviderCacheMode::CacheOnly,
                        fdroid_endpoint_id: None,
                        fdroid_endpoint_url: None,
                        fdroid_index_xml: None,
                        github_endpoint_id: None,
                        github_api_base_url: None,
                        github_releases_json: None,
                        github_release_transport: None,
                        github_include_prereleases: false,
                    },
                )
                .map(|result| result.package)
                .map_err(|e| e.to_string())
            })();
            match evaluated {
                Ok(resolved) => {
                    name = Some(resolved.name);
                    warning.free_network = resolved.permissions.free_network;
                    installed_target = resolved.installed.first().map(target_dto);
                    candidates = resolved.updates;
                }
                Err(message) => diagnostics.push(diagnostic(
                    if message.contains(CACHE_ONLY_MISS) || message.contains("cache-only miss") {
                        "startup.provider_cache_miss"
                    } else {
                        "startup.package_eval_failed"
                    },
                    message,
                )),
            }
        } else {
            diagnostics.push(diagnostic(
                "startup.repository_missing",
                "Tracked official package has no resolvable repository",
            ));
        }
    } else {
        diagnostics.push(diagnostic(
            match tracked.package_resolution {
                StoredPackageResolution::GenerateLocalPackage => {
                    "startup.package_generation_required"
                }
                StoredPackageResolution::MissingPackageDefinition => "startup.package_missing",
                StoredPackageResolution::OfficialRepositoryPackage => unreachable!(),
            },
            format!(
                "Tracked package resolution is {}",
                tracked.package_resolution.as_str()
            ),
        ));
    }

    let installed_version = installed_target
        .as_ref()
        .and_then(|target| inventory_version(inventory, target));
    if installed_target.is_none() {
        diagnostics.push(diagnostic(
            "startup.installed_target_missing",
            "Package evaluation returned no installed target",
        ));
        diagnostics.push(diagnostic(
            "startup.installed_inventory_missing",
            "No installed target was available to match against inventory",
        ));
    } else if installed_version.is_none() {
        diagnostics.push(diagnostic(
            "startup.installed_inventory_missing",
            "No matching installed inventory item was supplied",
        ));
    }

    let policy = UpdateSelectionPolicy {
        pin_version: tracked.pin_version.clone(),
    };
    let result = check_updates_offline(
        tracked.package_id.clone(),
        installed_version.clone(),
        candidates.clone(),
        policy,
    )
    .ok();
    let latest_version = candidates
        .iter()
        .max_by(|a, b| compare_versions(&a.version, &b.version))
        .map(|c| c.version.clone());
    let effective_installed_version = tracked
        .pin_version
        .clone()
        .or_else(|| installed_version.clone());
    let update_status = if installed_version.is_none() || installed_target.is_none() {
        UpdateStatus::NotInstalled
    } else {
        match result.as_ref().map(|r| r.status) {
            Some(UpdateCheckStatus::UpdateAvailable) => UpdateStatus::Available,
            Some(UpdateCheckStatus::UpToDate) => UpdateStatus::UpToDate,
            Some(UpdateCheckStatus::NoCandidates) | None => UpdateStatus::NoCandidates,
        }
    };
    StartupAppSummary {
        package_id: tracked.package_id.to_string(),
        repository_id: tracked.repository_id.map(|id| id.to_string()),
        name,
        favorite: tracked.favorite,
        pin_version: tracked.pin_version,
        installed_target,
        installed_version,
        effective_installed_version,
        latest_version,
        update_status,
        warning,
        diagnostics,
    }
}

#[cfg(feature = "lua")]
fn target_dto(target: &InstalledTarget) -> StartupInstalledTarget {
    match target {
        InstalledTarget::AndroidPackage { package_name } => StartupInstalledTarget {
            kind: "android_package".into(),
            id: package_name.clone(),
        },
        InstalledTarget::MagiskModule { module_id } => StartupInstalledTarget {
            kind: "magisk_module".into(),
            id: module_id.clone(),
        },
        InstalledTarget::Generic { id } => StartupInstalledTarget {
            kind: "generic".into(),
            id: id.clone(),
        },
    }
}

fn inventory_version(
    inventory: &InstalledInventory,
    target: &StartupInstalledTarget,
) -> Option<String> {
    inventory
        .items
        .iter()
        .find_map(|item| match (item, target.kind.as_str()) {
            (
                InstalledInventoryItem::AndroidPackage {
                    package_name,
                    version_name,
                    ..
                },
                "android_package",
            ) if package_name == &target.id => version_name.clone(),
            (
                InstalledInventoryItem::MagiskModule {
                    module_id,
                    version_name,
                    ..
                },
                "magisk_module",
            ) if module_id == &target.id => version_name.clone(),
            _ => None,
        })
}
