use super::super::data::release::*;
use super::base_provider::*;
use crate::rpc::client::*;
use async_trait::async_trait;

pub struct OutsideProvider {
    pub uuid: String,
    pub url: String,
}

#[async_trait]
impl BaseProvider for OutsideProvider {
    fn get_cache_request_key(
        &self,
        _function_type: &FunctionType,
        _data_map: &DataMap,
    ) -> Vec<String> {
        vec![]
    }

    async fn check_app_available(&self, fin: &FIn) -> FOut<bool> {
        FOut {
            result: match Client::new(&self.url).map(|client| async move {
                client
                    .check_app_available(
                        &self.uuid,
                        fin.data_map.app_data.to_owned(),
                        fin.data_map.hub_data.to_owned(),
                    )
                    .await
            }) {
                Ok(result) => match result.await {
                    Ok(result) => Ok(result),
                    Err(e) => Err(Box::new(e)),
                },
                Err(e) => Err(Box::new(e)),
            },
            cached_map: None,
        }
    }

    async fn get_latest_release(&self, fin: &FIn) -> FOut<ReleaseData> {
        FOut {
            result: match Client::new(&self.url).map(|client| async move {
                client
                    .get_latest_release(
                        &self.uuid,
                        fin.data_map.app_data.to_owned(),
                        fin.data_map.hub_data.to_owned(),
                    )
                    .await
            }) {
                Ok(result) => match result.await {
                    Ok(result) => Ok(result),
                    Err(e) => Err(Box::new(e)),
                },
                Err(e) => Err(Box::new(e)),
            },
            cached_map: None,
        }
    }

    async fn get_releases(&self, fin: &FIn) -> FOut<Vec<ReleaseData>> {
        FOut {
            result: match Client::new(&self.url).map(|client| async move {
                client
                    .get_releases(
                        &self.uuid,
                        fin.data_map.app_data.to_owned(),
                        fin.data_map.hub_data.to_owned(),
                    )
                    .await
            }) {
                Ok(result) => match result.await {
                    Ok(result) => Ok(result),
                    Err(e) => Err(Box::new(e)),
                },
                Err(e) => Err(Box::new(e)),
            },
            cached_map: None,
        }
    }
}

#[cfg(test)]
mod tests {}
