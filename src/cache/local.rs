use std::fs::create_dir_all;
use std::io::Result;
use std::path::PathBuf;
use std::time::SystemTime;

pub struct LocalCacheItem {
    cache_path: PathBuf,
}

impl LocalCacheItem {
    pub fn new(cache_path: &str) -> Self {
        Self {
            cache_path: PathBuf::from(cache_path),
        }
    }

    pub async fn get<T>(&self, key: &str, decoder: fn(Vec<u8>) -> T) -> Result<T> {
        let file = self.cache_path.join(key);
        tokio::fs::read(&file).await.map(|data| decoder(data))
    }

    pub async fn save<T>(&self, key: &str, data: T, encoder: fn(T) -> Vec<u8>) -> Result<()> {
        create_dir_all(&self.cache_path)?;
        let file = self.cache_path.join(key);
        tokio::fs::write(&file, encoder(data)).await
    }

    pub async fn remove(&self, key: &str) -> Result<()> {
        let file = self.cache_path.join(key);
        tokio::fs::remove_file(&file).await
    }

    pub async fn clean(&self) -> Result<()> {
        tokio::fs::remove_dir_all(&self.cache_path).await
    }

    pub async fn get_cache_time(&self, key: &str) -> Result<u64> {
        let file = self.cache_path.join(key);
        let metadata = tokio::fs::metadata(&file).await?;
        let modified_time = metadata.modified()?;
        Ok(modified_time
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_cache_item() {
        let cache_item = LocalCacheItem::new("./test_cache");
        let key = "test_key";
        let data = "test_data";
        cache_item
            .save(key, data, |data| data.as_bytes().to_vec())
            .await
            .unwrap();
        let result = cache_item
            .get(key, |data| String::from_utf8(data).unwrap())
            .await
            .unwrap();
        assert_eq!(result, data);
        let cache_time = cache_item.get_cache_time(key).await.unwrap();
        assert!(
            cache_time
                >= SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
        );
        cache_item.remove(key).await.unwrap();
        cache_item.clean().await.unwrap();
    }
}
