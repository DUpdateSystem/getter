use crate::{Provider, ReleaseData};
use async_trait::async_trait;
use std::error::Error;

pub struct GitLabProvider;

#[async_trait]
impl Provider for GitLabProvider {
    async fn get_latest_release(&self, repo: &str) -> Result<Option<ReleaseData>, Box<dyn Error>> {
        // TODO: Move GitLab provider implementation here
        Ok(None)
    }
}