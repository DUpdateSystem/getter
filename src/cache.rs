pub mod convert;
mod local;
pub mod manager;

use once_cell::sync::Lazy;

use crate::utils::instance::{InstanceContainer, InstanceGuard};

use self::manager::CacheManager;

static INSTANCE_CONTAINER: Lazy<InstanceContainer<CacheManager>> =
    Lazy::new(|| InstanceContainer::new());

pub fn init_cache_manager(local_cache_path: &str) {
    INSTANCE_CONTAINER.init(CacheManager::new(local_cache_path, None));
}

pub fn init_cache_manager_with_expire(local_cache_path: &str, expire_time: u64) {
    INSTANCE_CONTAINER.init(CacheManager::new(local_cache_path, Some(expire_time)));
}

pub fn _get<'a>() -> InstanceGuard<'a, CacheManager> {
    INSTANCE_CONTAINER.get()
}

#[macro_export]
macro_rules! get_cache_manager {
    () => {{
        $crate::cache::_get()
            .get()
            .expect("Cache manager is not initialized")
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_init_cache_manager() {
        let local_cache_path = "./test_init_cache_manager";
        init_cache_manager(local_cache_path);
        let _ = get_cache_manager!();
    }
}
