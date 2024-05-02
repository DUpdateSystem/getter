use std::io::Result;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio::fs::{metadata, read, remove_file, write};

use tokio::fs::create_dir_all;

pub struct LocalCacheItem {
    cache_path: PathBuf,
}

impl LocalCacheItem {
    pub fn new(cache_dir: &Path, key: &str) -> Self {
        Self {
            cache_path: cache_dir.join(key),
        }
    }

    pub async fn get<T>(&self, decoder: fn(Vec<u8>) -> T) -> Result<T> {
        read(&self.cache_path).await.map(|data| decoder(data))
    }

    pub async fn save<T>(&self, data: T, encoder: fn(T) -> Vec<u8>) -> Result<()> {
        let parent = self.cache_path.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "cache path not found")
        })?;
        create_dir_all(parent).await?;
        write(&self.cache_path, encoder(data)).await
    }

    pub async fn remove(&self) -> Result<()> {
        remove_file(&self.cache_path).await
    }

    pub async fn get_cache_time(&self) -> Result<u64> {
        let metadata = metadata(&self.cache_path).await?;
        let modified_time = metadata.modified()?;
        Ok(modified_time
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[tokio::test]
    async fn test_local_cache_item() {
        let key = "test_key";
        let data = "test_data";
        let cache_path = Path::new("./test_cache");
        let cache_item = LocalCacheItem::new(cache_path, key);
        cache_item
            .save(data, |data| data.as_bytes().to_vec())
            .await
            .unwrap();
        let result = cache_item
            .get(|data| String::from_utf8(data).unwrap())
            .await
            .unwrap();
        assert_eq!(result, data);
        let cache_time = cache_item.get_cache_time().await.unwrap();
        assert!(
            cache_time
                >= SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
        );
        cache_item.remove().await.unwrap();
        fs::remove_dir_all(cache_path).unwrap();
    }
}
