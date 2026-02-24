use file_locker::FileLock;
use jsonl::ReadError;
use serde::{de::DeserializeOwned, Serialize};
use std::io::{BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

pub trait HasId {
    fn id(&self) -> &str;
}

/// A simple JSONL-backed persistent store for a single record type.
///
/// Each line in the file is a JSON-serialized record. All mutating operations
/// use `file-locker` advisory locking to ensure safe concurrent access.
pub struct JsonlStore {
    path: PathBuf,
}

impl JsonlStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Ensure the backing file exists (creates parent dirs as needed).
    pub fn ensure_file(&self) -> Result<()> {
        if !self.path.exists() {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::File::create(&self.path)?;
        }
        Ok(())
    }

    /// Read all records from the file line by line.
    pub fn load_all<T: DeserializeOwned>(&self) -> Result<Vec<T>> {
        if !self.path.exists() {
            return Ok(vec![]);
        }
        let file = std::fs::File::open(&self.path)?;
        let reader = BufReader::new(file);
        read_all_from_reader(reader)
    }

    fn acquire_write_lock(&self) -> Result<FileLock> {
        self.ensure_file()?;
        FileLock::new(&self.path)
            .blocking(true)
            .writeable(true)
            .lock()
            .map_err(Error::Io)
    }

    /// Overwrite the entire file with the given records (must already hold lock).
    fn write_all_locked<T: Serialize>(lock: &mut FileLock, records: &[T]) -> Result<()> {
        lock.file.set_len(0)?;
        lock.file.seek(SeekFrom::Start(0))?;
        for record in records {
            jsonl::write(&mut lock.file, record)
                .map_err(|e| Error::Other(format!("jsonl write: {e}")))?;
        }
        lock.file.flush()?;
        Ok(())
    }

    /// Insert or replace a record (matched by id).
    pub fn upsert<T: Serialize + DeserializeOwned + HasId>(&self, record: &T) -> Result<()> {
        let mut lock = self.acquire_write_lock()?;
        let mut records: Vec<T> = {
            lock.file.seek(SeekFrom::Start(0))?;
            read_all_from_reader(BufReader::new(&lock.file))?
        };

        let id = record.id();
        let serialized: T = serde_json::from_str(
            &serde_json::to_string(record).map_err(|e| Error::Other(format!("serialize: {e}")))?,
        )
        .map_err(|e| Error::Other(format!("deserialize: {e}")))?;

        if let Some(pos) = records.iter().position(|r| r.id() == id) {
            records[pos] = serialized;
        } else {
            records.push(serialized);
        }

        Self::write_all_locked(&mut lock, &records)
    }

    /// Delete the record with the given id. Returns true if a record was removed.
    pub fn delete<T: Serialize + DeserializeOwned + HasId>(&self, id: &str) -> Result<bool> {
        let mut lock = self.acquire_write_lock()?;
        let records: Vec<T> = {
            lock.file.seek(SeekFrom::Start(0))?;
            read_all_from_reader(BufReader::new(&lock.file))?
        };

        let original_len = records.len();
        let filtered: Vec<T> = records.into_iter().filter(|r| r.id() != id).collect();
        let deleted = filtered.len() < original_len;
        Self::write_all_locked(&mut lock, &filtered)?;
        Ok(deleted)
    }

    /// Find a record by id (read-only, no lock).
    pub fn find_by_id<T: DeserializeOwned + HasId>(&self, id: &str) -> Result<Option<T>> {
        Ok(self.load_all::<T>()?.into_iter().find(|r| r.id() == id))
    }
}

fn read_all_from_reader<R: std::io::BufRead, T: DeserializeOwned>(mut reader: R) -> Result<Vec<T>> {
    let mut records = Vec::new();
    loop {
        match jsonl::read::<_, T>(&mut reader) {
            Ok(record) => records.push(record),
            Err(ReadError::Eof) => break,
            Err(ReadError::Deserialize(e)) => {
                eprintln!("JsonlStore: skipping malformed line: {e}");
            }
            Err(ReadError::Io(e)) => return Err(Error::Io(e)),
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use tempfile::NamedTempFile;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestRecord {
        id: String,
        value: String,
    }

    impl HasId for TestRecord {
        fn id(&self) -> &str {
            &self.id
        }
    }

    fn make_store() -> (JsonlStore, NamedTempFile) {
        let tmp = NamedTempFile::new().unwrap();
        let store = JsonlStore::new(tmp.path());
        (store, tmp)
    }

    #[test]
    fn test_empty_load() {
        let (store, _tmp) = make_store();
        let records: Vec<TestRecord> = store.load_all().unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn test_upsert_and_load() {
        let (store, _tmp) = make_store();
        let r = TestRecord {
            id: "1".to_string(),
            value: "hello".to_string(),
        };
        store.upsert(&r).unwrap();
        let records: Vec<TestRecord> = store.load_all().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0], r);
    }

    #[test]
    fn test_upsert_updates_existing() {
        let (store, _tmp) = make_store();
        store
            .upsert(&TestRecord {
                id: "1".to_string(),
                value: "old".to_string(),
            })
            .unwrap();
        store
            .upsert(&TestRecord {
                id: "1".to_string(),
                value: "new".to_string(),
            })
            .unwrap();
        let records: Vec<TestRecord> = store.load_all().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].value, "new");
    }

    #[test]
    fn test_multiple_records() {
        let (store, _tmp) = make_store();
        for i in 0..5 {
            store
                .upsert(&TestRecord {
                    id: i.to_string(),
                    value: format!("v{i}"),
                })
                .unwrap();
        }
        let records: Vec<TestRecord> = store.load_all().unwrap();
        assert_eq!(records.len(), 5);
    }

    #[test]
    fn test_delete() {
        let (store, _tmp) = make_store();
        store
            .upsert(&TestRecord {
                id: "1".to_string(),
                value: "a".to_string(),
            })
            .unwrap();
        store
            .upsert(&TestRecord {
                id: "2".to_string(),
                value: "b".to_string(),
            })
            .unwrap();
        let deleted = store.delete::<TestRecord>("1").unwrap();
        assert!(deleted);
        let records: Vec<TestRecord> = store.load_all().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "2");
    }

    #[test]
    fn test_delete_nonexistent() {
        let (store, _tmp) = make_store();
        let deleted = store.delete::<TestRecord>("nope").unwrap();
        assert!(!deleted);
    }

    #[test]
    fn test_find_by_id() {
        let (store, _tmp) = make_store();
        store
            .upsert(&TestRecord {
                id: "42".to_string(),
                value: "answer".to_string(),
            })
            .unwrap();
        let found: Option<TestRecord> = store.find_by_id("42").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().value, "answer");
    }

    #[test]
    fn test_find_by_id_missing() {
        let (store, _tmp) = make_store();
        let found: Option<TestRecord> = store.find_by_id("999").unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn test_file_not_exist_returns_empty() {
        let store = JsonlStore::new("/tmp/this_file_does_not_exist_upgradeall.jsonl");
        let records: Vec<TestRecord> = store.load_all().unwrap();
        assert!(records.is_empty());
    }
}
