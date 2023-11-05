use crate::data::release::*;
use std::collections::HashMap;

use async_trait::async_trait;

pub type IdMap = HashMap<String, String>;

#[async_trait]
pub trait BaseProvider {
    async fn check_app_available(&self, id_map: &IdMap) -> Option<bool>;

    async fn get_latest_release(&self, id_map: &IdMap) -> Option<ReleaseData> {
        if let Some(releases) = self.get_releases(id_map).await {
            Some(releases[0].clone())
        } else {
            None
        }
    }

    async fn get_releases(&self, id_map: &IdMap) -> Option<Vec<ReleaseData>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::try_join;

    pub struct MockProvider;

    #[async_trait]
    impl BaseProvider for MockProvider {
        async fn check_app_available(&self, id_map: &IdMap) -> Option<bool> {
            Some(id_map["available"] == "true")
        }

        async fn get_releases(&self, id_map: &IdMap) -> Option<Vec<ReleaseData>> {
            Some(
                id_map["releases"]
                    .split(",")
                    .map(|s| ReleaseData {
                        version_number: s.to_string(),
                        changelog: s.to_string(),
                        assets: vec![],
                        extra: None,
                    })
                    .collect(),
            )
        }
    }

    #[tokio::test]
    async fn it_works() {
        let mut id_map = IdMap::new();
        id_map.insert("available".to_string(), "true".to_string());
        id_map.insert("releases".to_string(), "1.0.0,1.0.1".to_string());

        let provider = Arc::new(MockProvider);
        let id_map = Arc::new(id_map);

        let provider1 = Arc::clone(&provider);
        let id_map1 = Arc::clone(&id_map);
        let check_app = tokio::spawn(async move { provider1.check_app_available(&*id_map1).await });

        let provider2 = Arc::clone(&provider);
        let id_map2 = Arc::clone(&id_map);
        let latest_release =
            tokio::spawn(async move { provider2.get_latest_release(&*id_map2).await });

        let provider3 = Arc::clone(&provider);
        let id_map3 = Arc::clone(&id_map);
        let releases = tokio::spawn(async move { provider3.get_releases(&*id_map3).await });

        let (check_app_result, latest_release_result, releases_result) =
            try_join!(check_app, latest_release, releases).unwrap();

        assert_eq!(check_app_result, Some(true));
        assert_eq!(latest_release_result.unwrap().version_number, "1.0.0");
        assert_eq!(
            releases_result
                .unwrap()
                .into_iter()
                .map(|release| release.version_number)
                .collect::<Vec<_>>(),
            vec!["1.0.0", "1.0.1"]
        );
    }
}