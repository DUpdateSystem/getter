use crate::{Provider, ReleaseData};
use async_trait::async_trait;
use std::error::Error;

pub struct LSPosedRepoProvider;

#[async_trait]
impl Provider for LSPosedRepoProvider {
    async fn get_latest_release(&self, repo: &str) -> Result<Option<ReleaseData>, Box<dyn Error>> {
        // TODO: Move LSPosed repo provider implementation here
        Ok(None)
    }
}