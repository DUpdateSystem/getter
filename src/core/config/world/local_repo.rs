use crate::error::{GetterError, Result};
use std::fs;
use std::path::PathBuf;

static LOCAL_REPO_DIR: &str = "local_repo";

pub struct LocalRepo {
    path: PathBuf,
}

impl LocalRepo {
    pub fn new(root_dir_path: &str) -> Self {
        let path = PathBuf::from(root_dir_path).join(LOCAL_REPO_DIR);
        Self { path }
    }

    pub fn load(&self, rule_path: &str) -> Result<String> {
        let path = PathBuf::from(&self.path).join(rule_path);
        let content = fs::read_to_string(&path)
            .map_err(|e| GetterError::new("LocalRepo", "load", Box::new(e)))?;
        Ok(content)
    }

    pub fn save(&self, rule_path: &str, content: &str) -> Result<()> {
        let path = PathBuf::from(&self.path).join(rule_path);
        let _ = fs::create_dir_all(path.parent().unwrap());
        fs::write(&path, content).map_err(|e| GetterError::new("LocalRepo", "save", Box::new(e)))?;
        Ok(())
    }
}
