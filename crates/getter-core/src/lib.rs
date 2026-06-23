//! Core domain model for the UpgradeAll getter rewrite.
//!
//! `getter-core` owns product/domain behavior. Flutter and Android code are
//! platform/UI adapters and must not reimplement package, repository, update or
//! storage rules.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

pub mod autogen;
pub mod diagnostics;
pub mod lua;
pub mod repository;
pub mod task;
pub mod update;

/// Error returned when parsing or constructing a [`PackageId`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PackageIdError {
    #[error("package id is empty")]
    Empty,
    #[error("package id must be '<kind>/<name>'")]
    MissingSeparator,
    #[error("package id kind is empty")]
    EmptyKind,
    #[error("package id name is empty")]
    EmptyName,
    #[error("unsupported package kind '{0}'")]
    UnsupportedKind(String),
    #[error("package id kind '{0}' contains invalid characters")]
    InvalidKind(String),
    #[error("package id name '{0}' contains invalid characters")]
    InvalidName(String),
}

/// Known package target kinds in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageKind {
    Android,
    Magisk,
    Generic,
}

impl PackageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Android => "android",
            Self::Magisk => "magisk",
            Self::Generic => "generic",
        }
    }
}

impl fmt::Display for PackageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PackageKind {
    type Err = PackageIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "android" => Ok(Self::Android),
            "magisk" => Ok(Self::Magisk),
            "generic" => Ok(Self::Generic),
            other => Err(PackageIdError::UnsupportedKind(other.to_owned())),
        }
    }
}

/// Readable package identifier such as `android/org.fdroid.fdroid`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PackageId {
    kind: PackageKind,
    name: String,
}

impl PackageId {
    pub fn new(kind: PackageKind, name: impl Into<String>) -> Result<Self, PackageIdError> {
        let name = name.into();
        validate_name(&name)?;
        Ok(Self { kind, name })
    }

    pub fn kind(&self) -> PackageKind {
        self.kind
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.kind, self.name)
    }
}

impl FromStr for PackageId {
    type Err = PackageIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            return Err(PackageIdError::Empty);
        }
        let (kind, name) = value
            .split_once('/')
            .ok_or(PackageIdError::MissingSeparator)?;
        if kind.is_empty() {
            return Err(PackageIdError::EmptyKind);
        }
        if name.is_empty() {
            return Err(PackageIdError::EmptyName);
        }
        validate_kind_segment(kind)?;
        validate_name(name)?;
        Ok(Self {
            kind: kind.parse()?,
            name: name.to_owned(),
        })
    }
}

impl TryFrom<String> for PackageId {
    type Error = PackageIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<PackageId> for String {
    fn from(value: PackageId) -> Self {
        value.to_string()
    }
}

/// Repository identifier such as `official`, `community`, `local`, or
/// `local_autogen`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepositoryId(String);

impl RepositoryId {
    pub fn new(value: impl Into<String>) -> Result<Self, RepositoryIdError> {
        let value = value.into();
        validate_repository_id(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RepositoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for RepositoryId {
    type Err = RepositoryIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for RepositoryId {
    type Error = RepositoryIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<RepositoryId> for String {
    fn from(value: RepositoryId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RepositoryIdError {
    #[error("repository id is empty")]
    Empty,
    #[error("repository id '{0}' contains invalid characters")]
    Invalid(String),
}

/// Higher repository priority wins during overlay resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepositoryPriority(i32);

impl RepositoryPriority {
    pub const LOCAL: Self = Self(100);
    pub const DEFAULT: Self = Self(0);
    pub const LOCAL_AUTOGEN: Self = Self(-1);

    pub const fn new(value: i32) -> Self {
        Self(value)
    }

    pub const fn value(self) -> i32 {
        self.0
    }

    pub fn cmp_winner(self, other: Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

impl Default for RepositoryPriority {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Ord for RepositoryPriority {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

impl PartialOrd for RepositoryPriority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Package metadata after repository resolution and Lua validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedPackage {
    pub id: PackageId,
    pub repository: RepositoryId,
    pub name: String,
    #[serde(default)]
    pub installed: Vec<InstalledTarget>,
    #[serde(default)]
    pub permissions: PackagePermissions,
    #[serde(default)]
    pub source_priority: Vec<String>,
}

/// Installed target matched by a package, such as an Android package name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstalledTarget {
    AndroidPackage { package_name: String },
    MagiskModule { module_id: String },
    Generic { id: String },
}

/// Package permission declaration relevant to UI warnings and Lua host APIs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagePermissions {
    #[serde(default)]
    pub free_network: bool,
}

/// Candidate version/update discovered by provider/package logic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateCandidate {
    pub version: String,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub artifacts: Vec<UpdateArtifact>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateArtifact {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub file_name: Option<String>,
}

/// Candidate selected for update after package/user-state policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectedUpdate {
    pub package_id: PackageId,
    pub candidate: UpdateCandidate,
    #[serde(default)]
    pub artifact: Option<UpdateArtifact>,
}

/// Executable update actions generated by the `resolve` lifecycle phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UpdateAction {
    Download { url: String, file_name: String },
    Install { installer: String, file: String },
    OpenUrl { url: String },
}

fn validate_kind_segment(value: &str) -> Result<(), PackageIdError> {
    if value
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        Ok(())
    } else {
        Err(PackageIdError::InvalidKind(value.to_owned()))
    }
}

fn validate_name(value: &str) -> Result<(), PackageIdError> {
    if value.is_empty() {
        return Err(PackageIdError::EmptyName);
    }
    if value.starts_with('/') || value.ends_with('/') || value.contains("//") {
        return Err(PackageIdError::InvalidName(value.to_owned()));
    }
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '/'))
    {
        Ok(())
    } else {
        Err(PackageIdError::InvalidName(value.to_owned()))
    }
}

fn validate_repository_id(value: &str) -> Result<(), RepositoryIdError> {
    if value.is_empty() {
        return Err(RepositoryIdError::Empty);
    }
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        Ok(())
    } else {
        Err(RepositoryIdError::Invalid(value.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_readable_android_package_id() {
        let id: PackageId = "android/org.fdroid.fdroid".parse().unwrap();
        assert_eq!(id.kind(), PackageKind::Android);
        assert_eq!(id.name(), "org.fdroid.fdroid");
        assert_eq!(id.to_string(), "android/org.fdroid.fdroid");
    }

    #[test]
    fn parses_readable_magisk_package_id() {
        let id: PackageId = "magisk/zygisk-next".parse().unwrap();
        assert_eq!(id.kind(), PackageKind::Magisk);
        assert_eq!(id.name(), "zygisk-next");
        assert_eq!(id.to_string(), "magisk/zygisk-next");
    }

    #[test]
    fn rejects_invalid_package_ids() {
        assert_eq!("".parse::<PackageId>(), Err(PackageIdError::Empty));
        assert_eq!(
            "android".parse::<PackageId>(),
            Err(PackageIdError::MissingSeparator)
        );
        assert_eq!(
            "/org.fdroid.fdroid".parse::<PackageId>(),
            Err(PackageIdError::EmptyKind)
        );
        assert_eq!(
            "android/".parse::<PackageId>(),
            Err(PackageIdError::EmptyName)
        );
        assert_eq!(
            "hub/org.fdroid.fdroid".parse::<PackageId>(),
            Err(PackageIdError::UnsupportedKind("hub".to_owned()))
        );
        assert!("android/org fdroid".parse::<PackageId>().is_err());
    }

    #[test]
    fn repository_priority_higher_number_wins() {
        assert!(RepositoryPriority::LOCAL > RepositoryPriority::DEFAULT);
        assert!(RepositoryPriority::DEFAULT > RepositoryPriority::LOCAL_AUTOGEN);
        assert_eq!(RepositoryPriority::LOCAL.value(), 100);
        assert_eq!(RepositoryPriority::DEFAULT.value(), 0);
        assert_eq!(RepositoryPriority::LOCAL_AUTOGEN.value(), -1);
    }

    #[test]
    fn repository_id_accepts_named_repositories() {
        assert_eq!(
            RepositoryId::new("local_autogen").unwrap().as_str(),
            "local_autogen"
        );
        assert_eq!(
            RepositoryId::new("community.1").unwrap().to_string(),
            "community.1"
        );
        assert!(RepositoryId::new("bad/repo").is_err());
    }
}
