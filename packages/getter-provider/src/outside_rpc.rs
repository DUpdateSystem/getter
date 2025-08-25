use crate::{Provider, ReleaseData};
use async_trait::async_trait;
use std::error::Error;

pub struct OutsideRpcProvider;

#[async_trait]
impl Provider for OutsideRpcProvider {
    async fn get_latest_release(&self, repo: &str) -> Result<Option<ReleaseData>, Box<dyn Error>> {
        // TODO: Move outside RPC provider implementation here
        Ok(None)
    }
}