//! Structured file logging, trace context and diagnostics for Rust desktop hosts.
use lumakit_support::SupportError;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use thiserror::Error;
use tracing_subscriber::{
    EnvFilter, Registry, fmt, layer::SubscriberExt, reload, util::SubscriberInitExt,
};

mod operation;
mod redact;
#[cfg(test)]
mod tests;
mod trace;

pub use operation::{OperationGuard, operation};
pub use redact::{bounded_text, diagnostic_hash, redact_secret, redact_url};
pub use trace::TraceContext;

/// 文件日志的最低记录级别；同时包含比所选级别更严重的事件。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum LogLevel {
    /// 仅记录错误。
    Error,
    /// 记录警告和错误。
    Warn,
    /// 记录信息、警告和错误。
    Info,
    /// 记录调试及更严重的事件，不包含 trace。
    Debug,
    /// 记录所有诊断事件。
    Trace,
}
impl LogLevel {
    fn as_filter(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}
/// 日志句柄当前级别及日志文件的存在状态。
#[derive(Debug, Clone, Serialize)]
pub struct LogStatus {
    /// 当前内存中的日志级别。
    pub level: LogLevel,
    /// 日志路径是否存在；不保证文件可读写。
    pub available: bool,
    /// 当前日志文件的路径。
    pub path: PathBuf,
}
/// 日志初始化、配置持久化或文件读取期间的错误。
#[derive(Debug, Error)]
pub enum LogError {
    /// 文件操作失败，或内部互斥锁已中毒。
    #[error(transparent)]
    Io(#[from] io::Error),
    /// 底层支持功能返回的错误。
    #[error(transparent)]
    Support(#[from] SupportError),
    /// 日志级别配置的 JSON 序列化失败。
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// 全局 tracing 订阅器已被初始化，无法再次安装。
    #[error("logging subscriber already initialized")]
    AlreadyInitialized,
    /// 无法通过重载句柄更新日志过滤器。
    #[error("invalid log level")]
    InvalidLevel,
}
#[derive(Debug, Serialize, Deserialize)]
struct LevelFile {
    level: LogLevel,
}
struct FileWriter {
    file: File,
    path: PathBuf,
    max_bytes: u64,
}
impl FileWriter {
    fn open(path: PathBuf, max_bytes: u64) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            file,
            path,
            max_bytes,
        })
    }
    fn rotate(&mut self, incoming: usize) -> io::Result<()> {
        if self.file.metadata()?.len() + incoming as u64 <= self.max_bytes {
            return Ok(());
        }
        self.file.flush()?;
        let old = self.path.with_extension("log.1");
        let _ = fs::remove_file(&old);
        fs::rename(&self.path, &old)?;
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        Ok(())
    }
}
impl Write for FileWriter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.rotate(b.len())?;
        self.file.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
#[derive(Clone)]
struct SharedWriter(Arc<Mutex<FileWriter>>);
impl Write for SharedWriter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("log writer poisoned"))?
            .write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("log writer poisoned"))?
            .flush()
    }
}
/// 可克隆的全局文件日志控制句柄；克隆之间共享级别和过滤器。
#[derive(Clone)]
pub struct LogHandle {
    level: Arc<Mutex<LogLevel>>,
    path: PathBuf,
    reload: reload::Handle<EnvFilter, Registry>,
}
impl LogHandle {
    /// 创建目录并安装全局日志订阅器，将日志追加到 `app.log`。
    ///
    /// 优先读取 `level.json`，读取或解析失败时使用 `default`。
    /// 写入使文件超过 `max_bytes` 时轮转到 `app.log.1`，仅保留一份旧文件。
    /// 目录或文件操作失败、全局订阅器已安装时返回错误。
    pub fn init(dir: &Path, default: LogLevel, max_bytes: u64) -> Result<Self, LogError> {
        fs::create_dir_all(dir)?;
        let path = dir.join("app.log");
        let level_path = dir.join("level.json");
        let level = fs::read(&level_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<LevelFile>(&b).ok())
            .map(|v| v.level)
            .unwrap_or(default);
        let filter = EnvFilter::new(level.as_filter());
        let (layer, reload) = reload::Layer::new(filter);
        let shared = SharedWriter(Arc::new(Mutex::new(FileWriter::open(
            path.clone(),
            max_bytes,
        )?)));
        let output = fmt::layer()
            .with_ansi(false)
            .with_writer(move || shared.clone());
        Registry::default()
            .with(layer)
            .with(output)
            .try_init()
            .map_err(|_| LogError::AlreadyInitialized)?;
        Ok(Self {
            level: Arc::new(Mutex::new(level)),
            path,
            reload,
        })
    }
    /// 返回共享的当前级别；级别锁中毒时返回 `Info`。
    pub fn level(&self) -> LogLevel {
        self.level.lock().map(|v| *v).unwrap_or(LogLevel::Info)
    }
    /// 更新过滤器及内存级别，再将级别写入 `level.json`。
    ///
    /// 重载、锁定或持久化失败时返回错误；持久化失败不会回滚已更新的级别。
    pub fn set_level(&self, level: LogLevel) -> Result<LogStatus, LogError> {
        self.reload
            .modify(|f| *f = EnvFilter::new(level.as_filter()))
            .map_err(|_| LogError::InvalidLevel)?;
        *self
            .level
            .lock()
            .map_err(|_| io::Error::other("log level poisoned"))? = level;
        fs::write(
            self.path.with_file_name("level.json"),
            serde_json::to_vec(&LevelFile { level })?,
        )?;
        Ok(self.status())
    }
    /// 返回当前级别、文件路径及路径是否存在的快照。
    pub fn status(&self) -> LogStatus {
        LogStatus {
            level: self.level(),
            available: self.path.exists(),
            path: self.path.clone(),
        }
    }
    /// 读取当前文件的最后若干行，按原顺序返回，最多 2000 行。
    ///
    /// 读取时载入整个文件；打开、读取或 UTF-8 解码失败时返回错误，不读取轮转文件。
    pub fn tail(&self, limit: usize) -> Result<Vec<String>, LogError> {
        let mut s = String::new();
        File::open(&self.path)?.read_to_string(&mut s)?;
        Ok(s.lines()
            .rev()
            .take(limit.min(2000))
            .map(str::to_owned)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect())
    }
    /// 当前不执行额外刷盘操作，始终返回成功。
    pub fn flush(&self) -> Result<(), LogError> {
        Ok(())
    }
}
