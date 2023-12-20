use bytes::Bytes;
pub struct CacheItem {
    created_at: i64,
}

impl CacheItem {
    pub fn get_data(&self) -> Bytes {
        Bytes::new()
    }
}

pub struct CacheItemBuilder;

impl CacheItemBuilder {
    pub fn new() -> Self {
        Self {}
    }

    pub fn build(&self) -> CacheItem {
        CacheItem { created_at: 0 }
    }
}
