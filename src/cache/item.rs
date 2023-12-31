use bytes::Bytes;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
pub struct CacheItem {
    created_at: u64,
    data: Bytes,
}

impl CacheItem {
    pub fn new(data: Bytes) -> Self {
        Self {
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_else(|_| Duration::from_secs(0))
                .as_secs(),
            data,
        }
    }

    pub fn get_data(&self) -> &Bytes {
        &self.data
    }

    pub fn check_expire(&self, duration_time: u64) -> bool {
        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_secs();

        now_unix >= self.created_at + duration_time
    }
}
