use crate::CacheManager;

pub struct CacheManagerBuilder;

impl CacheManagerBuilder {
    #[cfg(feature = "concurrent")]
    pub fn new_concurrent() -> CacheManager {
        use crate::concurrent::ConcurrentCache;
        CacheManager::new(Box::new(ConcurrentCache::new()))
    }

    pub fn new_local() -> CacheManager {
        use crate::local::LocalCache;
        CacheManager::new(Box::new(LocalCache::new()))
    }
}
