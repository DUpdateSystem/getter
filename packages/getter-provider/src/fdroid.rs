use crate::{Provider, ReleaseData};
use async_trait::async_trait;
use std::error::Error;

pub struct FDroidProvider;

#[async_trait]
impl Provider for FDroidProvider {
    async fn get_latest_release(&self, repo: &str) -> Result<Option<ReleaseData>, Box<dyn Error>> {
        // TODO: Move F-Droid provider implementation here
        Ok(None)
    }
}