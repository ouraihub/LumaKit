//! Structured file logging and diagnostics for Rust desktop hosts.
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}
impl LogLevel {
    fn as_filter(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct LogStatus {
    pub level: LogLevel,
    pub available: bool,
    pub path: PathBuf,
}
#[derive(Debug, Error)]
pub enum LogError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Support(#[from] SupportError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("logging subscriber already initialized")]
    AlreadyInitialized,
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
#[derive(Clone)]
pub struct LogHandle {
    level: Arc<Mutex<LogLevel>>,
    path: PathBuf,
    reload: reload::Handle<EnvFilter, Registry>,
}
impl LogHandle {
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
    pub fn level(&self) -> LogLevel {
        self.level.lock().map(|v| *v).unwrap_or(LogLevel::Info)
    }
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
    pub fn status(&self) -> LogStatus {
        LogStatus {
            level: self.level(),
            available: self.path.exists(),
            path: self.path.clone(),
        }
    }
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
    pub fn flush(&self) -> Result<(), LogError> {
        Ok(())
    }
}
