//! 文件日志、TraceId 链路与诊断工具：同一 TraceId 的入口、出口、错误和耗时可用 grep 检索。
use lumakit_support::SupportError;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use thiserror::Error;
use tracing_subscriber::{
    EnvFilter, Registry, fmt, layer::SubscriberExt, reload, util::SubscriberInitExt,
};

mod format;
mod operation;
mod panic_log;
mod parse;
mod redact;
mod source;
#[cfg(test)]
mod tests;
mod trace;
mod writer;

use writer::SharedWriter;

pub use format::{GrepFormat, SpanFields};
pub use operation::{OperationGuard, operation};
pub use parse::{LogLine, parse_line};
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
    /// 日志级别配置的 JSON 解析或序列化失败。
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
/// 可克隆的全局文件日志控制句柄；克隆之间共享级别和过滤器。
#[derive(Clone)]
pub struct LogHandle {
    level: Arc<Mutex<LogLevel>>,
    path: PathBuf,
    reload: reload::Handle<EnvFilter, Registry>,
    writer: SharedWriter,
}
impl LogHandle {
    /// 创建目录并安装全局日志订阅器，将日志追加到 `app.log`。
    ///
    /// 优先读取 `level.json`，仅文件不存在时使用 `default`；损坏配置和读取失败返回错误。
    /// 写入使文件超过 `max_bytes` 时轮转到 `app.log.1`，仅保留一份旧文件。
    /// 每行使用 `TraceId:…|Layer:…|ME:…|EV:…` 的 grep 格式。
    /// 安装安全同步 panic hook，保留 unwind/abort，不输出原始载荷。
    ///
    /// # Errors
    /// 目录或文件操作失败、全局订阅器已安装时返回错误。
    pub fn init(dir: &Path, default: LogLevel, max_bytes: u64) -> Result<Self, LogError> {
        fs::create_dir_all(dir)?;
        let path = dir.join("app.log");
        let level_path = dir.join("level.json");
        let level = match fs::read(&level_path) {
            Ok(bytes) => serde_json::from_slice::<LevelFile>(&bytes)?.level,
            Err(error) if error.kind() == io::ErrorKind::NotFound => default,
            Err(error) => return Err(error.into()),
        };
        let filter = log_filter(level);
        let (layer, reload) = reload::Layer::new(filter);
        let shared = SharedWriter::open(path.clone(), max_bytes)?;
        let output_writer = shared.clone();
        let output = fmt::layer()
            .with_ansi(false)
            .event_format(GrepFormat::utc())
            .with_writer(move || output_writer.clone())
            .log_internal_errors(false);
        Registry::default()
            .with(layer)
            .with(SpanFields)
            .with(output)
            .try_init()
            .map_err(|_| LogError::AlreadyInitialized)?;
        panic_log::install(shared.clone());
        Ok(Self {
            level: Arc::new(Mutex::new(level)),
            path,
            reload,
            writer: shared,
        })
    }
    /// 返回共享的当前级别；锁中毒时读取锁内最后保存的值，不替换为默认级别。
    pub fn level(&self) -> LogLevel {
        *self
            .level
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    /// 更新过滤器及内存级别，再将级别写入 `level.json`。
    ///
    /// # Errors
    /// 重载、锁定或持久化失败时返回错误；持久化失败不会回滚已更新的级别。
    pub fn set_level(&self, level: LogLevel) -> Result<LogStatus, LogError> {
        self.reload
            .modify(|f| *f = log_filter(level))
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
    /// 读取时载入整个文件，不读取轮转文件。
    ///
    /// # Errors
    /// 打开、读取或 UTF-8 解码失败时返回错误。
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
    /// 刷新实际日志 writer；若之前的写入或轮转失败，也返回该失败。
    ///
    /// # Errors
    /// writer 锁中毒、写入或刷新失败时返回错误。
    pub fn flush(&self) -> Result<(), LogError> {
        self.writer.clone().flush()?;
        Ok(())
    }
}

fn log_filter(level: LogLevel) -> EnvFilter {
    EnvFilter::new(format!("{},{}=error", level.as_filter(), panic_log::TARGET))
}
