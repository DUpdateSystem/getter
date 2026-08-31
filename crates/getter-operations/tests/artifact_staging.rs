#![cfg(feature = "lua")]

use getter_core::repository::RepositoryMetadata;
use getter_core::{RepositoryId, RepositoryPriority};
use getter_operations::app::download_app_with_transports;
use getter_operations::download::{
    RuntimeDownloadSink, RuntimeDownloadTransport, RuntimeDownloadTransportError,
    RuntimeDownloadTransportRequest,
};
use getter_operations::github_releases::{
    GithubReleaseTransport, GithubReleaseTransportError, GithubReleaseTransportRequest,
    GithubReleaseTransportResponse,
};
use getter_storage::{MainDb, StoredPackageResolution, TrackedPackageUpsert};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::rc::Rc;

struct BytesTransport {
    bodies: HashMap<String, Vec<u8>>,
    requests: RefCell<Vec<String>>,
}

impl RuntimeDownloadTransport for BytesTransport {
    fn fetch(
        &self,
        request: &RuntimeDownloadTransportRequest,
        sink: &mut dyn RuntimeDownloadSink,
    ) -> Result<(), RuntimeDownloadTransportError> {
        self.requests.borrow_mut().push(request.url.to_owned());
        let body = self.bodies.get(request.url).unwrap();
        sink.set_total_bytes(Some(body.len() as u64))
            .map_err(RuntimeDownloadTransportError::Sink)?;
        sink.write_chunk(body)
            .map_err(RuntimeDownloadTransportError::Sink)
    }
}

struct GithubMock {
    body: Vec<u8>,
    requests: RefCell<usize>,
}

impl GithubReleaseTransport for GithubMock {
    fn fetch_releases(
        &self,
        _request: &GithubReleaseTransportRequest<'_>,
    ) -> Result<GithubReleaseTransportResponse, GithubReleaseTransportError> {
        *self.requests.borrow_mut() += 1;
        Ok(GithubReleaseTransportResponse {
            body: String::from_utf8(self.body.clone()).unwrap(),
            etag: None,
            last_modified: None,
        })
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn fixture(script: &str, manifest: &str, pin: Option<&str>) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let package = temp.path().join("repo/official/android/com.example");
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("Manifest"), manifest).unwrap();
    fs::write(
        package.join("metadata.jsonc"),
        r#"{"type":"android:app","display_name":"Example","android":{"package_name":"com.example"}}"#,
    )
    .unwrap();
    fs::write(package.join("1.lua"), script).unwrap();
    let db = MainDb::open(temp.path().join("main.db")).unwrap();
    db.upsert_repository(
        &RepositoryMetadata {
            id: RepositoryId::new("official").unwrap(),
            name: "Official".into(),
            priority: RepositoryPriority::new(0),
            api_version: "1".into(),
        },
        Some(&temp.path().join("repo/official")),
        None,
    )
    .unwrap();
    db.upsert_tracked_package(&TrackedPackageUpsert {
        package_id: "android/com.example".parse().unwrap(),
        enabled: true,
        favorite: false,
        repository_id: None,
        package_resolution: StoredPackageResolution::OfficialRepositoryPackage,
        pin_version: pin.map(str::to_owned),
    })
    .unwrap();
    temp
}

#[test]
fn stages_multiple_artifacts_from_manifest_in_declaration_order_and_ignores_baselines() {
    let first = b"apk bytes";
    let second = b"symbols bytes";
    let script = r#"#!/bin/upa-lua v1
return package_version { updates = {
  { version="2", artifacts={{name="old",url="mock://old",file_name="old.apk"}} },
  { version="10", artifacts={
    {name="apk",url="mock://apk",file_name="release/path/Example APK.apk"},
    {name="symbols",url="mock://symbols",file_name="symbols.zip"}
  } }
} }"#;
    let manifest = format!(
        "{} release/path/Example APK.apk\n{} symbols.zip\n",
        digest(first),
        digest(second)
    );
    let temp = fixture(script, &manifest, Some("999"));
    let transport = Rc::new(BytesTransport {
        bodies: HashMap::from([
            ("mock://apk".into(), first.to_vec()),
            ("mock://symbols".into(), second.to_vec()),
        ]),
        requests: RefCell::new(Vec::new()),
    });

    let result = download_app_with_transports(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        transport.clone(),
    )
    .unwrap();

    assert_eq!(result.version, "10");
    assert_eq!(
        result
            .artifacts
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["apk", "symbols"]
    );
    assert!(result.artifacts[0]
        .path
        .ends_with(format!("{}-release_path_Example APK.apk", digest(first))));
    assert_eq!(
        &*transport.requests.borrow(),
        &["mock://apk", "mock://symbols"]
    );
    assert_eq!(fs::read(&result.artifacts[0].path).unwrap(), first);
}

#[test]
fn invalid_manifest_preflight_prevents_every_artifact_request() {
    for manifest in [
        "",
        "not-a-digest app.apk\n",
        &format!("{} app.apk\n{} app.apk\n", "a".repeat(64), "a".repeat(64)),
    ] {
        let script = r#"#!/bin/upa-lua v1
return package_version { updates = {{ version="2", artifacts={{name="app",url="mock://app",file_name="app.apk"}} }} }"#;
        let temp = fixture(script, manifest, None);
        let transport = Rc::new(BytesTransport {
            bodies: HashMap::new(),
            requests: RefCell::new(Vec::new()),
        });

        let error = download_app_with_transports(
            temp.path(),
            &"android/com.example".parse().unwrap(),
            None,
            transport.clone(),
        )
        .unwrap_err();
        assert!(
            error.code().starts_with("artifact.manifest_")
                || error.to_string().contains("package Manifest is invalid")
                || format!("{error:?}").contains("InvalidManifest"),
            "unexpected error: {error:?}"
        );
        assert!(transport.requests.borrow().is_empty());
        assert!(!temp.path().join("downloads").exists());
    }
}

#[test]
fn manifest_digest_hint_selects_one_of_multiple_filename_members() {
    let bytes = b"selected release bytes";
    let selected = digest(bytes);
    let script = format!(
        r#"#!/bin/upa-lua v1
return package_version {{ updates = {{ {{ version="2", artifacts={{{{name="app",url="mock://app",file_name="app.apk",sha256="{selected}"}}}} }} }} }}"#
    );
    let manifest = format!("{} app.apk\n{selected} app.apk\n", "a".repeat(64));
    let temp = fixture(&script, &manifest, None);
    let transport = Rc::new(BytesTransport {
        bodies: HashMap::from([("mock://app".into(), bytes.to_vec())]),
        requests: RefCell::new(Vec::new()),
    });

    let result = download_app_with_transports(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        transport,
    )
    .unwrap();

    assert_eq!(result.artifacts[0].sha256, selected);
}

#[test]
fn multiple_filename_members_without_digest_hint_are_ambiguous() {
    let script = r#"#!/bin/upa-lua v1
return package_version { updates = {{ version="2", artifacts={{name="app",url="mock://app",file_name="app.apk"}} }} }"#;
    let manifest = format!("{} app.apk\n{} app.apk\n", "a".repeat(64), "b".repeat(64));
    let temp = fixture(script, &manifest, None);
    let transport = Rc::new(BytesTransport {
        bodies: HashMap::new(),
        requests: RefCell::new(Vec::new()),
    });

    let error = download_app_with_transports(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        transport.clone(),
    )
    .unwrap_err();

    assert_eq!(error.code(), "artifact.manifest_ambiguous");
    assert!(transport.requests.borrow().is_empty());
}

#[test]
fn final_and_temporary_namespace_cross_collision_fails_preflight() {
    let first_digest = digest(b"same bytes");
    let second_digest = first_digest.clone();
    let second_name = "app.apk.download";
    let script = format!(
        r#"#!/bin/upa-lua v1
return package_version {{ updates = {{ {{ version="2", artifacts={{
  {{name="first",url="mock://first",file_name="app.apk"}},
  {{name="second",url="mock://second",file_name="{second_name}"}}
}} }} }} }}"#
    );
    let manifest = format!("{first_digest} app.apk\n{second_digest} {second_name}\n");
    let temp = fixture(&script, &manifest, None);
    let transport = Rc::new(BytesTransport {
        bodies: HashMap::new(),
        requests: RefCell::new(Vec::new()),
    });

    let error = download_app_with_transports(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        transport.clone(),
    )
    .unwrap_err();

    assert_eq!(error.code(), "artifact.path_collision");
    assert!(transport.requests.borrow().is_empty());
}

#[test]
fn reuses_final_without_transport_and_mismatch_leaves_only_temporary_file() {
    let expected = digest(b"good");
    let script = r#"#!/bin/upa-lua v1
return package_version { updates = {{ version="2", artifacts={{name="app",url="mock://app",file_name="dir/app.apk"}} }} }"#;
    let manifest = format!("{expected} dir/app.apk\n");
    let temp = fixture(script, &manifest, None);
    let final_path = temp
        .path()
        .join(format!("downloads/{expected}-dir_app.apk"));
    fs::create_dir_all(final_path.parent().unwrap()).unwrap();
    fs::write(&final_path, b"owned marker").unwrap();
    let no_call = Rc::new(BytesTransport {
        bodies: HashMap::new(),
        requests: RefCell::new(Vec::new()),
    });
    let reused = download_app_with_transports(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        no_call.clone(),
    )
    .unwrap();
    assert_eq!(reused.artifacts[0].status, "reused");
    assert!(no_call.requests.borrow().is_empty());

    fs::remove_file(&final_path).unwrap();
    let temporary_path = std::path::PathBuf::from(format!("{}.download", final_path.display()));
    fs::write(&temporary_path, b"old partial").unwrap();
    let mismatch = Rc::new(BytesTransport {
        bodies: HashMap::from([("mock://app".into(), b"bad".to_vec())]),
        requests: RefCell::new(Vec::new()),
    });
    let error = download_app_with_transports(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        mismatch,
    )
    .unwrap_err();
    assert_eq!(error.code(), "artifact.sha256_mismatch");
    assert!(!final_path.exists());
    assert_eq!(fs::read(temporary_path).unwrap(), b"bad");
}

#[test]
fn distinct_artifact_filenames_that_flatten_to_one_staging_path_fail_preflight() {
    let bytes = b"same";
    let sha256 = digest(bytes);
    let temp = fixture(
        r#"#!/bin/upa-lua v1
return package_version { updates = {{ version="2", artifacts={
          {name="slash",url="https://example.invalid/slash",file_name="dir/app.apk"},
          {name="flat",url="https://example.invalid/flat",file_name="dir_app.apk"},
        } }} }"#,
        &format!("{sha256} dir/app.apk\n{sha256} dir_app.apk\n"),
        None,
    );
    let transport = Rc::new(BytesTransport {
        bodies: HashMap::from([
            ("https://example.invalid/slash".to_owned(), bytes.to_vec()),
            ("https://example.invalid/flat".to_owned(), bytes.to_vec()),
        ]),
        requests: RefCell::new(Vec::new()),
    });

    let error = download_app_with_transports(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        transport.clone(),
    )
    .unwrap_err();

    assert_eq!(error.code(), "artifact.path_collision", "{error:?}");
    assert!(transport.requests.borrow().is_empty());
    assert!(!temp.path().join("downloads").exists());
}

#[test]
fn cold_github_refresh_uses_provider_digest_via_manifest() {
    let bytes = b"github apk";
    let sha = digest(bytes);
    let script = r#"#!/bin/upa-lua v1
local github_android = require("luaclass.github_android_apk")
return github_android.package {
  name="Example", android_package="com.example", owner="example", repo="app"
}"#;
    let releases = format!(
        r#"[{{"tag_name":"v2","name":"v2","body":"","draft":false,"prerelease":false,"assets":[{{"id":1,"name":"app.apk","content_type":"application/vnd.android.package-archive","size":10,"browser_download_url":"mock://github-apk","digest":"sha256:{sha}"}}]}}]"#
    );
    let source_sha512 = format!("{:x}", sha2::Sha512::digest(releases.as_bytes()));
    let temp = fixture(
        script,
        &format!("{source_sha512} github-releases.json\n{sha} app.apk\n"),
        None,
    );
    let github = Rc::new(GithubMock {
        body: releases.into_bytes(),
        requests: RefCell::new(0),
    });
    let transport = Rc::new(BytesTransport {
        bodies: HashMap::from([("mock://github-apk".into(), bytes.to_vec())]),
        requests: RefCell::new(Vec::new()),
    });

    let result = download_app_with_transports(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        Some(github.clone()),
        transport,
    )
    .unwrap();
    assert_eq!(result.version, "v2");
    assert_eq!(*github.requests.borrow(), 1);
    assert_eq!(fs::read(&result.artifacts[0].path).unwrap(), bytes);
}
