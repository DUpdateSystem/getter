mod local;
pub mod manager;
pub mod convert;

use once_cell::sync::Lazy;

use crate::utils::instance::{InstanceContainer, InstanceGuard};

use self::manager::CacheManager;

static INSTANCE_CONTAINER: Lazy<InstanceContainer<CacheManager>> = Lazy::new(|| InstanceContainer::new());

pub fn init_cache_manager(local_cache_path: &str) {
    INSTANCE_CONTAINER.init(CacheManager::new(local_cache_path));
}

pub fn _get<'a>() -> InstanceGuard<'a ,CacheManager> {
    INSTANCE_CONTAINER.get()
}

#[macro_export]
macro_rules! get_cache_manager {
    () => {{
        $crate::cache::_get().get().expect("Cache manager is not initialized")
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_manager() {
        let local_cache_path = "./test_cache_manager";
        init_cache_manager(local_cache_path);
        let _ = get_cache_manager!();
    }
}
