//! Structured repository/package diagnostics for offline validation.
//!
//! Diagnostics are getter-owned DTOs used by CLI and future app bridges. They
//! describe what getter observed; Flutter should only render them.

use crate::lua::{evaluate_package_directory_script, evaluate_package_file, LuaPackageError};
use crate::repository::{
    package_cache_key, InvalidPackageDirectory, RepositoryLayout, RepositoryLoadError,
    RepositoryPackageDirectoryLayout,
};
use crate::PackageId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLocation {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageValidationDiagnostic {
    pub severity: DiagnosticSeverity,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_id: Option<PackageId>,
    pub location: SourceLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryValidationReport {
    pub valid: bool,
    pub diagnostics: Vec<PackageValidationDiagnostic>,
    pub package_count: usize,
    pub network_required: bool,
}

impl RepositoryValidationReport {
    pub fn new(package_count: usize, diagnostics: Vec<PackageValidationDiagnostic>) -> Self {
        let valid = !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error);
        Self {
            valid,
            diagnostics,
            package_count,
            network_required: false,
        }
    }
}

/// Validate a repository path offline.
///
/// This intentionally loads and evaluates local Lua package files only. It must
/// not perform provider/network checks.
pub fn validate_repository_path(path: impl AsRef<Path>) -> RepositoryValidationReport {
    let root = path.as_ref();
    if root.join("repo.toml").is_file() {
        validate_legacy_repository_path(root)
    } else {
        validate_package_directory_repository_path(root)
    }
}

fn validate_legacy_repository_path(root: &Path) -> RepositoryValidationReport {
    let layout = match RepositoryLayout::load(root) {
        Ok(layout) => layout,
        Err(error) => {
            return RepositoryValidationReport::new(0, vec![repository_load_diagnostic(error)])
        }
    };

    let mut diagnostics = Vec::new();
    let mut package_count = 0usize;
    for package_file in &layout.packages {
        if let Err(error) = package_cache_key(&layout, package_file) {
            diagnostics.push(repository_load_diagnostic(error));
            continue;
        }
        match evaluate_package_file(&layout, &package_file.path) {
            Ok(_) => package_count += 1,
            Err(error) => diagnostics.push(lua_diagnostic(error, Some(package_file.id.clone()))),
        }
    }

    RepositoryValidationReport::new(package_count, diagnostics)
}

fn validate_package_directory_repository_path(root: &Path) -> RepositoryValidationReport {
    let layout = match RepositoryPackageDirectoryLayout::load(root) {
        Ok(layout) => layout,
        Err(error) => {
            return RepositoryValidationReport::new(0, vec![repository_load_diagnostic(error)])
        }
    };
    let mut diagnostics: Vec<_> = layout
        .invalid_packages
        .iter()
        .map(invalid_package_directory_diagnostic)
        .collect();
    let mut package_count = 0usize;
    let repository_id = "validation".parse().expect("static repository id is valid");
    for package in &layout.packages {
        let metadata = match layout.package_metadata(package) {
            Ok(metadata) => metadata,
            Err(error) => {
                diagnostics.push(repository_load_diagnostic(error));
                continue;
            }
        };
        if package.version_scripts.is_empty() {
            diagnostics.push(repository_load_diagnostic(
                RepositoryLoadError::MissingPackageVersionScript {
                    package_id: package.id.clone(),
                },
            ));
            continue;
        }
        let diagnostic_count_before = diagnostics.len();
        for script in &package.version_scripts {
            if let Err(error) =
                evaluate_package_directory_script(&repository_id, package, &metadata, script)
            {
                diagnostics.push(lua_diagnostic(error, Some(package.id.clone())));
            }
        }
        if diagnostics.len() == diagnostic_count_before {
            package_count += 1;
        }
    }
    RepositoryValidationReport::new(package_count, diagnostics)
}

fn repository_load_diagnostic(error: RepositoryLoadError) -> PackageValidationDiagnostic {
    let (code, path, message) = match error {
        RepositoryLoadError::ReadRootMetadata { path, source } => (
            "repository.read_root_metadata",
            path,
            format!("failed to read repository root metadata: {source}"),
        ),
        RepositoryLoadError::ParseRootMetadata { path, source } => (
            "repository.parse_root_metadata",
            path,
            format!("failed to parse repository root metadata: {source}"),
        ),
        RepositoryLoadError::UnsupportedRootMetadataVersion {
            path,
            found,
            expected,
        } => (
            "repository.unsupported_root_metadata_version",
            path,
            format!("unsupported repository root metadata version {found}; expected {expected}"),
        ),
        RepositoryLoadError::ReadRepositoryRoot { path, source } => (
            "repository.read_root",
            path,
            format!("failed to read repository root: {source}"),
        ),
        RepositoryLoadError::MissingGeneratedRepository { alias, path } => (
            "repository.missing_generated_repository",
            path,
            format!("configured generated repository '{alias}' does not exist"),
        ),
        RepositoryLoadError::ReadRepoToml { path, source } => (
            "repository.read_repo_toml",
            path,
            format!("failed to read repo.toml: {source}"),
        ),
        RepositoryLoadError::ParseRepoToml { path, source } => (
            "repository.parse_repo_toml",
            path,
            format!("failed to parse repo.toml: {source}"),
        ),
        RepositoryLoadError::RepositoryId(source) => (
            "repository.invalid_id",
            PathBuf::from("repo.toml"),
            source.to_string(),
        ),
        RepositoryLoadError::UnsupportedApiVersion(version) => (
            "repository.unsupported_api_version",
            PathBuf::from("repo.toml"),
            format!("unsupported repository api_version '{version}'"),
        ),
        RepositoryLoadError::MissingDirectory { path, directory } => (
            "repository.missing_directory",
            path,
            format!("missing required directory '{directory}'"),
        ),
        RepositoryLoadError::ReadPackagesDir { path, source } => (
            "repository.read_packages_dir",
            path,
            format!("failed to read packages directory: {source}"),
        ),
        RepositoryLoadError::InvalidPackagePath { path, reason } => {
            ("repository.invalid_package_path", path, reason)
        }
        RepositoryLoadError::PackageId { path, source } => (
            "repository.invalid_package_id",
            path,
            format!("invalid package id derived from path: {source}"),
        ),
        RepositoryLoadError::HashPackageFile { path, source } => (
            "repository.hash_package_file",
            path,
            format!("failed to hash package file: {source}"),
        ),
        RepositoryLoadError::ReadPackageMetadata { path, source } => (
            "package.read_metadata",
            path,
            format!("failed to read package metadata: {source}"),
        ),
        RepositoryLoadError::ParsePackageMetadata { path, source } => (
            "package.parse_metadata",
            path,
            format!("failed to parse package metadata: {source}"),
        ),
        RepositoryLoadError::InvalidPackageMetadata { path, reason } => (
            "package.metadata",
            path,
            reason,
        ),
        RepositoryLoadError::MissingPackageVersionScript { package_id } => (
            "package.missing_version_script",
            PathBuf::from(format!("{package_id}/metadata.jsonc")),
            format!("package '{package_id}' has no enabled version scripts"),
        ),
        RepositoryLoadError::AmbiguousPackageVersionScript { package_id } => (
            "package.ambiguous_version_script",
            PathBuf::from(format!("{package_id}/metadata.jsonc")),
            format!(
                "package '{package_id}' has multiple enabled version scripts; explicit version selection is not implemented"
            ),
        ),
        RepositoryLoadError::MissingLuaApiShebang { path, expected } => (
            "package.lua_api_shebang",
            path,
            format!("Lua version script is missing required shebang '{expected}'"),
        ),
    };
    diagnostic(code, message, path, None, None)
}

fn invalid_package_directory_diagnostic(
    package: &InvalidPackageDirectory,
) -> PackageValidationDiagnostic {
    diagnostic(
        "package.metadata",
        package.reason.clone(),
        package.metadata_path.clone(),
        None,
        package.id.clone(),
    )
}

fn lua_diagnostic(
    error: LuaPackageError,
    fallback_package_id: Option<PackageId>,
) -> PackageValidationDiagnostic {
    match error {
        LuaPackageError::ReadFile { path, source } => diagnostic(
            "package.read_file",
            format!("failed to read Lua package file: {source}"),
            path,
            None,
            fallback_package_id,
        ),
        LuaPackageError::Runtime { path, source } => diagnostic(
            "package.lua_runtime",
            format!("Lua runtime error: {source}"),
            path,
            None,
            fallback_package_id,
        ),
        LuaPackageError::NotATable { path } => diagnostic(
            "package.not_a_table",
            "Lua package did not return a table".to_owned(),
            path,
            None,
            fallback_package_id,
        ),
        LuaPackageError::UnsupportedValue {
            path,
            location,
            value_type,
        } => diagnostic(
            "package.unsupported_value",
            format!("unsupported Lua value at {location}: {value_type}"),
            path,
            Some(location),
            fallback_package_id,
        ),
        LuaPackageError::Schema { path, message } => {
            let field = schema_field_from_message(&message);
            diagnostic("package.schema", message, path, field, fallback_package_id)
        }
        LuaPackageError::Domain { path, message } => {
            diagnostic("package.domain", message, path, None, fallback_package_id)
        }
        LuaPackageError::Repository { path, source } => diagnostic(
            "package.repository",
            source.to_string(),
            path,
            None,
            fallback_package_id,
        ),
    }
}

fn schema_field_from_message(message: &str) -> Option<String> {
    let marker = "field '";
    let start = message.find(marker)? + marker.len();
    let end = message[start..].find('\'')?;
    Some(message[start..start + end].to_owned())
}

fn diagnostic(
    code: impl Into<String>,
    message: impl Into<String>,
    path: PathBuf,
    field: Option<String>,
    package_id: Option<PackageId>,
) -> PackageValidationDiagnostic {
    PackageValidationDiagnostic {
        severity: DiagnosticSeverity::Error,
        code: code.into(),
        message: message.into(),
        package_id,
        location: SourceLocation {
            path: path.to_string_lossy().to_string(),
            field,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture_repo() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(
            root.join("repo.toml"),
            r#"id = "official"
name = "Official"
priority = 0
api_version = "getter.repo.v1"
"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("packages/android")).unwrap();
        fs::create_dir(root.join("lib")).unwrap();
        fs::create_dir(root.join("templates")).unwrap();
        temp
    }

    #[test]
    fn valid_repository_has_no_diagnostics() {
        let temp = fixture_repo();
        fs::write(
            temp.path().join("packages/android/org.fdroid.fdroid.lua"),
            r#"return package_def { id = "android/org.fdroid.fdroid", name = "F-Droid" }"#,
        )
        .unwrap();

        let report = validate_repository_path(temp.path());
        assert!(report.valid, "{report:?}");
        assert_eq!(report.package_count, 1);
        assert!(report.diagnostics.is_empty());
        assert!(!report.network_required);
    }

    #[test]
    fn package_directory_repository_has_no_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/app/org.fdroid.fdroid");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{ "type": "android:app", "android": { "package_name": "org.fdroid.fdroid" } }"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("1.20.0.lua"),
            "#!/bin/upa-lua v1\nreturn package_version { installed = { { kind = \"android_package\", package_name = \"org.fdroid.fdroid\" } } }",
        )
        .unwrap();

        let report = validate_repository_path(temp.path());

        assert!(report.valid, "{report:?}");
        assert_eq!(report.package_count, 1);
        assert!(report.diagnostics.is_empty());
        assert!(!report.network_required);
    }

    #[test]
    fn package_directory_without_shebang_is_reported_as_invalid() {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/app/org.fdroid.fdroid");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{ "type": "android:app", "android": { "package_name": "org.fdroid.fdroid" } }"#,
        )
        .unwrap();
        fs::write(package_dir.join("1.20.0.lua"), "return package_version {}").unwrap();

        let report = validate_repository_path(temp.path());

        assert!(!report.valid);
        assert_eq!(report.diagnostics[0].code, "package.repository");
        assert!(report.diagnostics[0].message.contains("#!/bin/upa-lua v1"));
    }

    #[test]
    fn package_directory_validator_checks_each_version_script_without_selecting_one() {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/app/org.fdroid.fdroid");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{ "type": "android:app", "android": { "package_name": "org.fdroid.fdroid" } }"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("1.20.0.lua"),
            "#!/bin/upa-lua v1\nreturn package_version { installed = { { kind = \"android_package\", package_name = \"org.fdroid.fdroid\" } } }",
        )
        .unwrap();
        fs::write(
            package_dir.join("9999.lua"),
            "#!/bin/upa-lua v1\nreturn package_version { installed = { { kind = \"android_package\", package_name = \"org.fdroid.fdroid\" } } }",
        )
        .unwrap();

        let report = validate_repository_path(temp.path());

        assert!(report.valid, "{report:?}");
        assert_eq!(report.package_count, 1);
    }

    #[test]
    fn package_directory_metadata_error_is_stable_package_diagnostic() {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/app/org.fdroid.fdroid");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(package_dir.join("metadata.jsonc"), "[]").unwrap();

        let report = validate_repository_path(temp.path());

        assert!(!report.valid);
        assert_eq!(report.package_count, 0);
        assert_eq!(report.diagnostics[0].code, "package.metadata");
        assert_eq!(
            report.diagnostics[0]
                .package_id
                .as_ref()
                .unwrap()
                .to_string(),
            "android/app/org.fdroid.fdroid"
        );
    }

    #[test]
    fn missing_directory_is_stable_repository_diagnostic() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("repo.toml"),
            r#"id = "official"
name = "Official"
api_version = "getter.repo.v1"
"#,
        )
        .unwrap();

        let report = validate_repository_path(temp.path());
        assert!(!report.valid);
        assert_eq!(report.diagnostics[0].code, "repository.missing_directory");
    }

    #[test]
    fn lua_schema_error_is_stable_package_diagnostic() {
        let temp = fixture_repo();
        fs::write(
            temp.path().join("packages/android/org.fdroid.fdroid.lua"),
            r#"return { id = "android/org.fdroid.fdroid" }"#,
        )
        .unwrap();

        let report = validate_repository_path(temp.path());
        assert!(!report.valid);
        assert_eq!(report.diagnostics[0].code, "package.schema");
        assert_eq!(
            report.diagnostics[0].location.field.as_deref(),
            Some("name")
        );
        assert_eq!(
            report.diagnostics[0]
                .package_id
                .as_ref()
                .unwrap()
                .to_string(),
            "android/org.fdroid.fdroid"
        );
    }

    #[test]
    fn package_id_path_mismatch_is_domain_diagnostic() {
        let temp = fixture_repo();
        fs::write(
            temp.path().join("packages/android/org.fdroid.fdroid.lua"),
            r#"return { id = "android/com.termux", name = "Termux" }"#,
        )
        .unwrap();

        let report = validate_repository_path(temp.path());
        assert!(!report.valid);
        assert_eq!(report.diagnostics[0].code, "package.domain");
    }

    #[test]
    fn unsupported_api_version_is_stable_repository_diagnostic() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("repo.toml"),
            r#"id = "official"
name = "Official"
api_version = "getter.repo.v2"
"#,
        )
        .unwrap();

        let report = validate_repository_path(temp.path());
        assert!(!report.valid);
        assert_eq!(
            report.diagnostics[0].code,
            "repository.unsupported_api_version"
        );
    }
}
