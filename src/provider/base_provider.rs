use async_trait::async_trait;

#[async_trait]
pub trait BaseProvider {
    async fn check_app_available(&self) -> bool;

    async fn get_latest_release(&self) -> String {
        let releases = self.get_releases().await;
        releases[0].clone()
    }

    async fn get_releases(&self) -> Vec<String>;
}
