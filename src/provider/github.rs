use crate::provider::base_provider::{BaseProvider, IdMap};
use crate::utils::https_get;
use async_trait::async_trait;

pub struct GithubProvider;

#[async_trait]
impl BaseProvider for GithubProvider {
    async fn check_app_available(&self, id_map: &IdMap) -> bool {
        let url = format!("https://github.com/{}/{}", id_map["owner"], id_map["repo"]);

        match url.parse() {
            Ok(parsed_url) => https_get(parsed_url).await.is_ok(),
            Err(_) => false,
        }
    }

    async fn get_releases(&self, id_map: &IdMap) -> Vec<String> {
        vec!["1.0.0".to_string(), "1.0.1".to_string()]
    }
}
