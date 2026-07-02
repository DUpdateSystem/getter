//! Update selection helpers owned by getter core.

use crate::{
    PackageId, PackageKind, SelectedUpdate, UpdateAction, UpdateArtifact, UpdateCandidate,
};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

pub const OFFLINE_UPDATE_CHECK_FORMAT: &str = "getter-offline-update-check";
pub const OFFLINE_UPDATE_CHECK_VERSION: u32 = 1;

/// User state that affects update selection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSelectionPolicy {
    /// User-selected local version override used as the comparison baseline.
    #[serde(default, alias = "ignored_version")]
    pub pin_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfflineUpdateCheckFixture {
    pub format: String,
    pub version: u32,
    pub package_id: PackageId,
    #[serde(default)]
    pub installed_version: Option<String>,
    #[serde(default, alias = "ignored_version")]
    pub pin_version: Option<String>,
    #[serde(default)]
    pub candidates: Vec<UpdateCandidate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfflineUpdateCheckResult {
    pub network_required: bool,
    pub package_id: PackageId,
    #[serde(default)]
    pub installed_version: Option<String>,
    #[serde(default)]
    pub effective_local_version: Option<String>,
    pub policy: UpdateSelectionPolicy,
    pub status: UpdateCheckStatus,
    #[serde(default)]
    pub selected: Option<SelectedUpdate>,
    pub actions: Vec<UpdateAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateCheckStatus {
    UpdateAvailable,
    UpToDate,
    NoCandidates,
}

#[derive(Debug, thiserror::Error)]
pub enum OfflineUpdateCheckError {
    #[error("unsupported update check fixture format '{0}'")]
    UnsupportedFormat(String),
    #[error("unsupported update check fixture version {found}; expected {expected}")]
    UnsupportedVersion { found: u32, expected: u32 },
    #[error("selected update candidate '{version}' has no actionable artifacts")]
    MissingSelectedArtifact { version: String },
}

pub fn run_offline_update_check(
    fixture: OfflineUpdateCheckFixture,
) -> Result<OfflineUpdateCheckResult, OfflineUpdateCheckError> {
    if fixture.format != OFFLINE_UPDATE_CHECK_FORMAT {
        return Err(OfflineUpdateCheckError::UnsupportedFormat(fixture.format));
    }
    if fixture.version != OFFLINE_UPDATE_CHECK_VERSION {
        return Err(OfflineUpdateCheckError::UnsupportedVersion {
            found: fixture.version,
            expected: OFFLINE_UPDATE_CHECK_VERSION,
        });
    }

    let policy = UpdateSelectionPolicy {
        pin_version: fixture.pin_version,
    };
    check_updates_offline(
        fixture.package_id,
        fixture.installed_version,
        fixture.candidates,
        policy,
    )
}

pub fn check_updates_offline(
    package_id: PackageId,
    installed_version: Option<String>,
    candidates: Vec<UpdateCandidate>,
    policy: UpdateSelectionPolicy,
) -> Result<OfflineUpdateCheckResult, OfflineUpdateCheckError> {
    let effective_local_version = policy
        .pin_version
        .clone()
        .or_else(|| installed_version.clone());
    let selected = select_update(
        package_id.clone(),
        effective_local_version.as_deref(),
        &candidates,
        &policy,
    );
    let status = update_check_status(selected.as_ref(), &candidates);
    let actions = match selected.as_ref() {
        Some(selected) => {
            if selected.artifact.is_none() {
                return Err(OfflineUpdateCheckError::MissingSelectedArtifact {
                    version: selected.candidate.version.clone(),
                });
            }
            update_actions_for_selected(selected)
        }
        None => Vec::new(),
    };

    Ok(OfflineUpdateCheckResult {
        network_required: false,
        package_id,
        installed_version,
        effective_local_version,
        policy,
        status,
        selected,
        actions,
    })
}

pub fn update_actions_for_selected(selected: &SelectedUpdate) -> Vec<UpdateAction> {
    let Some(artifact) = selected.artifact.as_ref() else {
        return Vec::new();
    };
    let file_name = artifact_file_name(artifact);
    vec![
        UpdateAction::Download {
            url: artifact.url.clone(),
            file_name: file_name.clone(),
        },
        UpdateAction::Install {
            installer: installer_for_package_kind(selected.package_id.kind()).to_owned(),
            file: file_name,
        },
    ]
}

fn artifact_file_name(artifact: &UpdateArtifact) -> String {
    artifact
        .file_name
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| artifact.name.clone())
}

fn installer_for_package_kind(kind: PackageKind) -> &'static str {
    match kind {
        PackageKind::Android => "android_package",
        PackageKind::Magisk => "magisk_module",
        PackageKind::Generic => "generic_file",
    }
}

fn update_check_status(
    selected: Option<&SelectedUpdate>,
    candidates: &[UpdateCandidate],
) -> UpdateCheckStatus {
    if candidates.is_empty() {
        UpdateCheckStatus::NoCandidates
    } else if selected.is_some() {
        UpdateCheckStatus::UpdateAvailable
    } else {
        UpdateCheckStatus::UpToDate
    }
}

/// Compare human-facing version strings using a deterministic token ordering.
///
/// This intentionally starts small: digit runs compare numerically, ASCII text
/// runs compare case-insensitively, common separators are ignored, and a text
/// suffix such as `beta` or `rc` sorts before the final release with the same
/// numeric prefix.
///
/// Examples:
///
/// - `1.10` > `1.2`
/// - `1.0.0` == `1.0`
/// - `1.0.0-beta` < `1.0.0`
pub fn compare_versions(left: &str, right: &str) -> Ordering {
    let left = tokenize_version(left);
    let right = tokenize_version(right);
    let max_len = left.len().max(right.len());

    for index in 0..max_len {
        match (left.get(index), right.get(index)) {
            (Some(left), Some(right)) => {
                let ordering = left.cmp(right);
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some(VersionToken::Number(value)), None) if *value == 0 => continue,
            (None, Some(VersionToken::Number(value))) if *value == 0 => continue,
            (Some(VersionToken::Text(_)), None) => return Ordering::Less,
            (None, Some(VersionToken::Text(_))) => return Ordering::Greater,
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => break,
        }
    }

    Ordering::Equal
}

/// Select the best update candidate for a package.
///
/// The selected candidate is the highest version that is newer than the
/// effective local baseline. The effective baseline is normally the observed
/// installed version; callers pass `pin_version` instead when the user has set
/// a baseline override. If no baseline is known, the highest candidate is
/// selected.
pub fn select_update(
    package_id: PackageId,
    installed_version: Option<&str>,
    candidates: &[UpdateCandidate],
    policy: &UpdateSelectionPolicy,
) -> Option<SelectedUpdate> {
    candidates
        .iter()
        .filter(|candidate| is_selectable(candidate, installed_version, policy))
        .max_by(|left, right| compare_versions(&left.version, &right.version))
        .cloned()
        .map(|candidate| SelectedUpdate {
            package_id,
            artifact: first_artifact(&candidate),
            candidate,
        })
}

fn is_selectable(
    candidate: &UpdateCandidate,
    installed_version: Option<&str>,
    policy: &UpdateSelectionPolicy,
) -> bool {
    let _ = policy;

    match installed_version {
        Some(installed) => compare_versions(&candidate.version, installed) == Ordering::Greater,
        None => true,
    }
}

fn first_artifact(candidate: &UpdateCandidate) -> Option<UpdateArtifact> {
    candidate.artifacts.first().cloned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum VersionToken {
    Number(u128),
    Text(String),
}

impl Ord for VersionToken {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Number(left), Self::Number(right)) => left.cmp(right),
            (Self::Text(left), Self::Text(right)) => left.cmp(right),
            (Self::Number(_), Self::Text(_)) => Ordering::Greater,
            (Self::Text(_), Self::Number(_)) => Ordering::Less,
        }
    }
}

impl PartialOrd for VersionToken {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn tokenize_version(version: &str) -> Vec<VersionToken> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut current_kind: Option<TokenKind> = None;

    for ch in version.chars() {
        let kind = TokenKind::from_char(ch);
        match (kind, current_kind) {
            (Some(kind), Some(existing)) if kind == existing => current.push(ch),
            (Some(kind), Some(_)) => {
                push_token(&mut tokens, &current, current_kind.expect("kind exists"));
                current.clear();
                current.push(ch);
                current_kind = Some(kind);
            }
            (Some(kind), None) => {
                current.push(ch);
                current_kind = Some(kind);
            }
            (None, Some(existing)) => {
                push_token(&mut tokens, &current, existing);
                current.clear();
                current_kind = None;
            }
            (None, None) => {}
        }
    }

    if let Some(kind) = current_kind {
        push_token(&mut tokens, &current, kind);
    }

    tokens
}

fn push_token(tokens: &mut Vec<VersionToken>, value: &str, kind: TokenKind) {
    match kind {
        TokenKind::Number => tokens.push(VersionToken::Number(parse_u128_lossy(value))),
        TokenKind::Text => tokens.push(VersionToken::Text(value.to_ascii_lowercase())),
    }
}

fn parse_u128_lossy(value: &str) -> u128 {
    value.parse::<u128>().unwrap_or(u128::MAX)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Number,
    Text,
}

impl TokenKind {
    fn from_char(ch: char) -> Option<Self> {
        if ch.is_ascii_digit() {
            Some(Self::Number)
        } else if ch.is_ascii_alphabetic() {
            Some(Self::Text)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_numeric_segments_numerically() {
        assert_eq!(compare_versions("1.10", "1.2"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0", "1.0"), Ordering::Equal);
        assert_eq!(compare_versions("2", "10"), Ordering::Less);
    }

    #[test]
    fn treats_prerelease_suffix_as_older_than_release() {
        assert_eq!(compare_versions("1.0.0-beta", "1.0.0"), Ordering::Less);
        assert_eq!(
            compare_versions("1.0.0-rc1", "1.0.0-beta2"),
            Ordering::Greater
        );
    }

    #[test]
    fn selects_highest_newer_candidate() {
        let selected = select_update(
            "android/org.fdroid.fdroid".parse().unwrap(),
            Some("1.0.0"),
            &[candidate("1.0.1"), candidate("1.2.0"), candidate("0.9.0")],
            &UpdateSelectionPolicy::default(),
        )
        .unwrap();

        assert_eq!(selected.candidate.version, "1.2.0");
    }

    #[test]
    fn pin_version_overrides_comparison_baseline() {
        let result = check_updates_offline(
            "android/org.fdroid.fdroid".parse().unwrap(),
            Some("1.0.0".to_owned()),
            vec![candidate("1.1.0"), candidate("1.2.0")],
            UpdateSelectionPolicy {
                pin_version: Some("1.2.0".to_owned()),
            },
        )
        .unwrap();

        assert_eq!(result.status, UpdateCheckStatus::UpToDate);
        assert_eq!(result.installed_version.as_deref(), Some("1.0.0"));
        assert_eq!(result.effective_local_version.as_deref(), Some("1.2.0"));
        assert!(result.selected.is_none());
    }

    #[test]
    fn returns_none_when_no_candidate_is_newer() {
        assert!(select_update(
            "android/org.fdroid.fdroid".parse().unwrap(),
            Some("2.0.0"),
            &[candidate("1.9.0"), candidate("2.0.0")],
            &UpdateSelectionPolicy::default(),
        )
        .is_none());
    }

    #[test]
    fn unknown_installed_version_selects_highest_candidate() {
        let selected = select_update(
            "android/org.fdroid.fdroid".parse().unwrap(),
            None,
            &[candidate("1.0.0-beta"), candidate("1.0.0")],
            &UpdateSelectionPolicy::default(),
        )
        .unwrap();

        assert_eq!(selected.candidate.version, "1.0.0");
        assert_eq!(
            selected.artifact.as_ref().unwrap().file_name.as_deref(),
            Some("app.apk")
        );
    }

    #[test]
    fn offline_update_check_reports_update_available_with_actions() {
        let result = run_offline_update_check(OfflineUpdateCheckFixture {
            format: OFFLINE_UPDATE_CHECK_FORMAT.to_owned(),
            version: OFFLINE_UPDATE_CHECK_VERSION,
            package_id: "android/org.fdroid.fdroid".parse().unwrap(),
            installed_version: Some("1.0.0".to_owned()),
            pin_version: None,
            candidates: vec![candidate("1.0.1"), candidate("1.2.0")],
        })
        .unwrap();

        assert_eq!(result.status, UpdateCheckStatus::UpdateAvailable);
        assert_eq!(result.selected.as_ref().unwrap().candidate.version, "1.2.0");
        assert_eq!(
            result.actions,
            vec![
                UpdateAction::Download {
                    url: "https://example.invalid/1.2.0.apk".to_owned(),
                    file_name: "app.apk".to_owned()
                },
                UpdateAction::Install {
                    installer: "android_package".to_owned(),
                    file: "app.apk".to_owned()
                }
            ]
        );
    }

    #[test]
    fn offline_update_check_reports_up_to_date() {
        let result = check_updates_offline(
            "android/org.fdroid.fdroid".parse().unwrap(),
            Some("2.0.0".to_owned()),
            vec![candidate("1.9.0"), candidate("2.0.0")],
            UpdateSelectionPolicy::default(),
        )
        .unwrap();

        assert_eq!(result.status, UpdateCheckStatus::UpToDate);
        assert!(result.selected.is_none());
        assert!(result.actions.is_empty());
    }

    #[test]
    fn offline_update_check_uses_pin_version_as_baseline() {
        let result = check_updates_offline(
            "android/org.fdroid.fdroid".parse().unwrap(),
            Some("1.0.0".to_owned()),
            vec![candidate("1.1.0"), candidate("1.2.0"), candidate("1.3.0")],
            UpdateSelectionPolicy {
                pin_version: Some("1.2.0".to_owned()),
            },
        )
        .unwrap();

        assert_eq!(result.status, UpdateCheckStatus::UpdateAvailable);
        assert_eq!(result.selected.as_ref().unwrap().candidate.version, "1.3.0");
        assert_eq!(result.effective_local_version.as_deref(), Some("1.2.0"));
    }

    #[test]
    fn offline_update_check_reports_no_candidates() {
        let result = check_updates_offline(
            "android/org.fdroid.fdroid".parse().unwrap(),
            Some("1.0.0".to_owned()),
            Vec::new(),
            UpdateSelectionPolicy::default(),
        )
        .unwrap();

        assert_eq!(result.status, UpdateCheckStatus::NoCandidates);
        assert!(result.selected.is_none());
        assert!(result.actions.is_empty());
    }

    #[test]
    fn offline_update_check_rejects_selected_candidate_without_artifacts() {
        let error = check_updates_offline(
            "android/org.fdroid.fdroid".parse().unwrap(),
            Some("1.0.0".to_owned()),
            vec![UpdateCandidate {
                version: "1.2.0".to_owned(),
                version_code: None,
                changelog: None,
                channel: None,
                source: None,
                artifacts: Vec::new(),
            }],
            UpdateSelectionPolicy::default(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            OfflineUpdateCheckError::MissingSelectedArtifact { .. }
        ));
    }

    #[test]
    fn offline_update_check_rejects_wrong_contract() {
        let error = run_offline_update_check(OfflineUpdateCheckFixture {
            format: "wrong".to_owned(),
            version: OFFLINE_UPDATE_CHECK_VERSION,
            package_id: "android/org.fdroid.fdroid".parse().unwrap(),
            installed_version: None,
            pin_version: None,
            candidates: Vec::new(),
        })
        .unwrap_err();

        assert!(matches!(
            error,
            OfflineUpdateCheckError::UnsupportedFormat(_)
        ));
    }

    fn candidate(version: &str) -> UpdateCandidate {
        UpdateCandidate {
            version: version.to_owned(),
            version_code: None,
            changelog: None,
            channel: None,
            source: None,
            artifacts: vec![UpdateArtifact {
                name: "APK".to_owned(),
                url: format!("https://example.invalid/{version}.apk"),
                content_type: None,
                file_name: Some("app.apk".to_owned()),
                sha256: None,
                size: None,
            }],
        }
    }
}
