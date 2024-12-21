use std::{env, path::PathBuf};

use crate::locale::all_dir;

pub fn get_data_path(sub: &str) -> String {
    let data_dir = env::var("DATA_DIR").map(PathBuf::from).unwrap_or_else(|_| {
        all_dir()
            .expect("Non-support OS, you should set DATA_DIR env arg")
            .data_dir
    });
    data_dir
        .join(sub)
        .to_str()
        .expect("Invalid config path")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;

    #[test]
    fn test_get_data_path_env() {
        let path = "/tmp/getter_test";
        env::set_var("DATA_DIR", path);
        let path = PathBuf::from(get_data_path("test"));
        let _ = fs::remove_file(&path);
        let path = path.to_str().unwrap();
        assert_eq!(path, "/tmp/getter_test/test");
    }

    #[test]
    fn test_get_data_path_default() {
        let path = PathBuf::from(get_data_path("test"));
        let _ = fs::remove_file(&path);
        let path = path.to_str().unwrap();
        assert!(path.ends_with("test"))
    }
}
