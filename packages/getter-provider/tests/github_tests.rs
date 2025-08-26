use getter_provider::data::ReleaseData;
use getter_provider::providers::GitHubProvider;
use getter_provider::{BaseProvider, FIn, REVERSE_PROXY};
use mockito::Server;
use std::collections::BTreeMap;
use std::fs;

const GITHUB_URL: &str = "https://github.com";
const GITHUB_API_URL: &str = "https://api.github.com";

#[tokio::test]
async fn test_check_app_available() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("HEAD", "/DUpdateSystem/UpgradeAll")
        .with_status(200)
        .create_async()
        .await;

    let id_map = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
    let proxy_url = format!("{} -> {}", GITHUB_URL, server.url());
    let hub_data = BTreeMap::from([(REVERSE_PROXY, proxy_url.as_str())]);

    let github_provider = GitHubProvider::new();
    let result = github_provider
        .check_app_available(&FIn::new_with_frag(&id_map, &hub_data, None))
        .await;

    assert!(result.result.is_ok());
    assert!(result.result.unwrap());
}

#[tokio::test]
async fn test_get_releases() {
    let body = fs::read_to_string("tests/web/github_api_release.json").unwrap();
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/repos/DUpdateSystem/UpgradeAll/releases")
        .with_status(200)
        .with_body(body)
        .create();

    let id_map = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
    let proxy_url = format!("{} -> {}", GITHUB_API_URL, server.url());
    let hub_data = BTreeMap::from([(REVERSE_PROXY, proxy_url.as_str())]);

    let github_provider = GitHubProvider::new();
    let result = github_provider
        .get_releases(&FIn::new_with_frag(&id_map, &hub_data, None))
        .await;

    assert!(result.result.is_ok());
    let releases = result.result.unwrap();

    let release_json = fs::read_to_string("tests/data/provider_github_release.json").unwrap();
    let releases_saved = serde_json::from_str::<Vec<ReleaseData>>(&release_json).unwrap();
    assert_eq!(releases, releases_saved);
}

#[tokio::test]
async fn test_get_releases_token() {
    let body = fs::read_to_string("tests/web/github_api_release.json").unwrap();
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/repos/DUpdateSystem/UpgradeAll/releases")
        .match_header("authorization", "Bearer test_token")
        .with_status(200)
        .with_body(body)
        .create_async()
        .await;

    let id_map = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
    let proxy_url = format!("{} -> {}", GITHUB_API_URL, server.url());
    let hub_data = BTreeMap::from([(REVERSE_PROXY, proxy_url.as_str()), ("token", "test_token")]);

    let github_provider = GitHubProvider::new();
    let result = github_provider
        .get_releases(&FIn::new_with_frag(&id_map, &hub_data, None))
        .await;

    assert!(result.result.is_ok());
    let releases = result.result.unwrap();
    assert!(!releases.is_empty());

    // Test with token in app_data instead
    let mut id_map_with_token = id_map.clone();
    id_map_with_token.insert("token", "test_token");
    let hub_data_no_token = BTreeMap::from([
        (REVERSE_PROXY, proxy_url.as_str()),
        ("token", "   "), // Empty token
    ]);

    let result2 = github_provider
        .get_releases(&FIn::new_with_frag(
            &id_map_with_token,
            &hub_data_no_token,
            None,
        ))
        .await;

    assert!(result2.result.is_ok());
    let releases2 = result2.result.unwrap();
    assert!(!releases2.is_empty());
}
