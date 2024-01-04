pub mod convert;
pub mod item;
pub mod local;
pub mod manager;

use self::manager::CacheManager;
use once_cell::sync::Lazy;
use std::sync::{Mutex, MutexGuard};

static CACHE_MANAGER: Lazy<Mutex<Option<CacheManager>>> = Lazy::new(|| Mutex::new(None));

pub fn init_cache_manager(local_cache_path: &str) {
    let mut cache_manager = CACHE_MANAGER.lock().unwrap();
    *cache_manager = Some(CacheManager::new(local_cache_path));
}

pub struct CacheManagerGuard(MutexGuard<'static, Option<CacheManager>>);

impl CacheManagerGuard {
    pub fn get(&mut self) -> &mut CacheManager {
        self.0.as_mut().expect("Cache manager is not initialized")
    }
}

pub fn _get_cache_manager() -> CacheManagerGuard {
    CacheManagerGuard(CACHE_MANAGER.lock().unwrap())
}

#[macro_export]
macro_rules! get_cache_manager {
    () => {{
        $crate::cache::_get_cache_manager().get()
    }};
}
