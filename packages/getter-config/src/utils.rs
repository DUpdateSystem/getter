use std::{env, io, path::PathBuf};

pub struct DataDir {
    pub cache_dir: PathBuf,
    pub data_dir: PathBuf,
}

#[cfg(all(
    target_family = "unix",
    not(target_os = "macos"),
    not(target_os = "android")
))]
pub fn all_dir() -> Result<DataDir, io::Error> {
    let home_dir = env::var("HOME")
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "HOME not found"))
        .map(PathBuf::from)?;
    let cache_dir = home_dir.join(".cache/upa/");
    let data_dir = home_dir.join(".local/share/upa/");
    Ok(DataDir {
        cache_dir,
        data_dir,
    })
}

#[cfg(target_os = "macos")]
pub fn all_dir() -> Result<DataDir, io::Error> {
    let home_dir = env::var("HOME")
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "HOME not found"))
        .map(|home| PathBuf::from(home))?;
    let cache_dir = home_dir.join("Library/Caches/upa/");
    let data_dir = home_dir.join("Library/Application Support/upa/");
    Ok(DataDir {
        cache_dir,
        data_dir,
    })
}

#[cfg(target_family = "windows")]
pub fn all_dir() -> Result<DataDir, io::Error> {
    let home_dir = env::var("APPDATA")
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "APPDATA not found"))
        .map(|home| PathBuf::from(home))?;
    let cache_dir = home_dir.join("upa/cache/");
    let data_dir = home_dir.join("upa/data/");
    Ok(DataDir {
        cache_dir,
        data_dir,
    })
}

#[cfg(target_os = "android")]
pub fn all_dir() -> Result<DataDir, io::Error> {
    let home_dir = env::var("HOME")
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "HOME not found"))
        .map(|home| PathBuf::from(home))?;
    let cache_dir = home_dir.join(".upa/cache/");
    let data_dir = home_dir.join(".upa/data/");
    Ok(DataDir {
        cache_dir,
        data_dir,
    })
}

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
