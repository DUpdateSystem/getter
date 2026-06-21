//! Legacy Room migration mapping tests and pure mapping helpers.
//!
//! This module intentionally contains no Android Room/database reader. It is the
//! TDD boundary for source->target mapping rules before the full migration
//! implementation is added.

use getter_core::PackageId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyAppKind {
    Android,
    Magisk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyAppRecord {
    pub kind: LegacyAppKind,
    pub installed_id: String,
    pub official_package_available: bool,
    pub common_conversion_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyExtraAppRecord {
    pub ignored_version: Option<String>,
    pub favorite: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyAppMapping {
    pub package_id: PackageId,
    pub package_resolution: LegacyPackageResolution,
    pub user_state: LegacyUserStateMapping,
    pub warnings: Vec<LegacyMigrationWarning>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyPackageResolution {
    OfficialRepositoryPackage,
    GenerateLocalPackage,
    MissingPackageDefinition,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyUserStateMapping {
    pub ignored_version: Option<String>,
    pub favorite: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyMigrationWarning {
    MissingPackageDefinition { package_id: PackageId },
}

#[derive(Debug, thiserror::Error)]
pub enum LegacyRoomMappingError {
    #[error("legacy installed id is empty")]
    EmptyInstalledId,
    #[error("failed to construct target package id: {0}")]
    PackageId(#[from] getter_core::PackageIdError),
}

pub fn map_legacy_app(
    app: &LegacyAppRecord,
    extra: Option<&LegacyExtraAppRecord>,
) -> Result<LegacyAppMapping, LegacyRoomMappingError> {
    let package_id = map_legacy_package_id(app.kind, &app.installed_id)?;
    let package_resolution = if app.official_package_available {
        LegacyPackageResolution::OfficialRepositoryPackage
    } else if app.common_conversion_available {
        LegacyPackageResolution::GenerateLocalPackage
    } else {
        LegacyPackageResolution::MissingPackageDefinition
    };
    let warnings = match package_resolution {
        LegacyPackageResolution::MissingPackageDefinition => {
            vec![LegacyMigrationWarning::MissingPackageDefinition {
                package_id: package_id.clone(),
            }]
        }
        _ => Vec::new(),
    };
    let user_state = LegacyUserStateMapping {
        ignored_version: extra.and_then(|extra| extra.ignored_version.clone()),
        favorite: extra.is_some_and(|extra| extra.favorite),
    };

    Ok(LegacyAppMapping {
        package_id,
        package_resolution,
        user_state,
        warnings,
    })
}

pub fn map_legacy_package_id(
    kind: LegacyAppKind,
    installed_id: &str,
) -> Result<PackageId, LegacyRoomMappingError> {
    if installed_id.is_empty() {
        return Err(LegacyRoomMappingError::EmptyInstalledId);
    }
    let prefix = match kind {
        LegacyAppKind::Android => "android",
        LegacyAppKind::Magisk => "magisk",
    };
    Ok(format!("{prefix}/{installed_id}").parse()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_legacy_app_maps_to_readable_android_package_id() {
        let mapping = map_legacy_app(
            &LegacyAppRecord {
                kind: LegacyAppKind::Android,
                installed_id: "org.fdroid.fdroid".to_owned(),
                official_package_available: true,
                common_conversion_available: false,
            },
            None,
        )
        .unwrap();

        assert_eq!(mapping.package_id.to_string(), "android/org.fdroid.fdroid");
        assert_eq!(
            mapping.package_resolution,
            LegacyPackageResolution::OfficialRepositoryPackage
        );
        assert!(mapping.warnings.is_empty());
    }

    #[test]
    fn magisk_legacy_app_maps_to_readable_magisk_package_id() {
        let mapping = map_legacy_app(
            &LegacyAppRecord {
                kind: LegacyAppKind::Magisk,
                installed_id: "zygisk-next".to_owned(),
                official_package_available: true,
                common_conversion_available: false,
            },
            None,
        )
        .unwrap();

        assert_eq!(mapping.package_id.to_string(), "magisk/zygisk-next");
    }

    #[test]
    fn common_unofficial_conversion_generates_local_package() {
        let mapping = map_legacy_app(
            &LegacyAppRecord {
                kind: LegacyAppKind::Android,
                installed_id: "com.example.private".to_owned(),
                official_package_available: false,
                common_conversion_available: true,
            },
            None,
        )
        .unwrap();

        assert_eq!(
            mapping.package_resolution,
            LegacyPackageResolution::GenerateLocalPackage
        );
        assert!(mapping.warnings.is_empty());
    }

    #[test]
    fn unmapped_complex_app_preserves_id_and_records_missing_definition_warning() {
        let mapping = map_legacy_app(
            &LegacyAppRecord {
                kind: LegacyAppKind::Android,
                installed_id: "com.example.unmapped".to_owned(),
                official_package_available: false,
                common_conversion_available: false,
            },
            None,
        )
        .unwrap();

        assert_eq!(
            mapping.package_resolution,
            LegacyPackageResolution::MissingPackageDefinition
        );
        assert_eq!(
            mapping.warnings,
            vec![LegacyMigrationWarning::MissingPackageDefinition {
                package_id: "android/com.example.unmapped".parse().unwrap(),
            }]
        );
    }

    #[test]
    fn extra_app_ignore_and_favorite_state_are_preserved_when_present() {
        let mapping = map_legacy_app(
            &LegacyAppRecord {
                kind: LegacyAppKind::Android,
                installed_id: "org.fdroid.fdroid".to_owned(),
                official_package_available: true,
                common_conversion_available: false,
            },
            Some(&LegacyExtraAppRecord {
                ignored_version: Some("1.2.3".to_owned()),
                favorite: true,
            }),
        )
        .unwrap();

        assert_eq!(mapping.user_state.ignored_version.as_deref(), Some("1.2.3"));
        assert!(mapping.user_state.favorite);
    }
}
