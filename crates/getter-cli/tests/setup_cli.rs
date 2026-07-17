use getter_cli::{run, ExitCode};
use serde_json::{json, Value};
use std::fs;

#[test]
fn setup_preview_uses_product_safe_inventory_input_and_offline_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let inventory = temp.path().join("installed.json");
    fs::write(&inventory, json!({
        "format": "upgradeall-installed-inventory", "version": 1,
        "items": [{"kind":"android_package","package_name":"com.example.app","label":"Example","version_name":"1.0","version_code":1}]
    }).to_string()).unwrap();

    let output = run([
        "getter",
        "--data-dir",
        temp.path().to_str().unwrap(),
        "setup",
        "preview",
        "--inventory",
        inventory.to_str().unwrap(),
    ]);
    assert_eq!(output.exit_code, ExitCode::Success, "{}", output.stdout);
    let envelope: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(envelope["command"], "setup preview");
    assert_eq!(
        envelope["data"]["candidates"][0]["category"],
        "installed_fallback"
    );
    assert!(envelope["data"]["preview_id"]
        .as_str()
        .is_some_and(|value| value.len() == 64));
}

#[test]
fn setup_preview_envelope_can_be_applied_without_manual_json_extraction() {
    let temp = tempfile::tempdir().unwrap();
    let inventory = temp.path().join("installed.json");
    let preview = temp.path().join("preview.json");
    fs::write(
        &inventory,
        json!({
            "format": "upgradeall-installed-inventory",
            "version": 1,
            "items": [{
                "kind": "android_package",
                "package_name": "com.example.app",
                "label": "Example",
                "version_name": "1.0",
                "version_code": 1
            }]
        })
        .to_string(),
    )
    .unwrap();

    let preview_output = run([
        "getter",
        "--data-dir",
        temp.path().to_str().unwrap(),
        "setup",
        "preview",
        "--inventory",
        inventory.to_str().unwrap(),
    ]);
    assert_eq!(preview_output.exit_code, ExitCode::Success);
    fs::write(&preview, &preview_output.stdout).unwrap();

    let apply_output = run([
        "getter",
        "--data-dir",
        temp.path().to_str().unwrap(),
        "setup",
        "apply",
        "--preview",
        preview.to_str().unwrap(),
        "--accept-all",
    ]);
    assert_eq!(
        apply_output.exit_code,
        ExitCode::Success,
        "{}",
        apply_output.stdout
    );
    let envelope: Value = serde_json::from_str(&apply_output.stdout).unwrap();
    assert_eq!(
        envelope["data"]["applied_package_ids"][0],
        "android/app/com.example.app"
    );
    assert!(temp
        .path()
        .join("repo/autogen/android/app/com.example.app/.autogen.jsonc")
        .is_file());
}

#[test]
fn setup_request_rejects_provider_controls() {
    let temp = tempfile::tempdir().unwrap();
    let output = run([
        "getter",
        "--data-dir",
        temp.path().to_str().unwrap(),
        "setup",
        "preview",
        "--endpoint",
        "https://evil.invalid/index.xml",
    ]);
    assert_eq!(output.exit_code, ExitCode::Usage);
}
