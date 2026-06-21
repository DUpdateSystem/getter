//! Update selection helpers owned by getter core.

use crate::{PackageId, SelectedUpdate, UpdateArtifact, UpdateCandidate};
use std::cmp::Ordering;

/// User state that affects update selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateSelectionPolicy {
    /// Candidate version the user chose to ignore/mark as skipped.
    pub ignored_version: Option<String>,
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
/// installed version and not equal to the user's ignored version. If no
/// installed version is known, the highest non-ignored candidate is selected.
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
    if policy
        .ignored_version
        .as_deref()
        .is_some_and(|ignored| compare_versions(&candidate.version, ignored) == Ordering::Equal)
    {
        return false;
    }

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
    fn respects_ignored_version() {
        let selected = select_update(
            "android/org.fdroid.fdroid".parse().unwrap(),
            Some("1.0.0"),
            &[candidate("1.1.0"), candidate("1.2.0")],
            &UpdateSelectionPolicy {
                ignored_version: Some("1.2.0".to_owned()),
            },
        )
        .unwrap();

        assert_eq!(selected.candidate.version, "1.1.0");
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

    fn candidate(version: &str) -> UpdateCandidate {
        UpdateCandidate {
            version: version.to_owned(),
            channel: None,
            source: None,
            artifacts: vec![UpdateArtifact {
                name: "APK".to_owned(),
                url: format!("https://example.invalid/{version}.apk"),
                file_name: Some("app.apk".to_owned()),
            }],
        }
    }
}
