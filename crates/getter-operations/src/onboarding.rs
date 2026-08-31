//! Getter-owned fresh-install setup preview and apply orchestration.

use crate::autogen::{self, AutogenAcceptance, AutogenApplySpec, AutogenOperationError};
use crate::fdroid_autogen;
use crate::fdroid_signed_index::{
    verify_fdroid_index_jar, MAX_ARCHIVE_BYTES, OFFICIAL_INDEX_JAR_URL,
};
use getter_core::autogen::{
    validate_installed_inventory, InstalledInventory, InstalledInventoryItem,
};
use getter_core::PackageId;
use getter_storage::{CacheDb, MainDb};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha512};
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub trait FdroidCatalogTransport {
    fn fetch_official_index_xml(&self) -> Result<String, String>;
}

pub struct UreqFdroidCatalogTransport;

impl FdroidCatalogTransport for UreqFdroidCatalogTransport {
    fn fetch_official_index_xml(&self) -> Result<String, String> {
        let response = ureq::get(OFFICIAL_INDEX_JAR_URL)
            .call()
            .map_err(|error| format!("official F-Droid index.jar request failed: {error}"))?;
        let mut body = response.into_body().into_reader();
        let mut jar = Vec::new();
        body.by_ref()
            .take((MAX_ARCHIVE_BYTES + 1) as u64)
            .read_to_end(&mut jar)
            .map_err(|error| format!("official F-Droid index.jar read failed: {error}"))?;
        if jar.len() > MAX_ARCHIVE_BYTES {
            return Err(format!(
                "official F-Droid index.jar exceeds the {MAX_ARCHIVE_BYTES}-byte limit"
            ));
        }
        verify_fdroid_index_jar(&jar)
            .map_err(|error| format!("official F-Droid index.jar verification failed: {error}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupReadiness {
    NeedsPackageSetup,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupStatus {
    pub state: SetupReadiness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupCandidateCategory {
    Fdroid,
    InstalledFallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupCandidate {
    pub package_id: String,
    pub display_name: Option<String>,
    pub category: SetupCandidateCategory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupDiagnostic {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupPreview {
    pub format: String,
    pub version: u32,
    pub preview_id: String,
    pub candidates: Vec<SetupCandidate>,
    pub diagnostics: Vec<SetupDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StoredSetupPreview {
    format: String,
    version: u32,
    preview_id: String,
    candidates: Vec<SetupCandidate>,
    diagnostics: Vec<SetupDiagnostic>,
    fdroid_preview: Value,
    installed_preview: Value,
    inventory: InstalledInventory,
    context_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupAcceptance {
    AcceptAll,
    Accept(Vec<PackageId>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupApplyResult {
    pub readiness: SetupReadiness,
    pub applied_package_ids: Vec<String>,
}

pub fn derive_setup_status(
    has_enabled_tracked_package: bool,
    inventory: &InstalledInventory,
) -> SetupStatus {
    SetupStatus {
        state: if !has_enabled_tracked_package && !inventory.items.is_empty() {
            SetupReadiness::NeedsPackageSetup
        } else {
            SetupReadiness::Ready
        },
    }
}

pub fn derive_setup_status_cache_only(
    data_dir: &Path,
    main_db: &MainDb,
    cache_db: &CacheDb,
    inventory: &InstalledInventory,
) -> Result<SetupStatus, AutogenOperationError> {
    if main_db
        .tracked_packages()?
        .iter()
        .any(|package| package.enabled)
    {
        return Ok(SetupStatus {
            state: SetupReadiness::Ready,
        });
    }
    let request = json!({
        "mode": "use_cached",
        "installed_inventory": inventory,
    });
    let fdroid_preview = fdroid_autogen::preview_fdroid_packages_json(
        data_dir,
        main_db,
        cache_db,
        &request.to_string(),
    )
    .unwrap_or_else(|_| json!({ "candidates": [] }));
    let matched = fdroid_handled_android_ids(&fdroid_preview);
    let fallback = InstalledInventory::new(
        inventory
            .items
            .iter()
            .filter(|item| match item {
                InstalledInventoryItem::AndroidPackage { package_name, .. } => {
                    !matched.contains(package_name)
                }
                InstalledInventoryItem::MagiskModule { .. } => true,
            })
            .cloned()
            .collect(),
    );
    let fallback_plan = autogen::build_installed_autogen_plan(data_dir, main_db, &fallback)?;
    let actionable =
        !candidate_ids(&fdroid_preview).is_empty() || !fallback_plan.candidates.is_empty();
    Ok(SetupStatus {
        state: if actionable {
            SetupReadiness::NeedsPackageSetup
        } else {
            SetupReadiness::Ready
        },
    })
}

pub fn preview_setup_with_transport(
    data_dir: &Path,
    main_db: &MainDb,
    cache_db: &CacheDb,
    inventory: InstalledInventory,
    transport: &dyn FdroidCatalogTransport,
) -> Result<SetupPreview, AutogenOperationError> {
    validate_installed_inventory(&inventory)
        .map_err(|error| AutogenOperationError::Autogen(error.to_string()))?;
    let android_names: Vec<_> = inventory
        .items
        .iter()
        .filter_map(|item| match item {
            InstalledInventoryItem::AndroidPackage { package_name, .. } => {
                Some(package_name.clone())
            }
            InstalledInventoryItem::MagiskModule { .. } => None,
        })
        .collect();
    let refresh = transport.fetch_official_index_xml();
    let refresh_failed = refresh.is_err();
    let request = match refresh {
        Ok(xml) => {
            json!({"index_xml": xml, "mode": "force_refresh", "package_names": android_names, "installed_inventory": inventory})
        }
        Err(_) => {
            json!({"mode": "force_refresh", "package_names": android_names, "installed_inventory": inventory})
        }
    };
    let mut diagnostics = Vec::new();
    let fdroid_preview = match fdroid_autogen::preview_fdroid_packages_json(
        data_dir,
        main_db,
        cache_db,
        &request.to_string(),
    ) {
        Ok(preview) => preview,
        Err(_) if refresh_failed => {
            diagnostics.push(SetupDiagnostic { code: "setup.fdroid_unavailable".into(), message: "F-Droid catalog is unavailable; installed fallback candidates are still available".into() });
            let (target_alias, target_path, _) = autogen::generated_repository_config(data_dir)?;
            json!({
                "operation": "fdroid.autogen.preview",
                "target_repo_id": target_alias.as_str(),
                "target_repo_path": target_path,
                "candidates": [],
                "diagnostics": [],
            })
        }
        Err(error) => return Err(error),
    };
    for diagnostic in fdroid_preview
        .get("diagnostics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        diagnostics.push(SetupDiagnostic {
            code: diagnostic
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("setup.fdroid_stale")
                .to_owned(),
            message: diagnostic
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("F-Droid catalog cache is stale")
                .to_owned(),
        });
    }
    let matched_android_ids = fdroid_handled_android_ids(&fdroid_preview);
    let fallback_inventory = InstalledInventory::new(
        inventory
            .items
            .iter()
            .filter(|item| match item {
                InstalledInventoryItem::AndroidPackage { package_name, .. } => {
                    !matched_android_ids.contains(package_name)
                }
                InstalledInventoryItem::MagiskModule { .. } => true,
            })
            .cloned()
            .collect(),
    );
    let installed_plan =
        autogen::build_installed_autogen_plan(data_dir, main_db, &fallback_inventory)?;
    let installed_preview = autogen::installed_preview_json(data_dir, &installed_plan)?;
    let mut candidates = Vec::new();
    append_candidates(
        &mut candidates,
        &fdroid_preview,
        SetupCandidateCategory::Fdroid,
    );
    append_candidates(
        &mut candidates,
        &installed_preview,
        SetupCandidateCategory::InstalledFallback,
    );
    candidates.sort_by(|a, b| a.package_id.cmp(&b.package_id));
    candidates.dedup_by(|a, b| a.package_id == b.package_id);
    let context_fingerprint = current_context_fingerprint(main_db)?;
    let mut random = [0_u8; 32];
    getrandom::fill(&mut random).map_err(|error| {
        AutogenOperationError::Autogen(format!("failed to create setup preview id: {error}"))
    })?;
    let preview_id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let stored = StoredSetupPreview {
        format: "getter-setup-preview".into(),
        version: 1,
        preview_id: preview_id.clone(),
        candidates: candidates.clone(),
        diagnostics: diagnostics.clone(),
        fdroid_preview,
        installed_preview,
        inventory,
        context_fingerprint,
    };
    persist_setup_preview(data_dir, &stored)?;
    Ok(SetupPreview {
        format: stored.format,
        version: stored.version,
        preview_id,
        candidates,
        diagnostics,
    })
}

pub fn preview_setup(
    data_dir: &Path,
    main_db: &MainDb,
    cache_db: &CacheDb,
    inventory: InstalledInventory,
) -> Result<SetupPreview, AutogenOperationError> {
    preview_setup_with_transport(
        data_dir,
        main_db,
        cache_db,
        inventory,
        &UreqFdroidCatalogTransport,
    )
}

pub fn apply_setup_preview(
    data_dir: &Path,
    main_db: &MainDb,
    preview: &SetupPreview,
    acceptance: &SetupAcceptance,
) -> Result<SetupApplyResult, AutogenOperationError> {
    let (trusted, claim_path, ready_path) = claim_setup_preview(data_dir, &preview.preview_id)?;
    let prepare = (|| -> Result<_, AutogenOperationError> {
        if trusted.context_fingerprint != current_context_fingerprint(main_db)? {
            return Err(AutogenOperationError::Autogen(
                "setup preview is stale because repository or tracked-package state changed".into(),
            ));
        }
        let known: BTreeSet<_> = trusted
            .candidates
            .iter()
            .map(|candidate| candidate.package_id.clone())
            .collect();
        let accepted: BTreeSet<_> = match acceptance {
            SetupAcceptance::AcceptAll => known.clone(),
            SetupAcceptance::Accept(ids) => ids.iter().map(ToString::to_string).collect(),
        };
        if let Some(unknown) = accepted.difference(&known).next() {
            return Err(AutogenOperationError::Autogen(format!(
                "unknown setup candidate '{unknown}'"
            )));
        }
        let accepted_for = |category| {
            trusted
                .candidates
                .iter()
                .filter(|candidate| candidate.category == category)
                .filter(|candidate| accepted.contains(&candidate.package_id))
                .map(|candidate| {
                    candidate.package_id.parse().map_err(|error| {
                        AutogenOperationError::Autogen(format!(
                            "invalid preview package id: {error}"
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()
                .map(AutogenAcceptance::Accept)
        };
        let fdroid_acceptance = accepted_for(SetupCandidateCategory::Fdroid)?;
        let fallback_acceptance = accepted_for(SetupCandidateCategory::InstalledFallback)?;
        Ok((accepted, fdroid_acceptance, fallback_acceptance))
    })();
    let (accepted, fdroid_acceptance, fallback_acceptance) = match prepare {
        Ok(prepared) => prepared,
        Err(error) => {
            restore_setup_preview_claim(&claim_path, &ready_path)?;
            return Err(error);
        }
    };
    let apply_result = autogen::apply_preview_batch(
        data_dir,
        main_db,
        &[
            AutogenApplySpec {
                preview: &trusted.fdroid_preview,
                acceptance: &fdroid_acceptance,
                expected_operation: "fdroid.autogen.preview",
                expected_generator: getter_core::autogen::FDROID_AUTOGEN_GENERATOR,
            },
            AutogenApplySpec {
                preview: &trusted.installed_preview,
                acceptance: &fallback_acceptance,
                expected_operation: "installed.preview",
                expected_generator: getter_core::autogen::INSTALLED_AUTOGEN_GENERATOR,
            },
        ],
    );
    if let Err(error) = apply_result {
        restore_setup_preview_claim(&claim_path, &ready_path)?;
        return Err(error);
    }
    // Any accepted generated package is enabled by the committed batch. Empty
    // acceptance leaves the still-actionable trusted candidate set unchanged.
    let readiness = if accepted.is_empty() && !trusted.candidates.is_empty() {
        SetupReadiness::NeedsPackageSetup
    } else {
        SetupReadiness::Ready
    };
    // Changes are committed; cleanup cannot turn success into a reported failure.
    let _ = fs::remove_file(&claim_path);
    Ok(SetupApplyResult {
        readiness,
        applied_package_ids: accepted.into_iter().collect(),
    })
}

fn append_candidates(
    output: &mut Vec<SetupCandidate>,
    preview: &Value,
    category: SetupCandidateCategory,
) {
    for candidate in preview
        .get("candidates")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(package_id) = candidate.get("package_id").and_then(Value::as_str) {
            output.push(SetupCandidate {
                package_id: package_id.to_owned(),
                display_name: candidate
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                category,
            });
        }
    }
}

fn candidate_ids(preview: &Value) -> Vec<String> {
    preview
        .get("candidates")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|candidate| {
            candidate
                .get("package_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

fn fdroid_handled_android_ids(preview: &Value) -> HashSet<String> {
    let mut package_ids = candidate_ids(preview);
    package_ids.extend(
        preview
            .get("skipped")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|skip| {
                skip.get("reason").and_then(Value::as_str)
                    == Some("covered_by_higher_priority_repo")
            })
            .filter_map(|skip| {
                skip.get("package_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }),
    );
    package_ids
        .into_iter()
        .filter_map(|id| id.strip_prefix("android/f-droid/app/").map(str::to_owned))
        .collect()
}

fn setup_preview_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("cache/setup-previews")
}

fn setup_preview_path(data_dir: &Path, preview_id: &str) -> Result<PathBuf, AutogenOperationError> {
    if preview_id.len() != 64 || !preview_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AutogenOperationError::Autogen(
            "setup preview id is invalid".into(),
        ));
    }
    Ok(setup_preview_dir(data_dir).join(format!("{preview_id}.json")))
}

fn persist_setup_preview(
    data_dir: &Path,
    preview: &StoredSetupPreview,
) -> Result<(), AutogenOperationError> {
    let dir = setup_preview_dir(data_dir);
    fs::create_dir_all(&dir).map_err(|error| {
        AutogenOperationError::Autogen(format!("failed to create setup preview store: {error}"))
    })?;
    let final_path = setup_preview_path(data_dir, &preview.preview_id)?;
    let temp_path = dir.join(format!(".{}.tmp", preview.preview_id));
    let bytes = serde_json::to_vec(preview)
        .map_err(|error| AutogenOperationError::Autogen(error.to_string()))?;
    fs::write(&temp_path, bytes).map_err(|error| {
        AutogenOperationError::Autogen(format!("failed to write setup preview: {error}"))
    })?;
    fs::rename(&temp_path, &final_path).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        AutogenOperationError::Autogen(format!("failed to commit setup preview: {error}"))
    })
}

fn claim_setup_preview(
    data_dir: &Path,
    preview_id: &str,
) -> Result<(StoredSetupPreview, PathBuf, PathBuf), AutogenOperationError> {
    let path = setup_preview_path(data_dir, preview_id)?;
    let claim_path = setup_preview_dir(data_dir).join(format!(".{preview_id}.applying"));
    fs::rename(&path, &claim_path).map_err(|_| {
        AutogenOperationError::Autogen(
            "setup preview is unknown, consumed, or already applying".into(),
        )
    })?;
    let metadata = fs::metadata(&claim_path).map_err(|error| {
        AutogenOperationError::Autogen(format!("failed to inspect claimed setup preview: {error}"))
    })?;
    let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
    if SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default()
        .as_secs()
        > 30 * 60
    {
        let _ = fs::remove_file(&claim_path);
        return Err(AutogenOperationError::Autogen(
            "setup preview has expired".into(),
        ));
    }
    let bytes = fs::read(&claim_path).map_err(|error| {
        AutogenOperationError::Autogen(format!("failed to read setup preview: {error}"))
    })?;
    let preview: StoredSetupPreview = serde_json::from_slice(&bytes).map_err(|error| {
        AutogenOperationError::Autogen(format!("trusted setup preview is invalid: {error}"))
    })?;
    if preview.preview_id != preview_id {
        return Err(AutogenOperationError::Autogen(
            "trusted setup preview id does not match its key".into(),
        ));
    }
    Ok((preview, claim_path, path))
}

fn restore_setup_preview_claim(
    claim_path: &Path,
    ready_path: &Path,
) -> Result<(), AutogenOperationError> {
    fs::rename(claim_path, ready_path).map_err(|error| {
        AutogenOperationError::Autogen(format!(
            "setup apply failed and its preview claim could not be restored: {error}"
        ))
    })
}

fn current_context_fingerprint(main_db: &MainDb) -> Result<String, AutogenOperationError> {
    let repositories = main_db.repositories()?;
    let tracked = main_db.tracked_packages()?;
    let repository_state: Vec<_> = repositories
        .iter()
        .map(|repo| {
            json!({
                "id": repo.id.as_str(),
                "name": repo.name,
                "priority": repo.priority.value(),
                "api_version": repo.api_version,
                "path": repo.path,
                "revision": repo.revision,
            })
        })
        .collect();
    let tracked_state: Vec<_> = tracked
        .iter()
        .map(|package| {
            json!({
                "package_id": package.package_id.to_string(),
                "enabled": package.enabled,
                "favorite": package.favorite,
                "pin_version": package.pin_version,
                "repository_id": package.repository_id.as_ref().map(ToString::to_string),
                "resolution": format!("{:?}", package.package_resolution),
            })
        })
        .collect();
    let bytes = serde_json::to_vec(&json!({
        "repositories": repository_state,
        "tracked_packages": tracked_state,
    }))
    .map_err(|error| AutogenOperationError::Autogen(error.to_string()))?;
    Ok(format!("sha512:{:x}", Sha512::digest(bytes)))
}
