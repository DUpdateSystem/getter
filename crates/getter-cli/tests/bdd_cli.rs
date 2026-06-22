use cucumber::{given, then, when, World as _};
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

#[when("I run getter legacy report-list for that directory")]
fn run_getter_legacy_report_list(world: &mut CliWorld) {
    let output = run_getter(world, ["legacy".to_owned(), "report-list".to_owned()]);
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
    assert_eq!(json["command"], "legacy import-room-bundle");
    assert_eq!(json["error"]["code"], "migration.invalid_bundle");
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
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"].as_str() == Some(code.as_str())),
        "diagnostics should contain {code}: {diagnostics:?}"
    );
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
    assert!(report_json.get("bundle_file_name").is_some());
    assert!(
        report_json.get("raw_bundle").is_none(),
        "report must not include raw bundle content"
    );
}

#[then("the import reports one tracked app")]
fn import_reports_one_tracked_app(world: &mut CliWorld) {
    let json = current_json(world);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "legacy import-room-bundle");
    assert_eq!(json["data"]["imported_records"], 1);
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

fn current_json(world: &mut CliWorld) -> &Value {
    world
        .json
        .get_or_insert_with(|| parse_stdout(world.output.as_ref().expect("command output exists")))
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
