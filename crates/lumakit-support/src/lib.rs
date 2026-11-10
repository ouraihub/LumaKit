//! Shared, UI-free contracts for LumaKit desktop applications.
/// 随机配置 ID 与系统加密凭据；明文仅在显式解密时短暂持有。
pub mod secrets;
pub mod sqlite;
pub use secrets::new_id;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};
use thiserror::Error;

/// 数据目录、文件锁和版本化设置操作失败的原因。
#[derive(Debug, Error)]
pub enum SupportError {
    /// 应用标识不合法，或无法取得可执行文件所在目录。
    #[error("invalid app slug")]
    InvalidAppSlug,
    /// 文件系统、写入探针或随机标识生成失败。
    #[error("path operation failed")]
    Io(#[from] std::io::Error),
    /// 设置 JSON 编码或解码失败，调用方不应覆盖原文件。
    #[error("settings serialization failed")]
    Json(#[from] serde_json::Error),
    /// 文件版本高于当前支持值；拒绝加载，避免旧程序误写新格式。
    #[error("settings version {0} is newer than this application")]
    FutureVersion(u32),
    /// 未能取得独占文件锁，可能已被其它实例持有。
    #[error("another process is using this data directory")]
    AlreadyOpen,
}

/// 校验 1–32 字节的小写字母、数字或连字符标识，再返回可执行文件旁的可写 data 目录。
///
/// 标识不用于目录隔离；不同标识仍使用同一便携目录。
///
/// # Errors
/// 标识不合法返回 [`SupportError::InvalidAppSlug`]；目录创建或可写探测失败返回 I/O 错误。
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

/// 返回 EXE 旁的 `data` 目录，确认可写后才返回。
///
/// 可写目录必须明确；静默回退会让用户误以为历史丢失。
///
/// # Errors
/// 无法定位 EXE 目录返回 [`SupportError::InvalidAppSlug`]；目录创建或写入探测失败返回 I/O 错误。
pub fn portable_data_root() -> Result<PathBuf, SupportError> {
    let root = std::env::current_exe()?
        .parent()
        .ok_or(SupportError::InvalidAppSlug)?
        .join("data");
    fs::create_dir_all(&root)?;
    let probe = root.join(format!(
        ".write-probe-{}",
        new_id().map_err(std::io::Error::other)?
    ));
    File::create(&probe)?.sync_all()?;
    fs::remove_file(probe)?;
    Ok(root)
}

/// 锁文件可残留；是否占用由内核句柄锁决定，不靠文件是否存在。
#[derive(Debug)]
pub struct FileLock {
    _file: File,
}
impl FileLock {
    /// 创建父目录并取得独占文件锁；持有本值期间保持锁定，释放本值后解锁。
    ///
    /// # Errors
    /// 创建或打开失败返回 I/O 错误；任何内核锁定失败均映射为 [`SupportError::AlreadyOpen`]。
    pub fn acquire(path: &Path) -> Result<Self, SupportError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.try_lock().map_err(|_| SupportError::AlreadyOpen)?;
        Ok(Self { _file: file })
    }
}

/// 在旧文件仍存在时完成新文件写入；替换失败不删除原数据。
///
/// # Errors
/// 临时文件创建、`write` 回调、刷盘或替换失败时返回错误，并删除临时文件。
pub fn atomic_write(
    path: &Path,
    write: impl FnOnce(&mut BufWriter<File>) -> Result<(), SupportError>,
) -> Result<(), SupportError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("tmp-{}", new_id().map_err(std::io::Error::other)?));
    let result = (|| {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        let mut writer = BufWriter::new(file);
        write(&mut writer)?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// 将值与 schemaVersion 一起原子保存；替换失败时保留旧文件。
///
/// # Errors
/// JSON 序列化或文件创建、同步、替换失败时返回错误。
pub fn save_versioned<T: Serialize>(
    path: &Path,
    version: u32,
    value: &T,
) -> Result<(), SupportError> {
    #[derive(Serialize)]
    struct Envelope<'a, T> {
        #[serde(rename = "schemaVersion")]
        version: u32,
        value: &'a T,
    }
    atomic_write(path, |writer| {
        serde_json::to_writer(writer, &Envelope { version, value })?;
        Ok(())
    })
}

/// 读取版本与值；文件不存在返回 None，缺省 schemaVersion 按 0 处理，不自动迁移。
///
/// # Errors
/// 先检查版本上限，再解码值；新版本返回 [`SupportError::FutureVersion`]，读取或格式错误原样返回。
pub fn load_versioned<T: DeserializeOwned>(
    path: &Path,
    supported: u32,
) -> Result<Option<(u32, T)>, SupportError> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    #[derive(serde::Deserialize)]
    struct Version {
        #[serde(rename = "schemaVersion", default)]
        version: u32,
    }
    let version = serde_json::from_slice::<Version>(&bytes)?.version;
    if version > supported {
        return Err(SupportError::FutureVersion(version));
    }
    #[derive(serde::Deserialize)]
    struct Envelope<T> {
        value: T,
    }
    Ok(Some((
        version,
        serde_json::from_slice::<Envelope<T>>(&bytes)?.value,
    )))
}

/// 按 Unicode 字符脱敏：超过八字符保留前后各四个，其余替换为星号；短值全部遮蔽。
///
/// 空串显示一个星号；此方法只改变显示，不提供加密或安全存储。
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
    fn versioned_settings_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("settings.json");
        save_versioned(&path, 1, &serde_json::json!({"enabled": true}))?;
        let loaded: Option<(u32, serde_json::Value)> = load_versioned(&path, 1)?;
        assert_eq!(loaded.ok_or("settings were not loaded")?.0, 1);
        Ok(())
    }
    #[test]
    fn failed_write_keeps_previous_bytes_and_lock_releases()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("settings.json");
        save_versioned(&path, 1, &vec!["original"])?;
        let original = fs::read(&path)?;
        assert!(
            atomic_write(&path, |writer| {
                writer.write_all(b"partial")?;
                Err(std::io::Error::other("failure").into())
            })
            .is_err()
        );
        assert_eq!(fs::read(&path)?, original);
        let lock_path = dir.path().join("writer.lock");
        let lock = FileLock::acquire(&lock_path)?;
        assert!(FileLock::acquire(&lock_path).is_err());
        drop(lock);
        assert!(FileLock::acquire(&lock_path).is_ok());
        fs::write(&path, br#"{"schemaVersion":99,"value":"not-an-array"}"#)?;
        assert!(matches!(
            load_versioned::<Vec<String>>(&path, 1),
            Err(SupportError::FutureVersion(99))
        ));
        Ok(())
    }
}
