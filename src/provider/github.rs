use async_trait::async_trait;
use crate::provider::base_provider::BaseProvider;

pub struct GithubProvider;

#[async_trait]
impl BaseProvider for GithubProvider {
    async fn check_app_available(&self) -> bool {
        true
    }

    async fn get_releases(&self) -> Vec<String> {
        vec!["1.0.0".to_string(), "1.0.1".to_string()]
    }
}
