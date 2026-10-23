//! Shared, UI-free contracts for LumaKit desktop applications.
pub mod sqlite;
pub mod secrets;
pub use secrets::new_id;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SupportError {
    #[error("invalid app slug")]
    InvalidAppSlug,
    #[error("path operation failed")]
    Io(#[from] std::io::Error),
    #[error("settings serialization failed")]
    Json(#[from] serde_json::Error),
    #[error("settings version {0} is newer than this application")]
    FutureVersion(u32),
    #[error("another process is using this data directory")]
    AlreadyOpen,
}

pub fn data_root(app_slug: &str) -> Result<PathBuf, SupportError> {
    if app_slug.is_empty()
        || app_slug.len() > 32
        || !app_slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(SupportError::InvalidAppSlug);
    }
    portable_data_root()
}

/// 可写目录必须明确；静默回退会让用户误以为历史丢失。
pub fn portable_data_root() -> Result<PathBuf, SupportError> {
    let root = std::env::current_exe()?.parent().ok_or(SupportError::InvalidAppSlug)?.join("data");
    fs::create_dir_all(&root)?;
    let probe = root.join(format!(".write-probe-{}", new_id().map_err(std::io::Error::other)?));
    File::create(&probe)?.sync_all()?;
    fs::remove_file(probe)?;
    Ok(root)
}

/// 锁文件可残留；是否占用由内核句柄锁决定，不靠文件是否存在。
#[derive(Debug)]
pub struct FileLock { _file: File }
impl FileLock {
    pub fn acquire(path: &Path) -> Result<Self, SupportError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) { fs::create_dir_all(parent)?; }
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
        file.try_lock().map_err(|_| SupportError::AlreadyOpen)?;
        Ok(Self { _file: file })
    }
}

/// 在旧文件仍存在时完成新文件写入；替换失败不删除原数据。
pub fn atomic_write(path: &Path, write: impl FnOnce(&mut BufWriter<File>) -> Result<(), SupportError>) -> Result<(), SupportError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) { fs::create_dir_all(parent)?; }
    let temp = path.with_extension(format!("tmp-{}", new_id().map_err(std::io::Error::other)?));
    let result = (|| {
        let file = OpenOptions::new().write(true).create_new(true).open(&temp)?;
        let mut writer = BufWriter::new(file);
        write(&mut writer)?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() { let _ = fs::remove_file(&temp); }
    result
}

pub fn save_versioned<T: Serialize>(
    path: &Path,
    version: u32,
    value: &T,
) -> Result<(), SupportError> {
    #[derive(Serialize)]
    struct Envelope<'a, T> { #[serde(rename = "schemaVersion")] version: u32, value: &'a T }
    atomic_write(path, |writer| { serde_json::to_writer(writer, &Envelope { version, value })?; Ok(()) })
}

pub fn load_versioned<T: DeserializeOwned>(
    path: &Path,
    supported: u32,
) -> Result<Option<(u32, T)>, SupportError> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    #[derive(serde::Deserialize)]
    struct Version { #[serde(rename = "schemaVersion", default)] version: u32 }
    let version = serde_json::from_slice::<Version>(&bytes)?.version;
    if version > supported { return Err(SupportError::FutureVersion(version)); }
    #[derive(serde::Deserialize)]
    struct Envelope<T> { value: T }
    Ok(Some((version, serde_json::from_slice::<Envelope<T>>(&bytes)?.value)))
}

pub fn mask_secret(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 8 {
        return "*".repeat(chars.len().max(1));
    }
    format!(
        "{}{}{}",
        chars[..4].iter().collect::<String>(),
        "*".repeat(chars.len() - 8),
        chars[chars.len() - 4..].iter().collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn masks_short_and_long_secrets() {
        assert_eq!(mask_secret("short"), "*****");
        assert_eq!(mask_secret("sk-live-0123456789"), "sk-l**********6789");
    }
    #[test]
    fn versioned_settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        save_versioned(&path, 1, &serde_json::json!({"enabled": true})).unwrap();
        let loaded: Option<(u32, serde_json::Value)> = load_versioned(&path, 1).unwrap();
        assert_eq!(loaded.unwrap().0, 1);
    }
    #[test]
    fn failed_write_keeps_previous_bytes_and_lock_releases() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        save_versioned(&path, 1, &vec!["original"]).unwrap();
        let original = fs::read(&path).unwrap();
        assert!(atomic_write(&path, |writer| { writer.write_all(b"partial")?; Err(std::io::Error::other("failure").into()) }).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        let lock_path = dir.path().join("writer.lock");
        let lock = FileLock::acquire(&lock_path).unwrap();
        assert!(FileLock::acquire(&lock_path).is_err());
        drop(lock);
        assert!(FileLock::acquire(&lock_path).is_ok());
        fs::write(&path, br#"{"schemaVersion":99,"value":"not-an-array"}"#).unwrap();
        assert!(matches!(load_versioned::<Vec<String>>(&path, 1), Err(SupportError::FutureVersion(99))));
    }
}
