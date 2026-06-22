use cucumber::{given, then, when, World as _};
use rusqlite::Connection;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[derive(Debug, Default, cucumber::World)]
struct CliWorld {
    temp: Option<TempDir>,
    data_dir: Option<PathBuf>,
    bundle: Option<PathBuf>,
    legacy_db: Option<PathBuf>,
    inventory: Option<PathBuf>,
    autogen_preview: Option<PathBuf>,
    fixture_repo_id: Option<String>,
    fixture_repo_path: Option<PathBuf>,
    fixture_package_id: Option<String>,
    output: Option<std::process::Output>,
    json: Option<Value>,
}

#[given("an empty getter data directory")]
fn empty_getter_data_dir(world: &mut CliWorld) {
    let temp = tempfile::tempdir().expect("create tempdir");
    let data_dir = temp.path().join("getter-data");
    world.temp = Some(temp);
    world.data_dir = Some(data_dir);
    world.json = None;
}

#[given("an initialized getter data directory")]
fn initialized_getter_data_dir(world: &mut CliWorld) {
    empty_getter_data_dir(world);
    let output = run_getter(world, ["init".to_owned()]);
    assert_success(&output);
}

#[given("a corrupted legacy export bundle")]
fn corrupted_legacy_export_bundle(world: &mut CliWorld) {
    let temp = world.temp.as_ref().expect("tempdir exists");
    let bundle = temp.path().join("corrupted-room-bundle.json");
    fs::write(&bundle, "not-json").expect("write corrupted bundle");
    world.bundle = Some(bundle);
}

#[given("a syntactically valid legacy export bundle with an Android app")]
fn valid_legacy_export_bundle_with_android_app(world: &mut CliWorld) {
    let temp = world.temp.as_ref().expect("tempdir exists");
    let bundle = temp.path().join("valid-room-bundle.json");
    fs::write(
        &bundle,
        r#"{
  "format": "upgradeall-legacy-room-bundle",
  "version": 17,
  "apps": [
    {
      "kind": "android",
      "installed_id": "org.fdroid.fdroid",
      "official_package_available": true,
      "ignored_version": "1.20.0",
      "favorite": true
    }
  ]
}"#,
    )
    .expect("write valid bundle");
    world.bundle = Some(bundle);
}

#[given("a legacy Room v17 database with an Android app and extra app state")]
fn legacy_room_v17_database_with_android_app(world: &mut CliWorld) {
    let temp = world.temp.as_ref().expect("tempdir exists");
    let legacy_db = temp.path().join("app_metadata_database.db");
    create_fixture_legacy_room_db(&legacy_db, 17, true);
    world.legacy_db = Some(legacy_db);
}

#[given("an unsupported legacy Room database")]
fn unsupported_legacy_room_database(world: &mut CliWorld) {
    let temp = world.temp.as_ref().expect("tempdir exists");
    let legacy_db = temp.path().join("app_metadata_database.db");
    create_fixture_legacy_room_db(&legacy_db, 16, true);
    world.legacy_db = Some(legacy_db);
}

#[given("a malformed legacy Room database")]
fn malformed_legacy_room_database(world: &mut CliWorld) {
    let temp = world.temp.as_ref().expect("tempdir exists");
    let legacy_db = temp.path().join("app_metadata_database.db");
    create_fixture_legacy_room_db(&legacy_db, 17, false);
    world.legacy_db = Some(legacy_db);
}

#[given("a legacy Room v17 database with only unsupported app rows")]
fn legacy_room_v17_database_with_only_unsupported_app_rows(world: &mut CliWorld) {
    let temp = world.temp.as_ref().expect("tempdir exists");
    let legacy_db = temp.path().join("app_metadata_database.db");
    create_fixture_legacy_room_db(&legacy_db, 17, true);
    let conn = Connection::open(&legacy_db).expect("open legacy Room fixture");
    conn.execute("DELETE FROM extra_app", [])
        .expect("delete fixture extra_app");
    conn.execute("DELETE FROM app", [])
        .expect("delete fixture app");
    conn.execute(
        "INSERT INTO app(id, name, app_id, ignore_version_number, star) VALUES (1, 'Unsupported', ?1, NULL, 0)",
        [r#"{"unknown_provider":"com.example.unsupported"}"#],
    )
    .expect("insert unsupported app row");
    world.legacy_db = Some(legacy_db);
}

#[given(expr = "an installed inventory with Android app {string} labeled {string}")]
fn installed_inventory_with_android_app(world: &mut CliWorld, package_name: String, label: String) {
    let temp = world.temp.as_ref().expect("tempdir exists");
    let inventory = temp.path().join("installed-inventory.json");
    fs::write(
        &inventory,
        serde_json::to_vec_pretty(&serde_json::json!({
            "format": "upgradeall-installed-inventory",
            "version": 1,
            "items": [
                {
                    "kind": "android",
                    "package_name": package_name,
                    "label": label,
                    "version_name": "1.0.0",
                    "version_code": 1,
                }
            ]
        }))
        .expect("inventory serializes"),
    )
    .expect("write inventory");
    world.inventory = Some(inventory);
}

#[given("an empty installed inventory")]
fn empty_installed_inventory(world: &mut CliWorld) {
    let temp = world.temp.as_ref().expect("tempdir exists");
    let inventory = temp.path().join("installed-inventory.json");
    fs::write(
        &inventory,
        serde_json::to_vec_pretty(&serde_json::json!({
            "format": "upgradeall-installed-inventory",
            "version": 1,
            "items": []
        }))
        .expect("inventory serializes"),
    )
    .expect("write inventory");
    world.inventory = Some(inventory);
}

#[given(expr = "a tampered autogen cleanup preview for package {string}")]
fn tampered_autogen_cleanup_preview_for_package(world: &mut CliWorld, package_id: String) {
    let temp = world.temp.as_ref().expect("tempdir exists");
    let preview = temp.path().join("tampered-cleanup-preview.json");
    fs::write(
        &preview,
        serde_json::to_vec_pretty(&serde_json::json!({
            "operation": "cleanup.preview",
            "target_repo_id": "local_autogen",
            "target_repo_path": local_autogen_repo_path(world),
            "summary": {
                "candidate_count": 1,
                "skipped_count": 0,
                "write_count": 0,
                "delete_count": 1,
            },
            "candidates": [
                {
                    "package_id": package_id,
                    "action": "delete",
                    "output_relative_path": package_relative_path(&package_id),
                    "content_hash": "fnv1a64:0000000000000000",
                    "reason": "not_in_installed_inventory",
                }
            ],
            "skipped": [],
            "diagnostics": [],
        }))
        .expect("preview serializes"),
    )
    .expect("write tampered cleanup preview");
    world.autogen_preview = Some(preview);
}

#[given(expr = "a fixture Lua repository {string} with package {string}")]
fn fixture_lua_repository(world: &mut CliWorld, repo_id: String, package_id: String) {
    create_fixture_lua_repository(world, repo_id, package_id, "F-Droid".to_owned());
}

#[given(expr = "a fixture Lua repository {string} with package {string} named {string}")]
fn fixture_lua_repository_named(
    world: &mut CliWorld,
    repo_id: String,
    package_id: String,
    package_name: String,
) {
    create_fixture_lua_repository(world, repo_id, package_id, package_name);
}

#[given(expr = "a fixture Lua repository {string} with invalid Lua package {string}")]
fn fixture_lua_repository_invalid_lua(world: &mut CliWorld, repo_id: String, package_id: String) {
    create_custom_fixture_lua_repository(
        world,
        repo_id,
        package_id.clone(),
        format!("return package_def {{ id = \"{package_id}\", name = "),
    );
}

#[given(expr = "a fixture Lua repository {string} with schema-invalid package {string}")]
fn fixture_lua_repository_invalid_schema(
    world: &mut CliWorld,
    repo_id: String,
    package_id: String,
) {
    create_custom_fixture_lua_repository(
        world,
        repo_id,
        package_id.clone(),
        format!("return {{ id = \"{package_id}\" }}"),
    );
}

#[given(expr = "a fixture Lua repository {string} with mismatched package path {string}")]
fn fixture_lua_repository_mismatched_path(
    world: &mut CliWorld,
    repo_id: String,
    package_id: String,
) {
    create_custom_fixture_lua_repository(
        world,
        repo_id,
        package_id,
        "return { id = \"android/com.termux\", name = \"Termux\" }".to_owned(),
    );
}

#[given(expr = "an incomplete Lua repository {string}")]
fn incomplete_lua_repository(world: &mut CliWorld, repo_id: String) {
    let temp = world.temp.as_ref().expect("tempdir exists");
    let repo_path = temp.path().join(format!("repo-{repo_id}"));
    fs::create_dir_all(&repo_path).expect("create incomplete repo dir");
    fs::write(
        repo_path.join("repo.toml"),
        format!(
            "id = \"{repo_id}\"\nname = \"Fixture {repo_id}\"\npriority = 0\napi_version = \"getter.repo.v1\"\n"
        ),
    )
    .expect("write repo.toml");
    world.fixture_repo_id = Some(repo_id);
    world.fixture_repo_path = Some(repo_path);
    world.fixture_package_id = None;
}

#[when("I run getter init for that directory")]
fn run_getter_init(world: &mut CliWorld) {
    let output = run_getter(world, ["init".to_owned()]);
    world.output = Some(output);
    world.json = None;
}

#[when("I run getter app list for that directory")]
fn run_getter_app_list(world: &mut CliWorld) {
    let output = run_getter(world, ["app".to_owned(), "list".to_owned()]);
    world.output = Some(output);
    world.json = None;
}

#[when("I run getter hub list for that directory")]
fn run_getter_hub_list(world: &mut CliWorld) {
    let output = run_getter(world, ["hub".to_owned(), "list".to_owned()]);
    world.output = Some(output);
    world.json = None;
}

#[when("I run getter repo list for that directory")]
fn run_getter_repo_list(world: &mut CliWorld) {
    let output = run_getter(world, ["repo".to_owned(), "list".to_owned()]);
    world.output = Some(output);
    world.json = None;
}

#[when("I run getter storage validate for that directory")]
fn run_getter_storage_validate(world: &mut CliWorld) {
    let output = run_getter(world, ["storage".to_owned(), "validate".to_owned()]);
    world.output = Some(output);
    world.json = None;
}

#[when("I run getter repo add for that repository with priority 0")]
fn run_getter_repo_add(world: &mut CliWorld) {
    let repo_id = world
        .fixture_repo_id
        .as_ref()
        .expect("fixture repo id exists")
        .clone();
    run_getter_repo_add_with_priority(world, &repo_id, 0);
}

#[when(expr = "I run getter repo add for repository {string} with priority {int}")]
fn run_getter_repo_add_named(world: &mut CliWorld, repo_id: String, priority: i32) {
    run_getter_repo_add_with_priority(world, &repo_id, priority);
}

#[when("I run getter repo eval for that repository")]
fn run_getter_repo_eval(world: &mut CliWorld) {
    let repo_id = world
        .fixture_repo_id
        .as_ref()
        .expect("fixture repo id exists");
    let output = run_getter(
        world,
        ["repo".to_owned(), "eval".to_owned(), repo_id.to_owned()],
    );
    world.output = Some(output);
    world.json = None;
}

#[when("I run getter repo validate for that repository")]
fn run_getter_repo_validate(world: &mut CliWorld) {
    let repo_path = world
        .fixture_repo_path
        .as_ref()
        .expect("fixture repo path exists");
    let output = run_getter(
        world,
        [
            "repo".to_owned(),
            "validate".to_owned(),
            repo_path.to_string_lossy().to_string(),
        ],
    );
    world.output = Some(output);
    world.json = None;
}

#[when("I run getter package eval for that fixture package")]
fn run_getter_package_eval(world: &mut CliWorld) {
    let repo_id = world
        .fixture_repo_id
        .as_ref()
        .expect("fixture repo id exists");
    let package_id = world
        .fixture_package_id
        .as_ref()
        .expect("fixture package id exists");
    let output = run_getter(
        world,
        [
            "package".to_owned(),
            "eval".to_owned(),
            package_id.to_owned(),
            "--repo".to_owned(),
            repo_id.to_owned(),
        ],
    );
    world.output = Some(output);
    world.json = None;
}

#[when(expr = "I run getter package eval for package {string}")]
fn run_getter_package_eval_without_repo(world: &mut CliWorld, package_id: String) {
    let output = run_getter(world, ["package".to_owned(), "eval".to_owned(), package_id]);
    world.output = Some(output);
    world.json = None;
}

#[when("I run getter legacy import-room-bundle for that bundle")]
fn run_getter_legacy_import(world: &mut CliWorld) {
    let bundle = world.bundle.as_ref().expect("bundle exists");
    let output = run_getter(
        world,
        [
            "legacy".to_owned(),
            "import-room-bundle".to_owned(),
            bundle.to_string_lossy().to_string(),
        ],
    );
    world.output = Some(output);
    world.json = None;
}

#[when("I run getter legacy import-room-db for that database")]
fn run_getter_legacy_import_db(world: &mut CliWorld) {
    let legacy_db = world.legacy_db.as_ref().expect("legacy db exists");
    let output = run_getter(
        world,
        [
            "legacy".to_owned(),
            "import-room-db".to_owned(),
            legacy_db.to_string_lossy().to_string(),
        ],
    );
    world.output = Some(output);
    world.json = None;
}

#[when("I run getter legacy report-list for that directory")]
fn run_getter_legacy_report_list(world: &mut CliWorld) {
    let output = run_getter(world, ["legacy".to_owned(), "report-list".to_owned()]);
    world.output = Some(output);
    world.json = None;
}

#[when("I run getter autogen installed preview for that inventory")]
fn run_getter_autogen_installed_preview(world: &mut CliWorld) {
    let inventory = world.inventory.as_ref().expect("inventory exists");
    let output = run_getter(
        world,
        [
            "autogen".to_owned(),
            "installed".to_owned(),
            "preview".to_owned(),
            "--inventory".to_owned(),
            inventory.to_string_lossy().to_string(),
        ],
    );
    world.output = Some(output);
    world.json = None;
}

#[when("I run getter autogen installed apply for that preview with accept-all")]
fn run_getter_autogen_installed_apply_accept_all(world: &mut CliWorld) {
    let preview = world
        .autogen_preview
        .as_ref()
        .expect("autogen preview exists");
    let output = run_getter(
        world,
        [
            "autogen".to_owned(),
            "installed".to_owned(),
            "apply".to_owned(),
            "--preview".to_owned(),
            preview.to_string_lossy().to_string(),
            "--accept-all".to_owned(),
        ],
    );
    world.output = Some(output);
    world.json = None;
}

#[when("I run getter autogen cleanup preview for that inventory")]
fn run_getter_autogen_cleanup_preview(world: &mut CliWorld) {
    let inventory = world.inventory.as_ref().expect("inventory exists");
    let output = run_getter(
        world,
        [
            "autogen".to_owned(),
            "cleanup".to_owned(),
            "preview".to_owned(),
            "--inventory".to_owned(),
            inventory.to_string_lossy().to_string(),
        ],
    );
    world.output = Some(output);
    world.json = None;
}

#[when("I run getter autogen cleanup apply for that preview with accept-all")]
fn run_getter_autogen_cleanup_apply_accept_all(world: &mut CliWorld) {
    let preview = world
        .autogen_preview
        .as_ref()
        .expect("autogen preview exists");
    let output = run_getter(
        world,
        [
            "autogen".to_owned(),
            "cleanup".to_owned(),
            "apply".to_owned(),
            "--preview".to_owned(),
            preview.to_string_lossy().to_string(),
            "--accept-all".to_owned(),
        ],
    );
    world.output = Some(output);
    world.json = None;
}

#[when("I run getter repo validate for local_autogen")]
fn run_getter_repo_validate_for_local_autogen(world: &mut CliWorld) {
    let repo_path = local_autogen_repo_path(world);
    let output = run_getter(
        world,
        [
            "repo".to_owned(),
            "validate".to_owned(),
            repo_path.to_string_lossy().to_string(),
        ],
    );
    world.output = Some(output);
    world.json = None;
}

#[when(expr = "I run getter package eval for package {string} from local_autogen")]
fn run_getter_package_eval_from_local_autogen(world: &mut CliWorld, package_id: String) {
    let output = run_getter(
        world,
        [
            "package".to_owned(),
            "eval".to_owned(),
            package_id,
            "--repo".to_owned(),
            "local_autogen".to_owned(),
        ],
    );
    world.output = Some(output);
    world.json = None;
}

#[then("the command succeeds")]
fn command_succeeds(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("command output exists");
    assert_success(output);
}

#[then("the command fails with a documented migration error")]
fn command_fails_with_migration_error(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("command output exists");
    assert_eq!(output.status.code(), Some(20));
    let json = parse_stdout(output);
    assert_eq!(json["ok"], false);
    assert!(
        json["command"] == "legacy import-room-bundle"
            || json["command"] == "legacy import-room-db"
    );
    assert!(matches!(
        json["error"]["code"].as_str(),
        Some("migration.invalid_bundle" | "migration.invalid_db" | "migration.unsupported_db")
    ));
    world.json = Some(json);
}

#[then("the command fails with an autogen error")]
fn command_fails_with_autogen_error(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("command output exists");
    assert_eq!(output.status.code(), Some(1));
    let json = parse_stdout(output);
    assert_eq!(json["ok"], false);
    assert_eq!(json["command"], "autogen cleanup apply");
    assert_eq!(json["error"]["code"], "autogen.error");
    world.json = Some(json);
}

#[then(expr = "the command fails with direct DB migration error {string}")]
fn command_fails_with_direct_db_migration_error(world: &mut CliWorld, code: String) {
    let output = world.output.as_ref().expect("command output exists");
    assert_eq!(output.status.code(), Some(20));
    let json = parse_stdout(output);
    assert_eq!(json["ok"], false);
    assert_eq!(json["command"], "legacy import-room-db");
    assert_eq!(json["error"]["code"], code);
    let report_path = json["error"]["report_path"]
        .as_str()
        .expect("report_path should be a string");
    let report = fs::read_to_string(report_path).expect("report should be readable");
    let report_json: Value = serde_json::from_str(&report).expect("report should be JSON");
    assert_eq!(report_json["code"], code);
    world.json = Some(json);
}

#[then("the output is valid JSON")]
fn output_is_valid_json(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("command output exists");
    let json = parse_stdout(output);
    assert!(
        json.is_object(),
        "stdout JSON should be an object: {json:?}"
    );
    world.json = Some(json);
}

#[then("the getter data directory is usable")]
fn getter_data_directory_is_usable(world: &mut CliWorld) {
    let data_dir = world.data_dir.as_ref().expect("data dir exists");
    assert!(data_dir.join("main.db").is_file(), "main.db should exist");
    assert!(data_dir.join("cache.db").is_file(), "cache.db should exist");

    let output = run_getter(world, ["app".to_owned(), "list".to_owned()]);
    assert_success(&output);
}

#[then("the output contains an empty app list")]
fn output_contains_empty_app_list(world: &mut CliWorld) {
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "app list");
    assert_eq!(json["data"]["apps"], Value::Array(Vec::new()));
}

#[then("the output contains an empty hub list")]
fn output_contains_empty_hub_list(world: &mut CliWorld) {
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "hub list");
    assert_eq!(json["data"]["hubs"], Value::Array(Vec::new()));
}

#[then("the output contains an empty repository list")]
fn output_contains_empty_repository_list(world: &mut CliWorld) {
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "repo list");
    assert_eq!(json["data"]["repositories"], Value::Array(Vec::new()));
}

#[then("the output reports valid storage")]
fn output_reports_valid_storage(world: &mut CliWorld) {
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "storage validate");
    assert_eq!(json["data"]["valid"], true);
    assert!(json["data"]["main_db"].as_str().is_some());
    assert!(json["data"]["cache_db"].as_str().is_some());
}

#[then("the output contains the added repository")]
fn output_contains_added_repository(world: &mut CliWorld) {
    let repo_id = world
        .fixture_repo_id
        .as_ref()
        .expect("fixture repo id exists")
        .clone();
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "repo add");
    assert_eq!(json["data"]["repository"]["id"], repo_id);
}

#[then("the output reports a valid repository without network")]
fn output_reports_valid_repository_without_network(world: &mut CliWorld) {
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "repo validate");
    assert_eq!(json["data"]["valid"], true);
    assert_eq!(json["data"]["network_required"], false);
    assert_eq!(json["data"]["diagnostics"], Value::Array(Vec::new()));
    assert_eq!(json["data"]["package_count"], 1);
}

#[then(expr = "the output reports repository diagnostic {string}")]
fn output_reports_repository_diagnostic(world: &mut CliWorld, code: String) {
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "repo validate");
    assert_eq!(json["data"]["valid"], false);
    assert_eq!(json["data"]["network_required"], false);
    let diagnostics = json["data"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic["code"].as_str() == Some(code.as_str()))
        .unwrap_or_else(|| panic!("diagnostics should contain {code}: {diagnostics:?}"));
    assert_eq!(diagnostic["severity"], "error");
    assert!(diagnostic["message"].as_str().is_some());
    assert!(diagnostic["location"]["path"].as_str().is_some());
    if code == "package.schema" {
        assert_eq!(diagnostic["package_id"], "android/org.fdroid.fdroid");
        assert_eq!(diagnostic["location"]["field"], "name");
    }
}

#[then("the output contains the evaluated fixture package")]
fn output_contains_evaluated_fixture_package(world: &mut CliWorld) {
    let package_id = world
        .fixture_package_id
        .as_ref()
        .expect("fixture package id exists")
        .clone();
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "repo eval");
    let packages = json["data"]["packages"]
        .as_array()
        .expect("packages should be an array");
    assert!(
        packages
            .iter()
            .any(|package| package["id"].as_str() == Some(package_id.as_str())),
        "repo eval packages should include {package_id}: {packages:?}"
    );
}

#[then("the output contains the fixture package")]
fn output_contains_fixture_package(world: &mut CliWorld) {
    let package_id = world
        .fixture_package_id
        .as_ref()
        .expect("fixture package id exists")
        .clone();
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "package eval");
    assert_eq!(json["data"]["package"]["id"], package_id);
    assert_eq!(json["data"]["package"]["name"], "F-Droid");
}

#[then(expr = "the output contains package {string} named {string}")]
fn output_contains_named_package(world: &mut CliWorld, package_id: String, package_name: String) {
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "package eval");
    assert_eq!(json["data"]["package"]["id"], package_id);
    assert_eq!(json["data"]["package"]["name"], package_name);
}

#[then(expr = "the autogen preview contains candidate {string}")]
fn autogen_preview_contains_candidate(world: &mut CliWorld, package_id: String) {
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "autogen installed preview");
    assert_eq!(json["data"]["operation"], "installed.preview");
    let candidates = json["data"]["candidates"]
        .as_array()
        .expect("candidates array");
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate["package_id"].as_str() == Some(package_id.as_str())),
        "preview should contain {package_id}: {candidates:?}"
    );
}

#[then("the local_autogen repository has not been written")]
fn local_autogen_repository_has_not_been_written(world: &mut CliWorld) {
    assert!(
        !local_autogen_repo_path(world).exists(),
        "preview must not create local_autogen"
    );
}

#[then("I save the autogen preview to a file")]
fn save_autogen_preview_to_file(world: &mut CliWorld) {
    let data = current_json(world)
        .get("data")
        .expect("autogen command data exists")
        .clone();
    let temp = world.temp.as_ref().expect("tempdir exists");
    let preview = temp.path().join("autogen-preview.json");
    fs::write(
        &preview,
        serde_json::to_vec_pretty(&data).expect("preview serializes"),
    )
    .expect("write autogen preview");
    world.autogen_preview = Some(preview);
}

#[then(expr = "the local_autogen repository contains generated package {string}")]
fn local_autogen_repository_contains_generated_package(world: &mut CliWorld, package_id: String) {
    let path = local_autogen_repo_path(world).join(package_relative_path(&package_id));
    assert!(path.is_file(), "generated package should exist: {path:?}");
    let content = fs::read_to_string(&path).expect("generated package readable");
    assert!(content.contains("@generated by UpgradeAll getter local_autogen"));
}

#[then(expr = "the app list contains autogen tracked package {string}")]
fn app_list_contains_autogen_tracked_package(world: &mut CliWorld, package_id: String) {
    let output = run_getter(world, ["app".to_owned(), "list".to_owned()]);
    assert_success(&output);
    let json = parse_stdout(&output);
    let apps = json["data"]["apps"].as_array().expect("apps array");
    let app = apps
        .iter()
        .find(|app| app["id"].as_str() == Some(package_id.as_str()))
        .unwrap_or_else(|| panic!("app list should contain {package_id}: {apps:?}"));
    assert_eq!(app["repository_id"], "local_autogen");
    assert_eq!(app["package_resolution"], "generate_local_package");
}

#[then(expr = "the autogen preview skips package {string} because repository {string} covers it")]
fn autogen_preview_skips_package_because_repository_covers_it(
    world: &mut CliWorld,
    package_id: String,
    repository_id: String,
) {
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "autogen installed preview");
    let skipped = json["data"]["skipped"].as_array().expect("skipped array");
    let skip = skipped
        .iter()
        .find(|skip| skip["package_id"].as_str() == Some(package_id.as_str()))
        .unwrap_or_else(|| panic!("skipped should contain {package_id}: {skipped:?}"));
    assert_eq!(skip["reason"], "covered_by_higher_priority_repo");
    assert_eq!(skip["covering_repo_id"], repository_id);
}

#[then(expr = "the autogen cleanup preview contains delete candidate {string}")]
fn autogen_cleanup_preview_contains_delete_candidate(world: &mut CliWorld, package_id: String) {
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "autogen cleanup preview");
    assert_eq!(json["data"]["operation"], "cleanup.preview");
    let candidates = json["data"]["candidates"]
        .as_array()
        .expect("candidates array");
    let candidate = candidates
        .iter()
        .find(|candidate| candidate["package_id"].as_str() == Some(package_id.as_str()))
        .unwrap_or_else(|| {
            panic!("cleanup candidates should contain {package_id}: {candidates:?}")
        });
    assert_eq!(candidate["action"], "delete");
}

#[then(expr = "the local_autogen repository does not contain generated package {string}")]
fn local_autogen_repository_does_not_contain_generated_package(
    world: &mut CliWorld,
    package_id: String,
) {
    let path = local_autogen_repo_path(world).join(package_relative_path(&package_id));
    assert!(
        !path.exists(),
        "generated package should be deleted: {path:?}"
    );
}

#[then(expr = "the app list does not contain package {string}")]
fn app_list_does_not_contain_package(world: &mut CliWorld, package_id: String) {
    let output = run_getter(world, ["app".to_owned(), "list".to_owned()]);
    assert_success(&output);
    let json = parse_stdout(&output);
    let apps = json["data"]["apps"].as_array().expect("apps array");
    assert!(
        apps.iter()
            .all(|app| app["id"].as_str() != Some(package_id.as_str())),
        "app list must not contain {package_id}: {apps:?}"
    );
}

#[then(expr = "I replace generated autogen package {string} with user-edited content")]
fn replace_generated_autogen_package_with_user_edited_content(
    world: &mut CliWorld,
    package_id: String,
) {
    let path = local_autogen_repo_path(world).join(package_relative_path(&package_id));
    assert!(path.is_file(), "generated package should exist before edit");
    fs::write(
        &path,
        format!(
            "-- user edited\nreturn package_def {{ id = {id:?}, name = \"Edited Autogen\" }}\n",
            id = package_id
        ),
    )
    .expect("overwrite generated package with user edit");
}

#[then(expr = "local repository contains preserved package {string}")]
fn local_repository_contains_preserved_package(world: &mut CliWorld, package_id: String) {
    let local_path = world
        .data_dir
        .as_ref()
        .expect("data dir exists")
        .join("repositories")
        .join("local")
        .join(package_relative_path(&package_id));
    assert!(
        local_path.is_file(),
        "modified autogen file should be preserved into local: {local_path:?}"
    );
    let content = fs::read_to_string(local_path).expect("preserved local file readable");
    assert!(content.contains("Edited Autogen"));
}

#[then("no partially usable imported state is created")]
fn no_partially_usable_imported_state(world: &mut CliWorld) {
    let output = run_getter(world, ["app".to_owned(), "list".to_owned()]);
    assert_success(&output);
    let json = parse_stdout(&output);
    assert_eq!(json["data"]["apps"], Value::Array(Vec::new()));
}

#[then("a sanitized migration report is available")]
fn sanitized_migration_report_available(world: &mut CliWorld) {
    let json = current_json(world);
    let report_path = json["error"]["report_path"]
        .as_str()
        .expect("report_path should be a string");
    let report = fs::read_to_string(report_path).expect("report should be readable");
    let report_json: Value = serde_json::from_str(&report).expect("report should be JSON");
    assert_eq!(report_json["ok"], false);
    assert_eq!(report_json["imported_records"], 0);
    assert!(
        report_json.get("source_file_name").is_some()
            || report_json.get("bundle_file_name").is_some()
    );
    assert!(
        report_json.get("raw_bundle").is_none(),
        "report must not include raw bundle content"
    );
}

#[then("the import reports one tracked app")]
fn import_reports_one_tracked_app(world: &mut CliWorld) {
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert!(
        json["command"] == "legacy import-room-bundle"
            || json["command"] == "legacy import-room-db"
    );
    assert_eq!(json["data"]["imported_records"], 1);
    assert_eq!(
        json["data"]["apps"].as_array().expect("apps array").len(),
        1
    );
}

#[then("the direct migration reports dropped legacy hub warnings")]
fn direct_migration_reports_dropped_hub_warnings(world: &mut CliWorld) {
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "legacy import-room-db");
    assert_eq!(json["data"]["source_counts"]["hub_rows"], 1);
    assert_eq!(json["data"]["source_counts"]["extra_hub_rows"], 1);
    let warnings = json["data"]["warnings"].as_array().expect("warnings array");
    assert!(warnings
        .iter()
        .any(|warning| { warning["code"].as_str() == Some("legacy.dropped_hub_rows") }));
    assert!(warnings
        .iter()
        .any(|warning| { warning["code"].as_str() == Some("legacy.dropped_extra_hub_rows") }));
}

#[then("the direct migration report stays sanitized")]
fn direct_migration_report_stays_sanitized(world: &mut CliWorld) {
    let json = current_json(world);
    let stdout = serde_json::to_string(json).expect("JSON output serializes");
    assert_sanitized_direct_room_report_text(&stdout);
    let report_path = json["data"]["report_path"]
        .as_str()
        .expect("report_path should be a string");
    let report = fs::read_to_string(report_path).expect("report should be readable");
    assert_sanitized_direct_room_report_text(&report);
    let report_json: Value = serde_json::from_str(&report).expect("report should be JSON");
    assert_eq!(report_json["source_counts"]["hub_rows"], 1);
    assert_eq!(report_json["source_counts"]["extra_hub_rows"], 1);
    assert_report_has_drop_warnings(&report_json);
}

#[then("the output reports the legacy Room migration was already completed")]
fn output_reports_legacy_room_migration_already_completed(world: &mut CliWorld) {
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "legacy import-room-db");
    assert_eq!(json["data"]["already_imported"], true);
    assert_eq!(json["data"]["imported_records"], 0);
    assert_eq!(
        json["data"]["apps"].as_array().expect("apps array").len(),
        1
    );
}

#[then("the bundle output reports the legacy Room migration was already completed")]
fn bundle_output_reports_legacy_room_migration_already_completed(world: &mut CliWorld) {
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "legacy import-room-bundle");
    assert_eq!(json["data"]["already_imported"], true);
    assert_eq!(json["data"]["imported_records"], 0);
    assert_eq!(
        json["data"]["apps"].as_array().expect("apps array").len(),
        1
    );
}

#[then(expr = "the output lists migration report {string}")]
fn output_lists_migration_report(world: &mut CliWorld, code: String) {
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "legacy report-list");
    let reports = json["data"]["reports"].as_array().expect("reports array");
    assert!(
        reports
            .iter()
            .any(|report| report["code"].as_str() == Some(code.as_str())),
        "reports should contain {code}: {reports:?}"
    );
}

#[then("the direct migration report list stays sanitized")]
fn direct_migration_report_list_stays_sanitized(world: &mut CliWorld) {
    let json = current_json(world);
    let report_list = serde_json::to_string(json).expect("report list serializes");
    assert_sanitized_direct_room_report_text(&report_list);
    let reports = json["data"]["reports"].as_array().expect("reports array");
    let imported = reports
        .iter()
        .find(|report| report["code"].as_str() == Some("migration.imported"))
        .expect("imported report exists");
    assert_eq!(imported["source_counts"]["hub_rows"], 1);
    assert_eq!(imported["source_counts"]["extra_hub_rows"], 1);
    assert_report_has_drop_warnings(imported);
}

#[then(expr = "the app list contains imported package {string}")]
fn app_list_contains_imported_package(world: &mut CliWorld, package_id: String) {
    let output = run_getter(world, ["app".to_owned(), "list".to_owned()]);
    assert_success(&output);
    let json = parse_stdout(&output);
    let apps = json["data"]["apps"].as_array().expect("apps array");
    let app = apps
        .iter()
        .find(|app| app["id"].as_str() == Some(package_id.as_str()))
        .unwrap_or_else(|| panic!("app list should contain {package_id}: {apps:?}"));
    assert_eq!(app["favorite"], true);
    assert_eq!(app["ignored_version"], "1.20.0");
    assert_eq!(app["package_resolution"], "official_repository_package");
    world.output = Some(output);
    world.json = Some(json);
}

#[then(expr = "the app list contains directly imported package {string}")]
fn app_list_contains_directly_imported_package(world: &mut CliWorld, package_id: String) {
    let output = run_getter(world, ["app".to_owned(), "list".to_owned()]);
    assert_success(&output);
    let json = parse_stdout(&output);
    let apps = json["data"]["apps"].as_array().expect("apps array");
    let app = apps
        .iter()
        .find(|app| app["id"].as_str() == Some(package_id.as_str()))
        .unwrap_or_else(|| panic!("app list should contain {package_id}: {apps:?}"));
    assert_eq!(app["favorite"], true);
    assert_eq!(app["ignored_version"], "1.20.0");
    assert_eq!(app["package_resolution"], "missing_package_definition");
    world.output = Some(output);
    world.json = Some(json);
}

fn current_json(world: &mut CliWorld) -> &Value {
    world
        .json
        .get_or_insert_with(|| parse_stdout(world.output.as_ref().expect("command output exists")))
}

fn local_autogen_repo_path(world: &CliWorld) -> PathBuf {
    world
        .data_dir
        .as_ref()
        .expect("data dir exists")
        .join("repositories")
        .join("local_autogen")
}

fn package_relative_path(package_id: &str) -> PathBuf {
    let (kind, name) = package_id
        .split_once('/')
        .expect("test package id has kind/name");
    PathBuf::from("packages")
        .join(kind)
        .join(format!("{name}.lua"))
}

fn create_fixture_lua_repository(
    world: &mut CliWorld,
    repo_id: String,
    package_id: String,
    package_name: String,
) {
    let temp = world.temp.as_ref().expect("tempdir exists");
    let repo_path = temp.path().join(format!("repo-{repo_id}"));
    let package_name_path = package_id
        .strip_prefix("android/")
        .expect("fixture package id should be android package id");
    fs::create_dir_all(repo_path.join("packages/android")).expect("create packages dir");
    fs::create_dir(repo_path.join("lib")).expect("create lib dir");
    fs::create_dir(repo_path.join("templates")).expect("create templates dir");
    fs::write(
        repo_path.join("repo.toml"),
        format!(
            "id = \"{repo_id}\"\nname = \"Fixture {repo_id}\"\npriority = 0\napi_version = \"getter.repo.v1\"\n"
        ),
    )
    .expect("write repo.toml");
    fs::write(
        repo_path.join(format!("packages/android/{package_name_path}.lua")),
        format!(
            r#"
return package_def {{
  id = "{package_id}",
  name = "{package_name}",
  installed = {{
    {{ kind = "android_package", package_name = "{package_name_path}" }},
  }},
}}
"#
        ),
    )
    .expect("write package Lua");

    world.fixture_repo_id = Some(repo_id);
    world.fixture_repo_path = Some(repo_path);
    world.fixture_package_id = Some(package_id);
}

fn create_fixture_legacy_room_db(path: &PathBuf, version: u32, include_app_table: bool) {
    let conn = Connection::open(path).expect("create legacy Room fixture");
    conn.pragma_update(None, "user_version", version)
        .expect("set user_version");
    if include_app_table {
        conn.execute_batch(
            r#"
CREATE TABLE app (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    app_id TEXT NOT NULL,
    invalid_version_number_field_regex TEXT,
    include_version_number_field_regex TEXT,
    ignore_version_number TEXT,
    cloud_config TEXT,
    enable_hub_list TEXT,
    star INTEGER
);
CREATE TABLE extra_app (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    app_id TEXT NOT NULL,
    mark_version_number TEXT
);
CREATE TABLE hub (
    uuid TEXT PRIMARY KEY,
    hub_config TEXT NOT NULL,
    auth TEXT NOT NULL,
    ignore_app_id_list TEXT NOT NULL,
    applications_mode INTEGER NOT NULL DEFAULT 0,
    user_ignore_app_id_list TEXT NOT NULL,
    sort_point INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE extra_hub (
    id TEXT PRIMARY KEY,
    enable_global INTEGER NOT NULL DEFAULT 0,
    url_replace_search TEXT,
    url_replace_string TEXT
);
"#,
        )
        .expect("create legacy tables");
        let app_id = r#"{"android_app_package":"org.fdroid.fdroid"}"#;
        conn.execute(
            "INSERT INTO app(id, name, app_id, ignore_version_number, star) VALUES (1, 'F-Droid', ?1, '1.10.0', 1)",
            [app_id],
        )
        .expect("insert app");
        conn.execute(
            "INSERT INTO extra_app(id, app_id, mark_version_number) VALUES (1, ?1, '1.20.0')",
            [app_id],
        )
        .expect("insert extra app");
        conn.execute(
            "INSERT INTO hub(uuid, hub_config, auth, ignore_app_id_list, user_ignore_app_id_list) VALUES ('legacy-hub', '{\"secret\":\"UA_DIRECT_DB_SENTINEL_SECRET\"}', '{\"token\":\"UA_DIRECT_DB_SENTINEL_TOKEN\"}', '[]', '[]')",
            [],
        )
        .expect("insert hub");
        conn.execute(
            "INSERT INTO extra_hub(id, enable_global, url_replace_search, url_replace_string) VALUES ('GLOBAL', 1, 'UA_DIRECT_DB_SENTINEL_SEARCH', 'UA_DIRECT_DB_SENTINEL_REPLACE')",
            [],
        )
        .expect("insert extra hub");
    }
}

fn assert_report_has_drop_warnings(report: &Value) {
    let warnings = report["warnings"].as_array().expect("warnings array");
    assert!(warnings
        .iter()
        .any(|warning| warning["code"].as_str() == Some("legacy.dropped_hub_rows")));
    assert!(warnings
        .iter()
        .any(|warning| warning["code"].as_str() == Some("legacy.dropped_extra_hub_rows")));
}

fn assert_sanitized_direct_room_report_text(text: &str) {
    for forbidden in [
        "UA_DIRECT_DB_SENTINEL_SECRET",
        "UA_DIRECT_DB_SENTINEL_TOKEN",
        "UA_DIRECT_DB_SENTINEL_SEARCH",
        "UA_DIRECT_DB_SENTINEL_REPLACE",
        "legacy-hub",
    ] {
        assert!(
            !text.contains(forbidden),
            "direct Room migration report must not expose {forbidden}: {text}"
        );
    }
}

fn create_custom_fixture_lua_repository(
    world: &mut CliWorld,
    repo_id: String,
    package_id: String,
    package_source: String,
) {
    let temp = world.temp.as_ref().expect("tempdir exists");
    let repo_path = temp.path().join(format!("repo-{repo_id}"));
    let package_name_path = package_id
        .strip_prefix("android/")
        .expect("fixture package id should be android package id");
    fs::create_dir_all(repo_path.join("packages/android")).expect("create packages dir");
    fs::create_dir(repo_path.join("lib")).expect("create lib dir");
    fs::create_dir(repo_path.join("templates")).expect("create templates dir");
    fs::write(
        repo_path.join("repo.toml"),
        format!(
            "id = \"{repo_id}\"\nname = \"Fixture {repo_id}\"\npriority = 0\napi_version = \"getter.repo.v1\"\n"
        ),
    )
    .expect("write repo.toml");
    fs::write(
        repo_path.join(format!("packages/android/{package_name_path}.lua")),
        package_source,
    )
    .expect("write package Lua");

    world.fixture_repo_id = Some(repo_id);
    world.fixture_repo_path = Some(repo_path);
    world.fixture_package_id = Some(package_id);
}

fn run_getter_repo_add_with_priority(world: &mut CliWorld, repo_id: &str, priority: i32) {
    let temp = world.temp.as_ref().expect("tempdir exists");
    let repo_path = temp.path().join(format!("repo-{repo_id}"));
    let output = run_getter(
        world,
        [
            "repo".to_owned(),
            "add".to_owned(),
            repo_id.to_owned(),
            repo_path.to_string_lossy().to_string(),
            "--priority".to_owned(),
            priority.to_string(),
        ],
    );
    world.output = Some(output);
    world.json = None;
}

fn run_getter<I>(world: &CliWorld, command_args: I) -> std::process::Output
where
    I: IntoIterator<Item = String>,
{
    let exe = env!("CARGO_BIN_EXE_getter-cli");
    let data_dir = world.data_dir.as_ref().expect("data dir exists");
    let mut command = Command::new(exe);
    command.arg("--data-dir").arg(data_dir);
    for arg in command_args {
        command.arg(arg);
    }
    command.output().expect("run getter binary")
}

fn assert_success(output: &std::process::Output) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse_stdout(output);
    assert_eq!(json["ok"], true);
}

fn parse_stdout(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout should be JSON: {error}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[tokio::main]
async fn main() {
    let features = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/features/cli");
    CliWorld::cucumber()
        .fail_on_skipped()
        .max_concurrent_scenarios(Some(1))
        .run_and_exit(features)
        .await;
}
