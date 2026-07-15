//! Shared app inspection and explicit update-check operations.

#[cfg(feature = "lua")]
use crate::github_releases::GithubReleaseTransport;
use crate::startup::{self, StartupAppSummary};
use getter_core::autogen::{validate_installed_inventory, InstalledInventory};
#[cfg(feature = "lua")]
use getter_core::manifest::{ManifestError, PackageManifest};
#[cfg(feature = "lua")]
use getter_core::repository::PACKAGE_MANIFEST_FILE;
#[cfg(feature = "lua")]
use getter_core::runtime::GetterRuntime;
use getter_core::runtime::IssuedAction;
#[cfg(feature = "lua")]
use getter_core::update::compare_versions;
use getter_core::PackageId;
#[cfg(feature = "lua")]
use getter_core::UpdateArtifact;
use getter_storage::{MainDb, StorageError, StoredTrackedPackage};
use serde::{Deserialize, Serialize};
#[cfg(feature = "lua")]
use sha2::{Digest, Sha256};
use std::path::Path;
#[cfg(feature = "lua")]
use std::path::PathBuf;
#[cfg(feature = "lua")]
use std::{fs, io::Write, rc::Rc};

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
    #[cfg(feature = "lua")]
    #[error("selected package has no download candidates")]
    NoCandidates,
    #[cfg(feature = "lua")]
    #[error("package Manifest is missing")]
    ManifestMissing,
    #[cfg(feature = "lua")]
    #[error("package Manifest is invalid: {0}")]
    ManifestInvalid(ManifestError),
    #[cfg(feature = "lua")]
    #[error("artifact '{artifact}' filename '{file_name}' has no matching SHA-256 Manifest entry")]
    ManifestArtifactMissing { artifact: String, file_name: String },
    #[cfg(feature = "lua")]
    #[error("artifact '{artifact}' filename '{file_name}' has multiple SHA-256 Manifest entries and no digest hint")]
    ManifestArtifactAmbiguous { artifact: String, file_name: String },
    #[cfg(feature = "lua")]
    #[error("artifact '{artifact}' carries an invalid SHA-256 digest")]
    ArtifactSha256Invalid { artifact: String },
    #[cfg(feature = "lua")]
    #[error("artifact filenames '{first}' and '{second}' resolve to the same staging path")]
    ArtifactPathCollision { first: String, second: String },
    #[cfg(feature = "lua")]
    #[error("artifact '{artifact}' SHA-256 mismatch: expected {expected}, got {actual}")]
    Sha256Mismatch {
        artifact: String,
        expected: String,
        actual: String,
    },
    #[cfg(feature = "lua")]
    #[error("artifact staging I/O failed: {0}")]
    ArtifactIo(#[from] std::io::Error),
    #[cfg(feature = "lua")]
    #[error("artifact download failed: {0}")]
    ArtifactTransport(#[from] crate::download::RuntimeDownloadTransportError),
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
            #[cfg(feature = "lua")]
            Self::NoCandidates => "app.no_candidates",
            #[cfg(feature = "lua")]
            Self::ManifestMissing => "artifact.manifest_missing",
            #[cfg(feature = "lua")]
            Self::ManifestInvalid(source) => match source {
                ManifestError::Malformed { .. } => "artifact.manifest_malformed",
                ManifestError::Duplicate { .. } => "artifact.manifest_duplicate",
            },
            #[cfg(feature = "lua")]
            Self::ManifestArtifactMissing { .. } => "artifact.manifest_missing",
            #[cfg(feature = "lua")]
            Self::ManifestArtifactAmbiguous { .. } => "artifact.manifest_ambiguous",
            #[cfg(feature = "lua")]
            Self::ArtifactSha256Invalid { .. } => "artifact.sha256_invalid",
            #[cfg(feature = "lua")]
            Self::ArtifactPathCollision { .. } => "artifact.path_collision",
            #[cfg(feature = "lua")]
            Self::Sha256Mismatch { .. } => "artifact.sha256_mismatch",
            #[cfg(feature = "lua")]
            Self::ArtifactIo(_) => "artifact.io_error",
            #[cfg(feature = "lua")]
            Self::ArtifactTransport(_) => "artifact.transport_error",
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
            #[cfg(feature = "lua")]
            Self::NoCandidates => "Getter app has no download candidate",
            #[cfg(feature = "lua")]
            Self::ManifestMissing => "Getter package Manifest is missing",
            #[cfg(feature = "lua")]
            Self::ManifestInvalid(_) => "Getter package Manifest is invalid",
            #[cfg(feature = "lua")]
            Self::ManifestArtifactMissing { .. } => "Getter artifact is missing from Manifest",
            #[cfg(feature = "lua")]
            Self::ManifestArtifactAmbiguous { .. } => "Getter artifact Manifest entry is ambiguous",
            #[cfg(feature = "lua")]
            Self::ArtifactSha256Invalid { .. } => "Getter artifact SHA-256 is invalid",
            #[cfg(feature = "lua")]
            Self::ArtifactPathCollision { .. } => "Getter artifact staging paths collide",
            #[cfg(feature = "lua")]
            Self::Sha256Mismatch { .. } => "Getter artifact SHA-256 did not match",
            #[cfg(feature = "lua")]
            Self::ArtifactIo(_) => "Getter artifact staging failed",
            #[cfg(feature = "lua")]
            Self::ArtifactTransport(_) => "Getter artifact download failed",
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
            #[cfg(feature = "lua")]
            Self::NoCandidates => self.to_string(),
            #[cfg(feature = "lua")]
            Self::ManifestMissing
            | Self::ManifestInvalid(_)
            | Self::ManifestArtifactMissing { .. }
            | Self::ManifestArtifactAmbiguous { .. }
            | Self::ArtifactSha256Invalid { .. }
            | Self::ArtifactPathCollision { .. }
            | Self::Sha256Mismatch { .. }
            | Self::ArtifactIo(_)
            | Self::ArtifactTransport(_) => self.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppCheckResult {
    pub app: StartupAppSummary,
    /// Getter-issued action valid only in the runtime that performed this check.
    pub action: Option<IssuedAction>,
}

#[cfg(feature = "lua")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppDownloadResult {
    pub package_id: PackageId,
    pub repository_id: String,
    pub version: String,
    pub artifacts: Vec<StagedArtifact>,
}

#[cfg(feature = "lua")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedArtifact {
    pub name: String,
    pub path: PathBuf,
    pub sha256: String,
    pub status: String,
}

#[cfg(feature = "lua")]
struct PreparedArtifact {
    artifact: UpdateArtifact,
    digest: String,
    final_path: PathBuf,
    temporary_path: PathBuf,
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
pub fn download_app(
    data_dir: &Path,
    package_id: &PackageId,
) -> Result<AppDownloadResult, AppOperationError> {
    download_app_with_transports(
        data_dir,
        package_id,
        Some(Rc::new(
            crate::github_releases::UreqGithubReleaseTransport::new(),
        )),
        Rc::new(crate::download::UreqRuntimeDownloadTransport::new()),
    )
}

#[cfg(feature = "lua")]
pub fn download_app_with_transports(
    data_dir: &Path,
    package_id: &PackageId,
    github_release_transport: Option<Rc<dyn GithubReleaseTransport>>,
    download_transport: Rc<dyn crate::download::RuntimeDownloadTransport>,
) -> Result<AppDownloadResult, AppOperationError> {
    startup::bootstrap_data_dir(data_dir)?;
    let db = MainDb::open(data_dir.join("main.db"))?;
    let tracked = tracked_package(&db, package_id)?;
    let evaluation = crate::runtime::evaluate_registered_package(
        data_dir,
        &db,
        &crate::runtime::RegisteredPackageUpdateActionRequest {
            package_id: package_id.clone(),
            repository_id: tracked.repository_id,
            installed_version: None,
            pin_version: None,
        },
        crate::provider_cache::ProviderCacheMode::ForceRefresh,
        github_release_transport,
    )?;
    let candidate = evaluation
        .package
        .updates
        .iter()
        .max_by(|left, right| compare_versions(&left.version, &right.version))
        .cloned()
        .ok_or(AppOperationError::NoCandidates)?;

    let manifest_path = evaluation.package_path.join(PACKAGE_MANIFEST_FILE);
    let manifest_content = fs::read_to_string(&manifest_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            AppOperationError::ManifestMissing
        } else {
            AppOperationError::ArtifactIo(error)
        }
    })?;
    let manifest =
        PackageManifest::parse(&manifest_content).map_err(AppOperationError::ManifestInvalid)?;
    let downloads = data_dir.join("downloads");
    let prepared = candidate
        .artifacts
        .iter()
        .map(|artifact| prepare_artifact(&downloads, &manifest, artifact))
        .collect::<Result<Vec<_>, _>>()?;
    let mut staging_paths = std::collections::HashMap::new();
    for artifact in &prepared {
        let file_name = artifact
            .artifact
            .file_name
            .clone()
            .unwrap_or_else(|| artifact.artifact.name.clone());
        for path in [&artifact.final_path, &artifact.temporary_path] {
            if let Some(first) = staging_paths.insert(path.clone(), file_name.clone()) {
                return Err(AppOperationError::ArtifactPathCollision {
                    first,
                    second: file_name,
                });
            }
        }
    }
    if prepared
        .iter()
        .any(|artifact| !artifact.final_path.exists())
    {
        fs::create_dir_all(&downloads)?;
    }

    let artifacts = prepared
        .into_iter()
        .map(|artifact| stage_artifact(artifact, download_transport.as_ref()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AppDownloadResult {
        package_id: package_id.clone(),
        repository_id: evaluation.package.repository.to_string(),
        version: candidate.version,
        artifacts,
    })
}

#[cfg(feature = "lua")]
fn prepare_artifact(
    downloads: &Path,
    manifest: &PackageManifest,
    artifact: &UpdateArtifact,
) -> Result<PreparedArtifact, AppOperationError> {
    let file_name = artifact
        .file_name
        .as_deref()
        .unwrap_or(artifact.name.as_str());
    let members = manifest.artifact_sha256_members(file_name);
    let digest = if let Some(hint) = artifact.sha256.as_deref() {
        if hint.len() != 64 || !hint.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AppOperationError::ArtifactSha256Invalid {
                artifact: artifact.name.clone(),
            });
        }
        let hint = hint.to_ascii_lowercase();
        if !members.iter().any(|member| member == &hint) {
            return Err(AppOperationError::ManifestArtifactMissing {
                artifact: artifact.name.clone(),
                file_name: file_name.to_owned(),
            });
        }
        hint
    } else {
        match members {
            [] => {
                return Err(AppOperationError::ManifestArtifactMissing {
                    artifact: artifact.name.clone(),
                    file_name: file_name.to_owned(),
                });
            }
            [digest] => digest.clone(),
            _ => {
                return Err(AppOperationError::ManifestArtifactAmbiguous {
                    artifact: artifact.name.clone(),
                    file_name: file_name.to_owned(),
                });
            }
        }
    };
    let name = sanitized_artifact_file_name(file_name);
    let final_path = downloads.join(format!("{digest}-{name}"));
    let temporary_path = PathBuf::from(format!(
        "{}.download",
        final_path.as_os_str().to_string_lossy()
    ));
    Ok(PreparedArtifact {
        artifact: artifact.clone(),
        final_path,
        temporary_path,
        digest,
    })
}

#[cfg(feature = "lua")]
fn stage_artifact(
    prepared: PreparedArtifact,
    transport: &dyn crate::download::RuntimeDownloadTransport,
) -> Result<StagedArtifact, AppOperationError> {
    if prepared.final_path.exists() {
        return Ok(staged_artifact(&prepared, "reused"));
    }
    let mut sink = StagingSink::new(&prepared.temporary_path)?;
    transport.fetch(
        &crate::download::RuntimeDownloadTransportRequest {
            url: &prepared.artifact.url,
        },
        &mut sink,
    )?;
    let actual = sink.finish()?;
    if actual != prepared.digest {
        return Err(AppOperationError::Sha256Mismatch {
            artifact: prepared.artifact.name,
            expected: prepared.digest,
            actual,
        });
    }
    fs::rename(&prepared.temporary_path, &prepared.final_path)?;
    Ok(staged_artifact(&prepared, "downloaded"))
}

#[cfg(feature = "lua")]
fn staged_artifact(prepared: &PreparedArtifact, status: &str) -> StagedArtifact {
    StagedArtifact {
        name: prepared.artifact.name.clone(),
        path: prepared.final_path.clone(),
        sha256: prepared.digest.clone(),
        status: status.to_owned(),
    }
}

#[cfg(feature = "lua")]
struct StagingSink {
    file: fs::File,
    hasher: Sha256,
}

#[cfg(feature = "lua")]
impl StagingSink {
    fn new(path: &Path) -> Result<Self, std::io::Error> {
        Ok(Self {
            file: fs::File::create(path)?,
            hasher: Sha256::new(),
        })
    }

    fn finish(mut self) -> Result<String, std::io::Error> {
        self.file.flush()?;
        self.file.sync_all()?;
        Ok(format!("{:x}", self.hasher.finalize()))
    }
}

#[cfg(feature = "lua")]
impl crate::download::RuntimeDownloadSink for StagingSink {
    fn set_total_bytes(&mut self, _total_bytes: Option<u64>) -> Result<(), String> {
        Ok(())
    }

    fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), String> {
        self.file
            .write_all(chunk)
            .map_err(|error| error.to_string())?;
        self.hasher.update(chunk);
        Ok(())
    }
}

#[cfg(feature = "lua")]
fn sanitized_artifact_file_name(raw: &str) -> String {
    let sanitized = raw
        .trim()
        .chars()
        .map(|character| {
            if character.is_control() || matches!(character, '/' | '\\') {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        "artifact.bin".to_owned()
    } else {
        sanitized
    }
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
