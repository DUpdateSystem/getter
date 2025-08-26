use getter_provider::data::ReleaseData;
use getter_provider::providers::FDroidProvider;
use getter_provider::{BaseProvider, FIn, ANDROID_APP_TYPE, REVERSE_PROXY};
use mockito::Server;
use std::collections::BTreeMap;
use std::fs;

const FDROID_URL: &str = "https://f-droid.org";

#[tokio::test]
async fn test_check_app_available() {
    let package_id = "com.termux";
    let mut server = Server::new_async().await;
    let _m = server
        .mock("HEAD", format!("/packages/{}", package_id).as_str())
        .with_status(200)
        .create_async()
        .await;

    let provider = FDroidProvider::new();
    let app_data = BTreeMap::from([(ANDROID_APP_TYPE, package_id)]);
    let proxy_url = format!("{} -> {}", FDROID_URL, server.url());
    let hub_data = BTreeMap::from([(REVERSE_PROXY, proxy_url.as_str())]);
    let fin = FIn::new_with_frag(&app_data, &hub_data, None);
    let fout = provider.check_app_available(&fin).await;

    assert!(fout.result.is_ok());
    // The test should pass since we mocked 200 status
    assert!(fout.result.unwrap());
}

#[tokio::test]
async fn test_check_app_available_nonexist() {
    let package_id = "com.termux";
    let mut server = Server::new_async().await;
    let _m = server
        .mock("HEAD", format!("/packages/{}", package_id).as_str())
        .with_status(404)
        .create_async()
        .await;

    let provider = FDroidProvider::new();
    let nonexist_package_id = "nonexist";
    let app_data = BTreeMap::from([(ANDROID_APP_TYPE, nonexist_package_id)]);
    let proxy_url = format!("{} -> {}", FDROID_URL, server.url());
    let hub_data = BTreeMap::from([(REVERSE_PROXY, proxy_url.as_str())]);
    let fin = FIn::new_with_frag(&app_data, &hub_data, None);
    let fout = provider.check_app_available(&fin).await;

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
