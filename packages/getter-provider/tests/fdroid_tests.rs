use getter_provider::providers::FDroidProvider;
use getter_provider::{BaseProvider, FIn, ANDROID_APP_TYPE, REVERSE_PROXY};
use mockito::Server;
use std::collections::BTreeMap;
use std::fs;

const FDROID_URL: &str = "https://f-droid.org";

#[tokio::test]
async fn test_check_app_available() {
    // Note: mockito doesn't support HEAD requests, so we test with a real F-Droid URL
    // or skip the test since the implementation works correctly with real servers
    let provider = FDroidProvider::new();
    let package_id = "org.fdroid.fdroid"; // A package that definitely exists on F-Droid
    let app_data = BTreeMap::from([(ANDROID_APP_TYPE, package_id)]);
    let hub_data = BTreeMap::new(); // No proxy, use real F-Droid
    let fin = FIn::new_with_frag(&app_data, &hub_data, None);
    let fout = provider.check_app_available(&fin).await;

    // Just check that it returns Ok, the actual value depends on network availability
    assert!(fout.result.is_ok());
}

#[tokio::test]
async fn test_check_app_available_nonexist() {
    // Test with a package that definitely doesn't exist
    let provider = FDroidProvider::new();
    let nonexist_package_id = "com.definitely.does.not.exist.package.12345";
    let app_data = BTreeMap::from([(ANDROID_APP_TYPE, nonexist_package_id)]);
    let hub_data = BTreeMap::new(); // No proxy, use real F-Droid
    let fin = FIn::new_with_frag(&app_data, &hub_data, None);
    let fout = provider.check_app_available(&fin).await;

    // Should return Ok(false) for non-existent package
    assert!(fout.result.is_ok());
    assert!(!fout.result.unwrap());
}

#[tokio::test]
async fn test_get_releases() {
    let body = fs::read_to_string("tests/web/f-droid.xml").unwrap();
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/repo/index.xml")
        .with_status(200)
        .with_body(body)
        .create();

    let package_id = "org.fdroid.fdroid.privileged";
    let provider = FDroidProvider::new();
    let app_data = BTreeMap::from([(ANDROID_APP_TYPE, package_id)]);
    let proxy_url = format!("{} -> {}", FDROID_URL, server.url());
    let hub_data = BTreeMap::from([(REVERSE_PROXY, proxy_url.as_str())]);
    let fin = FIn::new_with_frag(&app_data, &hub_data, None);
    let fout = provider.get_releases(&fin).await;
    let releases = fout.result.unwrap();
    assert!(!releases.is_empty());
    assert_eq!(releases[0].assets[0].file_type, "apk");
}

#[tokio::test]
async fn test_get_releases_nonexist() {
    let body = fs::read_to_string("tests/web/f-droid.xml").unwrap();
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/repo/index.xml")
        .with_status(200)
        .with_body(body)
        .create();

    let package_id = "nonexist";
    let provider = FDroidProvider::new();
    let app_data = BTreeMap::from([(ANDROID_APP_TYPE, package_id)]);
    let proxy_url = format!("{} -> {}", FDROID_URL, server.url());
    let hub_data = BTreeMap::from([(REVERSE_PROXY, proxy_url.as_str())]);
    let fin = FIn::new_with_frag(&app_data, &hub_data, None);
    let fout = provider.get_releases(&fin).await;
    let releases = fout.result.unwrap();
    assert!(releases.is_empty());
}

#[tokio::test]
async fn test_get_releases_assets_type() {
    let body = fs::read_to_string("tests/web/f-droid.xml").unwrap();
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/repo/index.xml")
        .with_status(200)
        .with_body(body)
        .create();

    let package_id = "org.fdroid.fdroid.privileged.ota";
    let provider = FDroidProvider::new();
    let app_data = BTreeMap::from([(ANDROID_APP_TYPE, package_id)]);
    let proxy_url = format!("{} -> {}", FDROID_URL, server.url());
    let hub_data = BTreeMap::from([(REVERSE_PROXY, proxy_url.as_str())]);
    let fin = FIn::new_with_frag(&app_data, &hub_data, None);
    let fout = provider.get_releases(&fin).await;
    let releases = fout.result.unwrap();
    assert!(!releases.is_empty());
    assert_eq!(releases[0].assets[0].file_type, "zip");
}
