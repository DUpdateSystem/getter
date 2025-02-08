mod local;
pub mod manager;

use once_cell::sync::Lazy;
use std::{path::Path, sync::Arc};
use tokio::sync::Mutex;

use crate::utils::instance::InstanceContainer;

use self::manager::CacheManager;

static INSTANCE_CONTAINER: Lazy<InstanceContainer<CacheManager>> =
    Lazy::new(|| InstanceContainer::new(CacheManager::new()));

pub async fn init_cache_manager(local_cache_dir: &Path) {
    get_cache_manager()
        .await
        .lock()
        .await
        .set_local_cache_dir(local_cache_dir);
}

pub async fn init_cache_manager_with_expire(local_cache_path: &Path, expire_time: u64) {
    get_cache_manager()
        .await
        .lock()
        .await
        .set_local_cache_dir(local_cache_path)
        .set_global_expire_time(expire_time);
}

pub async fn get_cache_manager() -> Arc<Mutex<CacheManager>> {
    INSTANCE_CONTAINER.get().await.clone()
}
