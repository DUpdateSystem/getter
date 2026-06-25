//! Provider executor scaffolding for the UpgradeAll getter rewrite.
//!
//! Real network/provider execution is intentionally deferred to a later ADR.
//! The first Phase D bridge uses package-declared static update candidates as a
//! mock provider so the rest of the runtime can exercise getter-owned update
//! selection and opaque action issuance without direct network side effects.

pub use getter_core as core;

use getter_core::{ResolvedPackage, UpdateCandidate};

/// Mock provider that returns the static `updates` candidates materialized from
/// a resolved Lua package table.
///
/// This is development scaffolding, not the final live provider model. Keeping
/// it behind a provider-shaped boundary prevents operation code from treating
/// `package.updates` as the product update-check architecture.
#[derive(Debug, Default, Clone, Copy)]
pub struct StaticPackageUpdatesProvider;

impl StaticPackageUpdatesProvider {
    pub fn check_updates(self, package: &ResolvedPackage) -> Vec<UpdateCandidate> {
        package.updates.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use getter_core::PackagePermissions;

    #[test]
    fn static_provider_returns_package_declared_update_candidates() {
        let package = ResolvedPackage {
            id: "android/org.fdroid.fdroid".parse().unwrap(),
            name: "F-Droid".to_owned(),
            repository: "official".parse().unwrap(),
            installed: Vec::new(),
            permissions: PackagePermissions::default(),
            source_priority: Vec::new(),
            updates: vec![UpdateCandidate {
                version: "1.2.0".to_owned(),
                channel: Some("stable".to_owned()),
                source: Some("fixture".to_owned()),
                artifacts: Vec::new(),
            }],
        };

        let candidates = StaticPackageUpdatesProvider.check_updates(&package);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].version, "1.2.0");
        assert_eq!(candidates[0].source.as_deref(), Some("fixture"));
    }
}
