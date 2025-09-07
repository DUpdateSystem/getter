use getter_provider::data::ReleaseData;
use getter_provider::providers::LsposedRepoProvider;
use getter_provider::{BaseProvider, FIn, ANDROID_APP_TYPE, REVERSE_PROXY};
use mockito::Server;
use std::collections::BTreeMap;
use std::fs;

const LSPOSED_REPO_API_URL: &str = "https://modules.lsposed.org/modules.json";

#[tokio::test]
async fn test_check_app_available() {
    let body = fs::read_to_string("tests/web/lsposed_modules.json").unwrap();
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/modules.json")
        .with_status(200)
        .with_body(body)
        .create_async()
        .await;

    let id_map = BTreeMap::from([(ANDROID_APP_TYPE, "com.agoines.relaxhelp")]);
    let mock_url = format!("{}/modules.json", server.url());
    let proxy_url = format!("{} -> {}", LSPOSED_REPO_API_URL, mock_url);
    let hub_data = BTreeMap::from([(REVERSE_PROXY, proxy_url.as_str())]);

    let lsposed_provider = LsposedRepoProvider::new();
    let result = lsposed_provider
        .check_app_available(&FIn::new_with_frag(&id_map, &hub_data, None))
        .await;

    assert!(result.result.is_ok());
    assert!(result.result.unwrap());
}

#[tokio::test]
async fn test_get_releases() {
    let body = fs::read_to_string("tests/web/lsposed_modules.json").unwrap();
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/modules.json")
        .with_status(200)
        .with_body(body)
        .create_async()
        .await;

    let id_map = BTreeMap::from([(ANDROID_APP_TYPE, "com.agoines.relaxhelp")]);
    let mock_url = format!("{}/modules.json", server.url());
    let proxy_url = format!("{} -> {}", LSPOSED_REPO_API_URL, mock_url);
    let hub_data = BTreeMap::from([(REVERSE_PROXY, proxy_url.as_str())]);

    let lsposed_provider = LsposedRepoProvider::new();
    let result = lsposed_provider
        .get_releases(&FIn::new_with_frag(&id_map, &hub_data, None))
        .await;

    assert!(result.result.is_ok());
    let releases = result.result.unwrap();

    let release_json = fs::read_to_string("tests/data/provider_lsposed_releases.json").unwrap();
    let releases_saved = serde_json::from_str::<Vec<ReleaseData>>(&release_json).unwrap();
    assert_eq!(releases, releases_saved);
}
