use getter_core::manifest::{ManifestError, PackageManifest};

#[test]
fn parses_mixed_source_sha512_and_artifact_sha256_entries() {
    let source = "a".repeat(128);
    let artifact = "B".repeat(64);
    let manifest = PackageManifest::parse(&format!(
        "{source} github-releases.json\n{artifact} release/path/App.apk\n"
    ))
    .unwrap();

    assert_eq!(
        manifest.artifact_sha256("release/path/App.apk").unwrap(),
        "b".repeat(64)
    );
    assert_eq!(
        manifest.source_sha512("github-releases.json").unwrap(),
        source
    );
}

#[test]
fn duplicate_artifact_filename_is_rejected_even_when_digest_matches() {
    let digest = "a".repeat(64);
    let error =
        PackageManifest::parse(&format!("{digest} app.apk\n{digest} app.apk\n")).unwrap_err();

    assert!(matches!(error, ManifestError::Duplicate { .. }));
}

#[test]
fn sha256_and_multiple_sha512_memberships_can_share_one_filename() {
    let artifact = "a".repeat(64);
    let first_source = "b".repeat(128);
    let second_source = "c".repeat(128);
    let manifest = PackageManifest::parse(&format!(
        "{artifact} app.apk\n{first_source} app.apk\n{second_source} app.apk\n"
    ))
    .unwrap();

    assert_eq!(manifest.artifact_sha256("app.apk"), Some(artifact.as_str()));
    assert!(manifest.contains_sha512(&first_source));
    assert!(manifest.contains_sha512(&second_source));
}

#[test]
fn preserves_multiple_distinct_sha256_memberships_for_one_filename() {
    let first = "a".repeat(64);
    let second = "b".repeat(64);
    let manifest = PackageManifest::parse(&format!("{first} app.apk\n{second} app.apk\n")).unwrap();

    assert_eq!(
        manifest.artifact_sha256_members("app.apk"),
        &[first, second]
    );
}

#[test]
fn malformed_lines_are_rejected() {
    let error = PackageManifest::parse("not-a-digest app.apk\n").unwrap_err();
    assert!(matches!(error, ManifestError::Malformed { line: 1, .. }));
}
