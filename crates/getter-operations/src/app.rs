//! Shared app inspection and explicit update-check operations.

#[cfg(feature = "lua")]
use crate::github_releases::GithubReleaseTransport;
use crate::startup::{self, StartupAppSummary};
use getter_core::autogen::{validate_installed_inventory, InstalledInventory};
#[cfg(feature = "lua")]
use getter_core::manifest::{ManifestError, PackageManifest};
#[cfg(feature = "lua")]
use getter_core::repository::PACKAGE_MANIFEST_FILE;
use getter_core::runtime::IssuedAction;
#[cfg(feature = "lua")]
use getter_core::runtime::{
    GetterRuntime, RuntimeError, RuntimeTaskStatus, SealedAndroidApkInstallPlan, TaskPhase,
    TaskPhaseCategory, TaskPhaseReason,
};
#[cfg(feature = "lua")]
use getter_core::update::compare_versions;
use getter_core::PackageId;
#[cfg(feature = "lua")]
use getter_core::{
    InstalledTarget, Installer, InstallerArg, InstallerCommand, ResolvedPackage, SelectedUpdate,
    UpdateArtifact,
};
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
    #[cfg(feature = "lua")]
    #[error("selected candidate has no installer command")]
    InstallerMissing,
    #[cfg(feature = "lua")]
    #[error("installer declaration schema is invalid: {0}")]
    InstallerSchema(String),
    #[cfg(feature = "lua")]
    #[error("installer artifact reference '{0}' is unknown")]
    InstallerArtifactUnknown(String),
    #[cfg(feature = "lua")]
    #[error("installer artifact name '{0}' is duplicated")]
    InstallerArtifactDuplicate(String),
    #[cfg(feature = "lua")]
    #[error("installer target is unsupported")]
    InstallerTargetUnsupported,
    #[cfg(feature = "lua")]
    #[error("installer artifact '{0}' is unsupported")]
    InstallerArtifactUnsupported(String),
    #[cfg(feature = "lua")]
    #[error("runtime task '{task_id}' has no sealed Android APK install plan")]
    TaskInstallPlanMissing { task_id: String },
    #[cfg(feature = "lua")]
    #[error("runtime task '{task_id}' has no staged install artifact")]
    TaskInstallArtifactMissing { task_id: String },
    #[cfg(feature = "lua")]
    #[error(
        "runtime task '{task_id}' staged artifact does not match its sealed install plan: {detail}"
    )]
    TaskInstallArtifactMismatch { task_id: String, detail: String },
    #[cfg(feature = "lua")]
    #[error("installer executable '{0}' was not found")]
    InstallerCommandNotFound(String),
    #[cfg(feature = "lua")]
    #[error("installer command could not be spawned: {0}")]
    InstallerCommandSpawnFailed(std::io::Error),
    #[cfg(feature = "lua")]
    #[error("installer command failed with exit code {0}")]
    InstallerCommandFailed(i32),
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
            #[cfg(feature = "lua")]
            Self::InstallerMissing => "installer.missing",
            #[cfg(feature = "lua")]
            Self::InstallerSchema(_) => "installer.schema_invalid",
            #[cfg(feature = "lua")]
            Self::InstallerArtifactUnknown(_) => "installer.artifact_unknown",
            #[cfg(feature = "lua")]
            Self::InstallerArtifactDuplicate(_) => "installer.artifact_duplicate",
            #[cfg(feature = "lua")]
            Self::InstallerTargetUnsupported => "installer.target_unsupported",
            #[cfg(feature = "lua")]
            Self::InstallerArtifactUnsupported(_) => "installer.artifact_unsupported",
            #[cfg(feature = "lua")]
            Self::TaskInstallPlanMissing { .. } => "installer.task_plan_missing",
            #[cfg(feature = "lua")]
            Self::TaskInstallArtifactMissing { .. } => "installer.task_artifact_missing",
            #[cfg(feature = "lua")]
            Self::TaskInstallArtifactMismatch { .. } => "installer.task_artifact_mismatch",
            #[cfg(feature = "lua")]
            Self::InstallerCommandNotFound(_) => "installer.command_not_found",
            #[cfg(feature = "lua")]
            Self::InstallerCommandSpawnFailed(_) => "installer.command_spawn_failed",
            #[cfg(feature = "lua")]
            Self::InstallerCommandFailed(_) => "installer.command_failed",
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
            #[cfg(feature = "lua")]
            Self::InstallerMissing => "Getter package installer is missing",
            #[cfg(feature = "lua")]
            Self::InstallerSchema(_) => "Getter package installer schema is invalid",
            #[cfg(feature = "lua")]
            Self::InstallerArtifactUnknown(_) => "Getter installer references an unknown artifact",
            #[cfg(feature = "lua")]
            Self::InstallerArtifactDuplicate(_) => "Getter installer artifact name is duplicated",
            #[cfg(feature = "lua")]
            Self::InstallerTargetUnsupported => "Getter installer target is unsupported",
            #[cfg(feature = "lua")]
            Self::InstallerArtifactUnsupported(_) => "Getter installer artifact is unsupported",
            #[cfg(feature = "lua")]
            Self::TaskInstallPlanMissing { .. } => {
                "Getter runtime task has no sealed Android install plan"
            }
            #[cfg(feature = "lua")]
            Self::TaskInstallArtifactMissing { .. } => {
                "Getter runtime task has no staged install artifact"
            }
            #[cfg(feature = "lua")]
            Self::TaskInstallArtifactMismatch { .. } => {
                "Getter runtime task artifact does not match its sealed install plan"
            }
            #[cfg(feature = "lua")]
            Self::InstallerCommandNotFound(_) => "Getter installer command was not found",
            #[cfg(feature = "lua")]
            Self::InstallerCommandSpawnFailed(_) => "Getter installer command could not be started",
            #[cfg(feature = "lua")]
            Self::InstallerCommandFailed(_) => "Getter installer command failed",
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
            | Self::ArtifactTransport(_)
            | Self::InstallerMissing
            | Self::InstallerSchema(_)
            | Self::InstallerArtifactUnknown(_)
            | Self::InstallerArtifactDuplicate(_)
            | Self::InstallerTargetUnsupported
            | Self::InstallerArtifactUnsupported(_)
            | Self::TaskInstallPlanMissing { .. }
            | Self::TaskInstallArtifactMissing { .. }
            | Self::TaskInstallArtifactMismatch { .. }
            | Self::InstallerCommandNotFound(_)
            | Self::InstallerCommandSpawnFailed(_)
            | Self::InstallerCommandFailed(_) => self.to_string(),
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
pub const PLATFORM_INSTALL_HANDOFF_FORMAT: &str = "getter-platform-install-handoff";
#[cfg(feature = "lua")]
pub const PLATFORM_INSTALL_HANDOFF_VERSION: u32 = 1;

#[cfg(feature = "lua")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformInstallHandoff {
    pub format: String,
    pub version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub package_id: PackageId,
    pub repository_id: String,
    pub package_version: String,
    pub request: PlatformInstallRequest,
}

#[cfg(feature = "lua")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlatformInstallRequest {
    AndroidApk {
        target: AndroidInstallTarget,
        artifact: StagedArtifact,
    },
}

#[cfg(feature = "lua")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AndroidInstallTarget {
    pub kind: String,
    pub package_name: String,
}

#[cfg(feature = "lua")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedInstallerCommand {
    pub executable: PathBuf,
    pub args: Vec<String>,
}

#[cfg(feature = "lua")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[cfg(feature = "lua")]
pub trait CommandResolver {
    fn resolve(&self, executable: &str) -> Option<PathBuf>;
}
#[cfg(feature = "lua")]
pub trait CommandRunner {
    fn run(&self, command: &ResolvedInstallerCommand) -> std::io::Result<RunnerOutput>;
}

#[cfg(feature = "lua")]
pub trait CommandObserver {
    fn before_execute(&self, command: &ResolvedInstallerCommand) -> std::io::Result<()>;
}

#[cfg(feature = "lua")]
pub struct NoopCommandObserver;
#[cfg(feature = "lua")]
impl CommandObserver for NoopCommandObserver {
    fn before_execute(&self, _: &ResolvedInstallerCommand) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(feature = "lua")]
pub struct PathCommandResolver;
#[cfg(feature = "lua")]
impl CommandResolver for PathCommandResolver {
    fn resolve(&self, executable: &str) -> Option<PathBuf> {
        which::which(executable).ok()
    }
}

#[cfg(feature = "lua")]
pub struct ProcessCommandRunner;
#[cfg(feature = "lua")]
impl CommandRunner for ProcessCommandRunner {
    fn run(&self, command: &ResolvedInstallerCommand) -> std::io::Result<RunnerOutput> {
        let mut child = std::process::Command::new(&command.executable)
            .args(&command.args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| {
            std::io::Error::other("installer runner failed to capture child stdout")
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            std::io::Error::other("installer runner failed to capture child stderr")
        })?;

        let stdout_reader = std::thread::spawn(move || drain_bounded(stdout));
        let stderr_reader = std::thread::spawn(move || drain_bounded(stderr));
        let status = child.wait();
        let stdout = join_output_reader(stdout_reader, "stdout")?;
        let stderr = join_output_reader(stderr_reader, "stderr")?;
        let status = status?;

        Ok(RunnerOutput {
            status: status.code().unwrap_or(-1),
            stdout,
            stderr,
        })
    }
}

#[cfg(feature = "lua")]
fn drain_bounded(mut stream: impl std::io::Read) -> std::io::Result<Vec<u8>> {
    const LIMIT: usize = 64 * 1024;
    let mut captured = Vec::with_capacity(LIMIT);
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Ok(captured);
        }
        let remaining = LIMIT.saturating_sub(captured.len());
        captured.extend_from_slice(&buffer[..read.min(remaining)]);
    }
}

#[cfg(feature = "lua")]
fn join_output_reader(
    reader: std::thread::JoinHandle<std::io::Result<Vec<u8>>>,
    stream: &str,
) -> std::io::Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| std::io::Error::other(format!("installer runner {stream} reader panicked")))?
}

#[cfg(feature = "lua")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInstallResult {
    pub package_id: PackageId,
    pub repository_id: String,
    pub version: String,
    pub artifacts: Vec<StagedArtifact>,
    pub command: ResolvedInstallerCommand,
    pub status: String,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
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
    prepare_app_with_transports(
        data_dir,
        package_id,
        github_release_transport,
        download_transport,
    )
    .map(|(result, _, _)| result)
}

#[cfg(feature = "lua")]
fn prepare_app_with_transports(
    data_dir: &Path,
    package_id: &PackageId,
    github_release_transport: Option<Rc<dyn GithubReleaseTransport>>,
    download_transport: Rc<dyn crate::download::RuntimeDownloadTransport>,
) -> Result<
    (
        AppDownloadResult,
        getter_core::UpdateCandidate,
        Vec<InstalledTarget>,
    ),
    AppOperationError,
> {
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
    let downloads = absolute_staging_root(data_dir)?;
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

    let installed_targets = evaluation.package.installed.clone();
    let artifacts = prepared
        .into_iter()
        .map(|artifact| stage_artifact(artifact, download_transport.as_ref()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((
        AppDownloadResult {
            package_id: package_id.clone(),
            repository_id: evaluation.package.repository.to_string(),
            version: candidate.version.clone(),
            artifacts,
        },
        candidate,
        installed_targets,
    ))
}

#[cfg(feature = "lua")]
pub(crate) fn seal_android_apk_install_plan(
    package_path: &Path,
    package: &ResolvedPackage,
    selected: Option<&SelectedUpdate>,
) -> Result<Option<SealedAndroidApkInstallPlan>, AppOperationError> {
    let Some(selected) = selected else {
        return Ok(None);
    };
    let Some(declaration) = selected.candidate.install.clone() else {
        return Ok(None);
    };
    let installer = declaration
        .parse()
        .map_err(|error| AppOperationError::InstallerSchema(error.to_string()))?;
    let Installer::AndroidApk(installer) = installer else {
        return Ok(None);
    };
    let [InstalledTarget::AndroidPackage { package_name }] = package.installed.as_slice() else {
        return Err(AppOperationError::InstallerTargetUnsupported);
    };
    if package_name.trim().is_empty() {
        return Err(AppOperationError::InstallerTargetUnsupported);
    }
    let declared_artifact = selected
        .candidate
        .artifacts
        .iter()
        .find(|artifact| artifact.name == installer.artifact.artifact)
        .ok_or_else(|| {
            AppOperationError::InstallerArtifactUnknown(installer.artifact.artifact.clone())
        })?;
    if selected
        .artifact
        .as_ref()
        .map(|artifact| artifact.name.as_str())
        != Some(declared_artifact.name.as_str())
    {
        return Err(AppOperationError::InstallerArtifactUnsupported(
            declared_artifact.name.clone(),
        ));
    }
    let declared_file_name = declared_artifact
        .file_name
        .as_deref()
        .unwrap_or(declared_artifact.name.as_str());
    if !declared_file_name.to_ascii_lowercase().ends_with(".apk") {
        return Err(AppOperationError::InstallerArtifactUnsupported(
            declared_artifact.name.clone(),
        ));
    }
    let manifest_content =
        fs::read_to_string(package_path.join(PACKAGE_MANIFEST_FILE)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                AppOperationError::ManifestMissing
            } else {
                AppOperationError::ArtifactIo(error)
            }
        })?;
    let manifest =
        PackageManifest::parse(&manifest_content).map_err(AppOperationError::ManifestInvalid)?;
    let artifact_sha256 = manifest_artifact_digest(&manifest, declared_artifact)?;

    Ok(Some(SealedAndroidApkInstallPlan {
        repository_id: package.repository.to_string(),
        package_version: selected.candidate.version.clone(),
        package_name: package_name.clone(),
        artifact_name: declared_artifact.name.clone(),
        artifact_file_name: crate::download::safe_download_file_name(declared_file_name),
        artifact_sha256,
    }))
}

#[cfg(feature = "lua")]
pub fn prepare_platform_install_for_task(
    runtime: &GetterRuntime,
    data_dir: &Path,
    task_id: &str,
) -> Result<PlatformInstallHandoff, AppOperationError> {
    let task = runtime
        .task(task_id)
        .map_err(crate::runtime::RuntimeOperationError::from)?;
    if task.status != RuntimeTaskStatus::Running
        || task.phase
            != TaskPhase::with_reason(
                TaskPhaseCategory::WaitingUser,
                TaskPhaseReason::InstallHandoff,
            )
    {
        return Err(crate::runtime::RuntimeOperationError::from(
            RuntimeError::TaskNotWaitingForUser(task_id.to_owned()),
        )
        .into());
    }
    let plan = runtime
        .android_apk_install_plan(task_id)
        .map_err(crate::runtime::RuntimeOperationError::from)?
        .ok_or_else(|| AppOperationError::TaskInstallPlanMissing {
            task_id: task_id.to_owned(),
        })?;
    let downloaded =
        task.downloaded_file
            .ok_or_else(|| AppOperationError::TaskInstallArtifactMissing {
                task_id: task_id.to_owned(),
            })?;
    let expected_path =
        data_dir
            .join("downloads")
            .join(task_id)
            .join(crate::download::safe_download_file_name(
                &plan.artifact_file_name,
            ));
    let actual_path = PathBuf::from(&downloaded.local_path);
    if downloaded.file_name != plan.artifact_file_name || actual_path != expected_path {
        return Err(AppOperationError::TaskInstallArtifactMismatch {
            task_id: task_id.to_owned(),
            detail: "file name or Getter-owned task path changed".to_owned(),
        });
    }
    if downloaded.sha256 != plan.artifact_sha256 {
        return Err(AppOperationError::Sha256Mismatch {
            artifact: plan.artifact_name,
            expected: plan.artifact_sha256,
            actual: downloaded.sha256,
        });
    }
    let metadata = fs::metadata(&actual_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            AppOperationError::TaskInstallArtifactMissing {
                task_id: task_id.to_owned(),
            }
        } else {
            AppOperationError::ArtifactIo(error)
        }
    })?;
    if metadata.len() != downloaded.size_bytes {
        return Err(AppOperationError::TaskInstallArtifactMismatch {
            task_id: task_id.to_owned(),
            detail: "file size changed after Getter staging".to_owned(),
        });
    }
    let current_sha256 = sha256_file(&actual_path)?;
    if current_sha256 != plan.artifact_sha256 {
        return Err(AppOperationError::Sha256Mismatch {
            artifact: plan.artifact_name,
            expected: plan.artifact_sha256,
            actual: current_sha256,
        });
    }

    Ok(PlatformInstallHandoff {
        format: PLATFORM_INSTALL_HANDOFF_FORMAT.into(),
        version: PLATFORM_INSTALL_HANDOFF_VERSION,
        task_id: Some(task.task_id),
        package_id: task.package_id,
        repository_id: plan.repository_id,
        package_version: plan.package_version,
        request: PlatformInstallRequest::AndroidApk {
            target: AndroidInstallTarget {
                kind: "android".into(),
                package_name: plan.package_name,
            },
            artifact: StagedArtifact {
                name: plan.artifact_name,
                path: actual_path,
                sha256: plan.artifact_sha256,
                status: "downloaded".to_owned(),
            },
        },
    })
}

#[cfg(feature = "lua")]
pub fn prepare_platform_install(
    data_dir: &Path,
    package_id: &PackageId,
) -> Result<PlatformInstallHandoff, AppOperationError> {
    prepare_platform_install_with_transports(
        data_dir,
        package_id,
        Some(Rc::new(
            crate::github_releases::UreqGithubReleaseTransport::new(),
        )),
        Rc::new(crate::download::UreqRuntimeDownloadTransport::new()),
    )
}

#[cfg(feature = "lua")]
pub fn prepare_platform_install_with_transports(
    data_dir: &Path,
    package_id: &PackageId,
    github_release_transport: Option<Rc<dyn GithubReleaseTransport>>,
    download_transport: Rc<dyn crate::download::RuntimeDownloadTransport>,
) -> Result<PlatformInstallHandoff, AppOperationError> {
    let (download, candidate, installed_targets) = prepare_app_with_transports(
        data_dir,
        package_id,
        github_release_transport,
        download_transport,
    )?;
    let installer = candidate
        .install
        .ok_or(AppOperationError::InstallerMissing)?
        .parse()
        .map_err(|error| AppOperationError::InstallerSchema(error.to_string()))?;
    let Installer::AndroidApk(installer) = installer else {
        return Err(AppOperationError::InstallerTargetUnsupported);
    };
    let [InstalledTarget::AndroidPackage { package_name }] = installed_targets.as_slice() else {
        return Err(AppOperationError::InstallerTargetUnsupported);
    };
    if package_name.trim().is_empty() {
        return Err(AppOperationError::InstallerTargetUnsupported);
    }
    let declared_artifact = candidate
        .artifacts
        .iter()
        .find(|artifact| artifact.name == installer.artifact.artifact)
        .ok_or_else(|| {
            AppOperationError::InstallerArtifactUnknown(installer.artifact.artifact.clone())
        })?;
    let declared_file_name = declared_artifact
        .file_name
        .as_deref()
        .unwrap_or(&declared_artifact.name);
    if !declared_file_name.to_ascii_lowercase().ends_with(".apk") {
        return Err(AppOperationError::InstallerArtifactUnsupported(
            installer.artifact.artifact,
        ));
    }
    let artifact = unique_staged_artifact(&download.artifacts, &declared_artifact.name)?;

    Ok(PlatformInstallHandoff {
        format: PLATFORM_INSTALL_HANDOFF_FORMAT.into(),
        version: PLATFORM_INSTALL_HANDOFF_VERSION,
        task_id: None,
        package_id: download.package_id,
        repository_id: download.repository_id,
        package_version: download.version,
        request: PlatformInstallRequest::AndroidApk {
            target: AndroidInstallTarget {
                kind: "android".into(),
                package_name: package_name.clone(),
            },
            artifact: artifact.clone(),
        },
    })
}

#[cfg(feature = "lua")]
pub fn install_app(
    data_dir: &Path,
    package_id: &PackageId,
) -> Result<AppInstallResult, AppOperationError> {
    install_app_with_observer(data_dir, package_id, &NoopCommandObserver)
}

#[cfg(feature = "lua")]
pub fn install_app_with_observer(
    data_dir: &Path,
    package_id: &PackageId,
    observer: &dyn CommandObserver,
) -> Result<AppInstallResult, AppOperationError> {
    install_app_with_dependencies_and_observer(
        data_dir,
        package_id,
        Some(Rc::new(
            crate::github_releases::UreqGithubReleaseTransport::new(),
        )),
        Rc::new(crate::download::UreqRuntimeDownloadTransport::new()),
        &PathCommandResolver,
        &ProcessCommandRunner,
        observer,
    )
}

#[cfg(feature = "lua")]
pub fn install_app_with_dependencies(
    data_dir: &Path,
    package_id: &PackageId,
    github_release_transport: Option<Rc<dyn GithubReleaseTransport>>,
    download_transport: Rc<dyn crate::download::RuntimeDownloadTransport>,
    resolver: &dyn CommandResolver,
    runner: &dyn CommandRunner,
) -> Result<AppInstallResult, AppOperationError> {
    install_app_with_dependencies_and_observer(
        data_dir,
        package_id,
        github_release_transport,
        download_transport,
        resolver,
        runner,
        &NoopCommandObserver,
    )
}

#[cfg(feature = "lua")]
pub fn install_app_with_dependencies_and_observer(
    data_dir: &Path,
    package_id: &PackageId,
    github_release_transport: Option<Rc<dyn GithubReleaseTransport>>,
    download_transport: Rc<dyn crate::download::RuntimeDownloadTransport>,
    resolver: &dyn CommandResolver,
    runner: &dyn CommandRunner,
    observer: &dyn CommandObserver,
) -> Result<AppInstallResult, AppOperationError> {
    let (download, candidate, _) = prepare_app_with_transports(
        data_dir,
        package_id,
        github_release_transport,
        download_transport,
    )?;
    let installer = candidate
        .install
        .ok_or(AppOperationError::InstallerMissing)?
        .parse()
        .map_err(|error| AppOperationError::InstallerSchema(error.to_string()))?;
    let Installer::Command(installer) = installer else {
        return Err(AppOperationError::InstallerTargetUnsupported);
    };
    validate_installer_command(&installer)?;
    let command = resolve_installer(&installer, &download.artifacts, resolver)?;
    observer
        .before_execute(&command)
        .map_err(AppOperationError::InstallerCommandSpawnFailed)?;
    let output = runner
        .run(&command)
        .map_err(AppOperationError::InstallerCommandSpawnFailed)?;
    let stdout = bounded_output(&output.stdout);
    let stderr = bounded_output(&output.stderr);
    if output.status != 0 {
        return Err(AppOperationError::InstallerCommandFailed(output.status));
    }
    Ok(AppInstallResult {
        package_id: download.package_id,
        repository_id: download.repository_id,
        version: download.version,
        artifacts: download.artifacts,
        command,
        status: "succeeded".into(),
        exit_code: output.status,
        stdout,
        stderr,
    })
}

#[cfg(feature = "lua")]
fn unique_staged_artifact<'a>(
    artifacts: &'a [StagedArtifact],
    name: &str,
) -> Result<&'a StagedArtifact, AppOperationError> {
    let mut matches = artifacts.iter().filter(|artifact| artifact.name == name);
    let artifact = matches
        .next()
        .ok_or_else(|| AppOperationError::InstallerArtifactUnknown(name.to_owned()))?;
    if matches.next().is_some() {
        return Err(AppOperationError::InstallerArtifactDuplicate(
            name.to_owned(),
        ));
    }
    Ok(artifact)
}

#[cfg(feature = "lua")]
fn resolve_installer(
    installer: &InstallerCommand,
    artifacts: &[StagedArtifact],
    resolver: &dyn CommandResolver,
) -> Result<ResolvedInstallerCommand, AppOperationError> {
    let executable = resolver
        .resolve(&installer.executable)
        .ok_or_else(|| AppOperationError::InstallerCommandNotFound(installer.executable.clone()))?;
    let mut by_name = std::collections::HashMap::new();
    for artifact in artifacts {
        if by_name
            .insert(artifact.name.as_str(), &artifact.path)
            .is_some()
        {
            return Err(AppOperationError::InstallerArtifactDuplicate(
                artifact.name.clone(),
            ));
        }
    }
    let args = installer
        .args
        .iter()
        .map(|arg| match arg {
            InstallerArg::Literal(value) => Ok(value.clone()),
            InstallerArg::Artifact(reference) => by_name
                .get(reference.artifact.as_str())
                .map(|path| path.to_string_lossy().into_owned())
                .ok_or_else(|| {
                    AppOperationError::InstallerArtifactUnknown(reference.artifact.clone())
                }),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ResolvedInstallerCommand { executable, args })
}

#[cfg(feature = "lua")]
fn validate_installer_command(installer: &InstallerCommand) -> Result<(), AppOperationError> {
    if installer.executable.is_empty() {
        return Err(AppOperationError::InstallerSchema(
            "installer executable must not be empty".to_owned(),
        ));
    }
    if installer.args.iter().any(
        |arg| matches!(arg, InstallerArg::Artifact(reference) if reference.artifact.is_empty()),
    ) {
        return Err(AppOperationError::InstallerSchema(
            "installer artifact reference must not be empty".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(feature = "lua")]
fn bounded_output(bytes: &[u8]) -> String {
    const LIMIT: usize = 64 * 1024;
    String::from_utf8_lossy(&bytes[..bytes.len().min(LIMIT)]).into_owned()
}

#[cfg(all(test, feature = "lua"))]
mod installer_staging_tests {
    use super::absolute_staging_root;

    #[test]
    fn relative_nonexistent_data_dir_produces_absolute_canonical_staging_root() {
        let name = format!("target/relative-staging-test-{}", std::process::id());
        let data_dir = std::path::Path::new(&name);
        let _ = std::fs::remove_dir_all(data_dir);
        let root = absolute_staging_root(data_dir).unwrap();
        assert!(root.is_absolute());
        assert_eq!(root.file_name().unwrap(), "downloads");
        assert!(root.parent().unwrap().is_dir());
        std::fs::remove_dir_all(data_dir).unwrap();
    }
}

#[cfg(feature = "lua")]
fn absolute_staging_root(data_dir: &Path) -> Result<PathBuf, AppOperationError> {
    let absolute_data_dir = if data_dir.is_absolute() {
        data_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(AppOperationError::ArtifactIo)?
            .join(data_dir)
    };
    fs::create_dir_all(&absolute_data_dir)?;
    Ok(fs::canonicalize(absolute_data_dir)?.join("downloads"))
}

#[cfg(feature = "lua")]
fn manifest_artifact_digest(
    manifest: &PackageManifest,
    artifact: &UpdateArtifact,
) -> Result<String, AppOperationError> {
    let file_name = artifact
        .file_name
        .as_deref()
        .unwrap_or(artifact.name.as_str());
    let members = manifest.artifact_sha256_members(file_name);
    if let Some(hint) = artifact.sha256.as_deref() {
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
        return Ok(hint);
    }
    match members {
        [] => Err(AppOperationError::ManifestArtifactMissing {
            artifact: artifact.name.clone(),
            file_name: file_name.to_owned(),
        }),
        [digest] => Ok(digest.clone()),
        _ => Err(AppOperationError::ManifestArtifactAmbiguous {
            artifact: artifact.name.clone(),
            file_name: file_name.to_owned(),
        }),
    }
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
    let digest = manifest_artifact_digest(manifest, artifact)?;
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
fn sha256_file(path: &Path) -> Result<String, std::io::Error> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
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
