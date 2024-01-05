use std::env;
use std::io::{self, ErrorKind};
use std::path::PathBuf;

#[cfg(all(target_family = "unix", not(target_os = "macos"), not(target_os = "android")))]
fn cache_dir() -> Result<PathBuf, io::Error> {
    env::var("HOME")
        .map_err(|_| io::Error::new(ErrorKind::NotFound, "HOME not found"))
        .map(|home| PathBuf::from(home).join(".cache/upa/"))
}

#[cfg(target_os = "macos")]
fn cache_dir() -> Result<PathBuf, io::Error> {
    env::var("HOME")
        .map_err(|_| io::Error::new(ErrorKind::NotFound, "HOME not found"))
        .map(|home| PathBuf::from(home).join("Library/Caches/upa/"))
}

#[cfg(target_family = "windows")]
fn cache_dir() -> Result<PathBuf, io::Error> {
    env::var("APPDATA")
        .map_err(|_| io::Error::new(ErrorKind::NotFound, "APPDATA not found"))
        .map(|app_data| PathBuf::from(app_data).join("upa/cache/"))
}

#[cfg(target_os = "android")]
fn cache_dir() -> Result<PathBuf, io::Error> {
    env::var("ANDROID_DATA")
        .map_err(|_| io::Error::new(ErrorKind::NotFound, "ANDROID_DATA not found"))
        .map(|data_dir| PathBuf::from(data_dir).join("cache/upa/"))
}
