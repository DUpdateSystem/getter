use std::env;
use std::io::{self, ErrorKind};
use std::path::PathBuf;

pub struct DataDir{
    pub cache_dir: PathBuf,
    pub data_dir: PathBuf,
}

#[cfg(all(target_family = "unix", not(target_os = "macos"), not(target_os = "android")))]
pub fn all_dir() -> Result<DataDir, io::Error> {
    let home_dir = env::var("HOME")
        .map_err(|_| io::Error::new(ErrorKind::NotFound, "HOME not found"))
        .map(|home| PathBuf::from(home))?;
    let cache_dir = home_dir.join(".cache/upa/");
    let data_dir = home_dir.join(".local/share/upa/");
    Ok(DataDir{cache_dir, data_dir})
}

#[cfg(target_os = "macos")]
pub fn all_dir() -> Result<DataDir, io::Error> {
    let home_dir = env::var("HOME")
        .map_err(|_| io::Error::new(ErrorKind::NotFound, "HOME not found"))
        .map(|home| PathBuf::from(home))?;
    let cache_dir = home_dir.join("Library/Caches/upa/");
    let data_dir = home_dir.join("Library/Application Support/upa/");
    Ok(DataDir{cache_dir, data_dir})
}

#[cfg(target_family = "windows")]
pub fn all_dir() -> Result<DataDir, io::Error> {
    let home_dir = env::var("APPDATA")
        .map_err(|_| io::Error::new(ErrorKind::NotFound, "APPDATA not found"))
        .map(|home| PathBuf::from(home))?;
    let cache_dir = home_dir.join("upa/cache/");
    let data_dir = home_dir.join("upa/data/");
    Ok(DataDir{cache_dir, data_dir})
}


#[cfg(target_os = "android")]
pub fn all_dir() -> Result<DataDir, io::Error> {
    let home_dir = env::var("HOME")
        .map_err(|_| io::Error::new(ErrorKind::NotFound, "HOME not found"))
        .map(|home| PathBuf::from(home))?;
    let cache_dir = home_dir.join(".upa/cache/");
    let data_dir = home_dir.join(".upa/data/");
    Ok(DataDir{cache_dir, data_dir})
}
