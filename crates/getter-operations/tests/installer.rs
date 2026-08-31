#![cfg(feature = "lua")]

use getter_core::repository::RepositoryMetadata;
use getter_core::runtime::GetterRuntime;
use getter_core::{RepositoryId, RepositoryPriority};
use getter_operations::app::{
    download_app_with_transports, install_app_with_dependencies,
    install_app_with_dependencies_and_observer, prepare_platform_install_for_task,
    prepare_platform_install_with_transports, CommandObserver, CommandResolver, CommandRunner,
    PlatformInstallRequest, ResolvedInstallerCommand, RunnerOutput,
};
use getter_operations::download::{
    RuntimeDownloadSink, RuntimeDownloadTransport, RuntimeDownloadTransportError,
    RuntimeDownloadTransportRequest,
};
use getter_operations::runtime::{
    issue_action_from_registered_package_json_with_github_transport,
    submit_action_and_download_json_with_transport,
};
use getter_storage::{MainDb, StoredPackageResolution, TrackedPackageUpsert};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

struct Bytes(Vec<u8>);
impl RuntimeDownloadTransport for Bytes {
    fn fetch(
        &self,
        _: &RuntimeDownloadTransportRequest,
        sink: &mut dyn RuntimeDownloadSink,
    ) -> Result<(), RuntimeDownloadTransportError> {
        sink.write_chunk(&self.0)
            .map_err(RuntimeDownloadTransportError::Sink)
    }
}
struct Resolver(PathBuf);
impl CommandResolver for Resolver {
    fn resolve(&self, executable: &str) -> Option<PathBuf> {
        (executable == "fake-installer").then(|| self.0.clone())
    }
}
#[derive(Default)]
struct Runner {
    seen: RefCell<Option<ResolvedInstallerCommand>>,
}
impl CommandRunner for Runner {
    fn run(&self, command: &ResolvedInstallerCommand) -> std::io::Result<RunnerOutput> {
        *self.seen.borrow_mut() = Some(command.clone());
        Ok(RunnerOutput {
            status: 0,
            stdout: b"installed".to_vec(),
            stderr: Vec::new(),
        })
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn native_fixture(temp: &Path, name: &str, source_code: &str) -> PathBuf {
    let source = temp.join(format!("{name}.rs"));
    let executable = temp.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    fs::write(&source, source_code).unwrap();
    let status = Command::new("rustc")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .status()
        .expect("rustc must be available while running Rust tests");
    assert!(status.success(), "compile platform-native fixture {name}");
    executable
}

fn fake_installer(temp: &Path) -> PathBuf {
    native_fixture(
        temp,
        "fake-installer",
        r#"fn main() {
    for argument in std::env::args().skip(1) {
        println!("{argument}");
    }
}"#,
    )
}
fn fixture_with_metadata(install: &str, bytes: &[u8], metadata: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    getter_operations::startup::bootstrap_data_dir(temp.path()).unwrap();
    let package = temp.path().join("repo/official/android/com.example");
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("metadata.jsonc"), metadata).unwrap();
    fs::write(
        package.join("Manifest"),
        format!("{} app.apk\n", digest(bytes)),
    )
    .unwrap();
    fs::write(package.join("1.lua"), format!(r#"#!/bin/upa-lua v1
return package_version {{ updates = {{{{ version="2", artifacts={{{{name="app",file_name="app.apk",url="mock://app"}}}}, install={install} }}}} }}"#)).unwrap();
    let db = MainDb::open(temp.path().join("main.db")).unwrap();
    db.upsert_repository(
        &RepositoryMetadata {
            id: RepositoryId::new("official").unwrap(),
            name: "Official".into(),
            priority: RepositoryPriority::DEFAULT,
            api_version: "v1".into(),
        },
        Some(&temp.path().join("repo/official")),
        None,
    )
    .unwrap();
    db.upsert_tracked_package(&TrackedPackageUpsert {
        package_id: "android/com.example".parse().unwrap(),
        enabled: true,
        favorite: false,
        pin_version: None,
        repository_id: None,
        package_resolution: StoredPackageResolution::OfficialRepositoryPackage,
    })
    .unwrap();
    temp
}

fn fixture(install: &str, bytes: &[u8]) -> tempfile::TempDir {
    fixture_with_metadata(
        install,
        bytes,
        r#"{"type":"android:app","android":{"package_name":"com.example"}}"#,
    )
}

#[test]
fn task_scoped_prepare_uses_the_sealed_candidate_and_task_staging() {
    let bytes = b"apk";
    let temp = fixture(r#"{kind="android_apk",artifact={artifact="app"}}"#, bytes);
    let db = MainDb::open(temp.path().join("main.db")).unwrap();
    let mut runtime = GetterRuntime::new();
    let issued = issue_action_from_registered_package_json_with_github_transport(
        &mut runtime,
        temp.path(),
        &db,
        &serde_json::json!({
            "package_id": "android/com.example",
            "installed_version": "1"
        })
        .to_string(),
        None,
    )
    .unwrap();
    let action_id = issued["action"]["action_id"].as_str().unwrap();
    let task = submit_action_and_download_json_with_transport(
        &mut runtime,
        temp.path(),
        &serde_json::json!({"action_id": action_id}).to_string(),
        &Bytes(bytes.to_vec()),
    )
    .unwrap();
    assert_eq!(task["status"], "running");
    assert_eq!(task["phase"]["category"], "waiting_user");
    assert_eq!(task["phase"]["reason"], "install_handoff");
    let task_id = task["task_id"].as_str().unwrap();
    let task_path = PathBuf::from(task["downloaded_file"]["local_path"].as_str().unwrap());

    let package = temp.path().join("repo/official/android/com.example");
    fs::write(
        package.join("metadata.jsonc"),
        r#"{"type":"android:app","android":{"package_name":"com.changed"}}"#,
    )
    .unwrap();
    fs::write(
        package.join("Manifest"),
        format!("{} app.apk\n", digest(b"changed")),
    )
    .unwrap();
    fs::write(
        package.join("1.lua"),
        r#"#!/bin/upa-lua v1
return package_version { updates = {{ version="99", artifacts={{name="app",file_name="app.apk",url="mock://changed"}}, install={kind="android_apk",artifact={artifact="app"}} }} }"#,
    )
    .unwrap();

    let handoff = prepare_platform_install_for_task(&runtime, temp.path(), task_id).unwrap();

    assert_eq!(handoff.task_id.as_deref(), Some(task_id));
    assert_eq!(serde_json::to_value(&handoff).unwrap()["task_id"], task_id);
    assert_eq!(handoff.package_id.to_string(), "android/com.example");
    assert_eq!(handoff.repository_id, "official");
    assert_eq!(handoff.package_version, "2");
    let PlatformInstallRequest::AndroidApk { target, artifact } = handoff.request;
    assert_eq!(target.package_name, "com.example");
    assert_eq!(artifact.name, "app");
    assert_eq!(artifact.path, task_path);
    assert_eq!(artifact.sha256, digest(bytes));
}

#[test]
fn task_scoped_prepare_rejects_staged_bytes_changed_after_getter_download() {
    let bytes = b"apk";
    let temp = fixture(r#"{kind="android_apk",artifact={artifact="app"}}"#, bytes);
    let db = MainDb::open(temp.path().join("main.db")).unwrap();
    let mut runtime = GetterRuntime::new();
    let issued = issue_action_from_registered_package_json_with_github_transport(
        &mut runtime,
        temp.path(),
        &db,
        &serde_json::json!({
            "package_id": "android/com.example",
            "installed_version": "1"
        })
        .to_string(),
        None,
    )
    .unwrap();
    let action_id = issued["action"]["action_id"].as_str().unwrap();
    let task = submit_action_and_download_json_with_transport(
        &mut runtime,
        temp.path(),
        &serde_json::json!({"action_id": action_id}).to_string(),
        &Bytes(bytes.to_vec()),
    )
    .unwrap();
    let task_id = task["task_id"].as_str().unwrap();
    let task_path = task["downloaded_file"]["local_path"].as_str().unwrap();
    fs::write(task_path, b"bad").unwrap();

    let error = prepare_platform_install_for_task(&runtime, temp.path(), task_id).unwrap_err();

    assert_eq!(error.code(), "artifact.sha256_mismatch");
}

#[test]
fn task_scoped_prepare_reports_missing_sealed_android_plan() {
    let bytes = b"apk";
    let temp = fixture(r#"{executable="fake-installer",args={}}"#, bytes);
    let db = MainDb::open(temp.path().join("main.db")).unwrap();
    let mut runtime = GetterRuntime::new();
    let issued = issue_action_from_registered_package_json_with_github_transport(
        &mut runtime,
        temp.path(),
        &db,
        &serde_json::json!({
            "package_id": "android/com.example",
            "installed_version": "1"
        })
        .to_string(),
        None,
    )
    .unwrap();
    let action_id = issued["action"]["action_id"].as_str().unwrap();
    let task = submit_action_and_download_json_with_transport(
        &mut runtime,
        temp.path(),
        &serde_json::json!({"action_id": action_id}).to_string(),
        &Bytes(bytes.to_vec()),
    )
    .unwrap();
    let task_id = task["task_id"].as_str().unwrap();

    let error = prepare_platform_install_for_task(&runtime, temp.path(), task_id).unwrap_err();

    assert_eq!(error.code(), "installer.task_plan_missing");
}

#[test]
fn prepares_android_apk_handoff_from_verified_staging() {
    let bytes = b"apk";
    let temp = fixture(r#"{kind="android_apk",artifact={artifact="app"}}"#, bytes);

    let handoff = prepare_platform_install_with_transports(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        Rc::new(Bytes(bytes.to_vec())),
    )
    .unwrap();

    assert_eq!(handoff.format, "getter-platform-install-handoff");
    assert_eq!(handoff.version, 1);
    assert_eq!(handoff.task_id, None);
    assert_eq!(handoff.package_id.to_string(), "android/com.example");
    assert_eq!(handoff.repository_id.as_str(), "official");
    assert_eq!(handoff.package_version, "2");
    let artifact_path = match &handoff.request {
        PlatformInstallRequest::AndroidApk { artifact, .. } => artifact.path.clone(),
    };
    assert_eq!(
        serde_json::to_value(&handoff).unwrap(),
        serde_json::json!({
            "format": "getter-platform-install-handoff",
            "version": 1,
            "package_id": "android/com.example",
            "repository_id": "official",
            "package_version": "2",
            "request": {
                "kind": "android_apk",
                "target": {
                    "kind": "android",
                    "package_name": "com.example"
                },
                "artifact": {
                    "name": "app",
                    "path": artifact_path,
                    "sha256": digest(bytes),
                    "status": "downloaded"
                }
            }
        })
    );
    let PlatformInstallRequest::AndroidApk { target, artifact } = handoff.request;
    assert_eq!(target.kind, "android");
    assert_eq!(target.package_name, "com.example");
    assert_eq!(artifact.name, "app");
    assert!(artifact.path.is_absolute());
    assert_eq!(artifact.sha256, digest(bytes));
    assert_eq!(artifact.status, "downloaded");
}

#[test]
fn platform_prepare_rejects_command_unknown_non_apk_and_malformed_declarations_stably() {
    let cases = [
        (
            r#"{executable="fake-installer",args={}}"#,
            "installer.target_unsupported",
        ),
        (
            r#"{kind="android_apk",artifact={artifact="missing"}}"#,
            "installer.artifact_unknown",
        ),
        (
            r#"{kind="android_apk",artifact={artifact="app"},surprise=true}"#,
            "installer.schema_invalid",
        ),
    ];
    for (install, expected_code) in cases {
        let bytes = b"apk";
        let temp = fixture(install, bytes);
        let error = prepare_platform_install_with_transports(
            temp.path(),
            &"android/com.example".parse().unwrap(),
            None,
            Rc::new(Bytes(bytes.to_vec())),
        )
        .unwrap_err();
        assert_eq!(error.code(), expected_code);
    }

    let bytes = b"not apk";
    let temp = fixture(r#"{kind="android_apk",artifact={artifact="app"}}"#, bytes);
    let package = temp.path().join("repo/official/android/com.example");
    fs::write(
        package.join("Manifest"),
        format!("{} payload.zip\n", digest(bytes)),
    )
    .unwrap();
    fs::write(package.join("1.lua"), r#"#!/bin/upa-lua v1
return package_version { updates = {{ version="2", artifacts={{name="app",file_name="payload.zip",url="mock://app"}}, install={kind="android_apk",artifact={artifact="app"}} }} }"#).unwrap();
    let error = prepare_platform_install_with_transports(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        Rc::new(Bytes(bytes.to_vec())),
    )
    .unwrap_err();
    assert_eq!(error.code(), "installer.artifact_unsupported");
}

#[test]
fn platform_prepare_requires_exactly_one_nonempty_android_package_target() {
    let bytes = b"apk";
    for metadata in [
        r#"{"type":"generic","generic":{"id":"com.example"}}"#,
        r#"{"type":"android:app","android":{"package_name":""}}"#,
    ] {
        let temp = fixture_with_metadata(
            r#"{kind="android_apk",artifact={artifact="app"}}"#,
            bytes,
            metadata,
        );
        let error = prepare_platform_install_with_transports(
            temp.path(),
            &"android/com.example".parse().unwrap(),
            None,
            Rc::new(Bytes(bytes.to_vec())),
        )
        .unwrap_err();
        assert_eq!(error.code(), "installer.target_unsupported");
    }
}

#[test]
fn direct_command_install_rejects_android_apk_without_resolving_or_running() {
    let bytes = b"apk";
    let temp = fixture(r#"{kind="android_apk",artifact={artifact="app"}}"#, bytes);
    let runner = Runner::default();
    let error = install_app_with_dependencies(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        Rc::new(Bytes(bytes.to_vec())),
        &Resolver(PathBuf::from("/fake/bin/installer")),
        &runner,
    )
    .unwrap_err();
    assert_eq!(error.code(), "installer.target_unsupported");
    assert!(runner.seen.borrow().is_none());
}

#[test]
fn malformed_installer_does_not_block_download_but_fails_install_after_staging() {
    let bytes = b"apk";
    let temp = fixture(
        r#"{executable="fake-installer", args={}, surprise=true}"#,
        bytes,
    );
    let download = download_app_with_transports(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        Rc::new(Bytes(bytes.to_vec())),
    )
    .unwrap();
    assert!(download.artifacts[0].path.exists());

    let error = install_app_with_dependencies(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        Rc::new(Bytes(bytes.to_vec())),
        &Resolver(PathBuf::from("/fake/bin/installer")),
        &Runner::default(),
    )
    .unwrap_err();
    assert_eq!(error.code(), "installer.schema_invalid");
    assert!(download.artifacts[0].path.exists());
}

#[test]
fn resolves_artifact_to_staged_absolute_path_and_preserves_literal_argv() {
    let bytes = b"apk";
    let temp = fixture(
        r#"{executable="fake-installer",args={"install",{artifact="app"},"$HOME;literal"}}"#,
        bytes,
    );
    let runner = Runner::default();
    let result = install_app_with_dependencies(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        Rc::new(Bytes(bytes.to_vec())),
        &Resolver(PathBuf::from("/fake/bin/installer")),
        &runner,
    )
    .unwrap();
    let command = runner.seen.borrow().clone().unwrap();
    assert_eq!(command.executable, PathBuf::from("/fake/bin/installer"));
    assert_eq!(command.args[0], "install");
    assert!(Path::new(&command.args[1]).is_absolute());
    assert_eq!(command.args[1], result.artifacts[0].path.to_string_lossy());
    assert_eq!(command.args[2], "$HOME;literal");
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, "installed");
}

#[test]
fn observer_runs_immediately_before_runner() {
    struct Ordered {
        events: Rc<RefCell<Vec<&'static str>>>,
    }
    impl CommandObserver for Ordered {
        fn before_execute(&self, _: &ResolvedInstallerCommand) -> std::io::Result<()> {
            self.events.borrow_mut().push("observed");
            Ok(())
        }
    }
    impl CommandRunner for Ordered {
        fn run(&self, _: &ResolvedInstallerCommand) -> std::io::Result<RunnerOutput> {
            self.events.borrow_mut().push("ran");
            Ok(RunnerOutput {
                status: 0,
                stdout: vec![],
                stderr: vec![],
            })
        }
    }
    let bytes = b"apk";
    let temp = fixture(r#"{executable="fake-installer",args={}}"#, bytes);
    let events = Rc::new(RefCell::new(Vec::new()));
    let ordered = Ordered {
        events: events.clone(),
    };
    install_app_with_dependencies_and_observer(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        Rc::new(Bytes(bytes.to_vec())),
        &Resolver(PathBuf::from("/fake/bin/installer")),
        &ordered,
        &ordered,
    )
    .unwrap();
    assert_eq!(&*events.borrow(), &["observed", "ran"]);
}

#[test]
fn rejects_unknown_reference_after_preserving_staged_file() {
    let bytes = b"apk";
    let temp = fixture(
        r#"{executable="fake-installer",args={{artifact="missing"}}}"#,
        bytes,
    );
    let error = install_app_with_dependencies(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        Rc::new(Bytes(bytes.to_vec())),
        &Resolver(PathBuf::from("/fake/bin/installer")),
        &Runner::default(),
    )
    .unwrap_err();
    assert_eq!(error.code(), "installer.artifact_unknown");
    assert!(temp
        .path()
        .join("downloads")
        .read_dir()
        .unwrap()
        .next()
        .is_some());
}

#[test]
fn process_runner_executes_fake_file_with_exact_argv_without_shell_expansion() {
    let bytes = b"apk";
    let temp = fixture(
        r#"{executable="fake-installer",args={"install",{artifact="app"},"$HOME;literal"}}"#,
        bytes,
    );
    let executable = fake_installer(temp.path());
    let result = install_app_with_dependencies(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        Rc::new(Bytes(bytes.to_vec())),
        &Resolver(executable),
        &getter_operations::app::ProcessCommandRunner,
    )
    .unwrap();
    let lines: Vec<_> = result.stdout.lines().collect();
    assert_eq!(lines[0], "install");
    assert_eq!(lines[1], result.artifacts[0].path.to_string_lossy());
    assert_eq!(lines[2], "$HOME;literal");
}

#[test]
fn process_runner_bounds_and_concurrently_drains_both_output_streams() {
    const CAPTURE_LIMIT: usize = 64 * 1024;

    let temp = tempfile::tempdir().unwrap();
    let executable = native_fixture(
        temp.path(),
        "large-output",
        r#"use std::io::{self, Write};
fn main() {
    let stdout = vec![b'o'; 2 * 1024 * 1024];
    let stderr = vec![b'e'; 2 * 1024 * 1024];
    let stdout_thread = std::thread::spawn(move || io::stdout().write_all(&stdout).unwrap());
    io::stderr().write_all(&stderr).unwrap();
    stdout_thread.join().unwrap();
}"#,
    );
    let output = getter_operations::app::ProcessCommandRunner
        .run(&ResolvedInstallerCommand {
            executable,
            args: Vec::new(),
        })
        .unwrap();

    assert_eq!(output.status, 0);
    assert_eq!(output.stdout.len(), CAPTURE_LIMIT);
    assert_eq!(output.stderr.len(), CAPTURE_LIMIT);
    assert!(output.stdout.iter().all(|byte| *byte == b'o'));
    assert!(output.stderr.iter().all(|byte| *byte == b'e'));
}

#[test]
fn command_not_found_is_stable_and_preserves_staged_file() {
    let bytes = b"apk";
    let temp = fixture(r#"{executable="absent",args={}}"#, bytes);
    let error = install_app_with_dependencies(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        Rc::new(Bytes(bytes.to_vec())),
        &Resolver(PathBuf::from("/unused")),
        &Runner::default(),
    )
    .unwrap_err();
    assert_eq!(error.code(), "installer.command_not_found", "{error:?}");
    assert!(temp
        .path()
        .join("downloads")
        .read_dir()
        .unwrap()
        .next()
        .is_some());
}

#[test]
fn runner_io_error_is_stable_and_observer_precedes_attempt() {
    struct Ordered {
        events: Rc<RefCell<Vec<&'static str>>>,
    }
    impl CommandObserver for Ordered {
        fn before_execute(&self, _: &ResolvedInstallerCommand) -> std::io::Result<()> {
            self.events.borrow_mut().push("observed");
            Ok(())
        }
    }
    impl CommandRunner for Ordered {
        fn run(&self, _: &ResolvedInstallerCommand) -> std::io::Result<RunnerOutput> {
            self.events.borrow_mut().push("ran");
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "fixture spawn failed",
            ))
        }
    }
    let bytes = b"apk";
    let temp = fixture(r#"{executable="fake-installer",args={}}"#, bytes);
    let events = Rc::new(RefCell::new(Vec::new()));
    let ordered = Ordered {
        events: events.clone(),
    };
    let error = install_app_with_dependencies_and_observer(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        Rc::new(Bytes(bytes.to_vec())),
        &Resolver(PathBuf::from("/fake/bin/installer")),
        &ordered,
        &ordered,
    )
    .unwrap_err();
    assert_eq!(error.code(), "installer.command_spawn_failed");
    assert_eq!(&*events.borrow(), &["observed", "ran"]);
}

#[test]
fn nonzero_child_is_stable_and_observer_precedes_failure() {
    struct Failed {
        events: Rc<RefCell<Vec<&'static str>>>,
    }
    impl CommandObserver for Failed {
        fn before_execute(&self, _: &ResolvedInstallerCommand) -> std::io::Result<()> {
            self.events.borrow_mut().push("observed");
            Ok(())
        }
    }
    impl CommandRunner for Failed {
        fn run(&self, _: &ResolvedInstallerCommand) -> std::io::Result<RunnerOutput> {
            self.events.borrow_mut().push("ran");
            Ok(RunnerOutput {
                status: 23,
                stdout: Vec::new(),
                stderr: b"failed".to_vec(),
            })
        }
    }
    let bytes = b"apk";
    let temp = fixture(r#"{executable="fake-installer",args={}}"#, bytes);
    let events = Rc::new(RefCell::new(Vec::new()));
    let failed = Failed {
        events: events.clone(),
    };
    let error = install_app_with_dependencies_and_observer(
        temp.path(),
        &"android/com.example".parse().unwrap(),
        None,
        Rc::new(Bytes(bytes.to_vec())),
        &Resolver(PathBuf::from("/fake/bin/installer")),
        &failed,
        &failed,
    )
    .unwrap_err();
    assert_eq!(error.code(), "installer.command_failed");
    assert_eq!(&*events.borrow(), &["observed", "ran"]);
    assert!(temp
        .path()
        .join("downloads")
        .read_dir()
        .unwrap()
        .next()
        .is_some());
}
