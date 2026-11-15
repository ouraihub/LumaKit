//! Host-configured daily rolling log. The host passes a filename prefix
//! and a data directory; the subscriber writes `<prefix>.YYYY-MM-DD.log`
//! files and discards files older than `keep_days` days on init.
//!
//! The daily path uses the same `GrepFormat`, `TraceContext` and
//! `operation` helpers as the single-file `LogHandle::init` path; only
//! the writer and the panic hook differ.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use time::{OffsetDateTime, format_description::FormatItem, macros::format_description};
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{
    EnvFilter, Registry, fmt, layer::SubscriberExt, reload, util::SubscriberInitExt,
};

use crate::format::{GrepFormat, SpanFields};
use crate::{LogError, LogLevel, LogStatus, log_filter};

/// Daily log file retention: files whose date is older than `keep_days`
/// days (inclusive) relative to today are deleted on init.
///
/// Reusable across hosts; a value of `0` disables the cleanup pass.
#[derive(Debug, Clone, Copy)]
pub struct DailyOptions {
    /// Keep how many days of files, including today. Older files are
    /// deleted at init time.
    pub keep_days: usize,
}

impl Default for DailyOptions {
    fn default() -> Self {
        Self { keep_days: 7 }
    }
}

/// Cloneable handle to a daily-rolling file log. Keep it alive until the
/// process exits; dropping it flushes buffered lines through the owned
/// `WorkerGuard`.
#[derive(Clone)]
pub struct DailyLogHandle {
    inner: Arc<DailyInner>,
}

struct DailyInner {
    level: Mutex<LogLevel>,
    dir: PathBuf,
    prefix: String,
    reload: reload::Handle<EnvFilter, Registry>,
    _guard: WorkerGuard,
}

impl std::fmt::Debug for DailyLogHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DailyLogHandle")
            .field("dir", &self.inner.dir)
            .field("prefix", &self.inner.prefix)
            .finish_non_exhaustive()
    }
}

impl DailyLogHandle {
    /// Install the global subscriber. Creates `<dir>/` if needed, writes
    /// `<prefix>.YYYY-MM-DD.log`, and installs the shared panic hook.
    ///
    /// # Errors
    /// Directory creation fails, or a subscriber is already installed.
    pub fn init(
        dir: &Path,
        prefix: &str,
        default: LogLevel,
        opts: DailyOptions,
    ) -> Result<Self, LogError> {
        if prefix.is_empty() {
            return Err(LogError::InvalidLevel);
        }
        fs::create_dir_all(dir)?;
        prune_old_files(dir, prefix, opts.keep_days)?;

        let appender = RollingFileAppender::new(Rotation::DAILY, dir, prefix);
        let (non_blocking, guard) = tracing_appender::non_blocking(appender);

        let filter = log_filter(default);
        let (layer, reload) = reload::Layer::new(filter);
        let output = fmt::layer()
            .with_ansi(false)
            .event_format(GrepFormat::utc())
            .with_writer(non_blocking.clone())
            .log_internal_errors(false);
        Registry::default()
            .with(layer)
            .with(SpanFields)
            .with(output)
            .try_init()
            .map_err(|_| LogError::AlreadyInitialized)?;

        install_panic_hook(non_blocking);

        Ok(Self {
            inner: Arc::new(DailyInner {
                level: Mutex::new(default),
                dir: dir.to_path_buf(),
                prefix: prefix.to_owned(),
                reload,
                _guard: guard,
            }),
        })
    }

    /// Current effective level.
    #[must_use]
    pub fn level(&self) -> LogLevel {
        *self
            .inner
            .level
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Update the effective level at runtime.
    ///
    /// # Errors
    /// Reload failure or poisoned internal mutex.
    pub fn set_level(&self, level: LogLevel) -> Result<LogStatus, LogError> {
        self.inner
            .reload
            .modify(|f| *f = log_filter(level))
            .map_err(|_| LogError::InvalidLevel)?;
        *self
            .inner
            .level
            .lock()
            .map_err(|_| io::Error::other("log level poisoned"))? = level;
        Ok(self.status())
    }

    /// Snapshot of level + newest-file path.
    #[must_use]
    pub fn status(&self) -> LogStatus {
        let path = newest_file(&self.inner.dir, &self.inner.prefix)
            .ok()
            .flatten()
            .unwrap_or_else(|| today_path(&self.inner.dir, &self.inner.prefix));
        LogStatus {
            level: self.level(),
            available: path.exists(),
            path,
        }
    }

    /// Return the newest-file path, or the computed today path if none
    /// exists yet.
    #[must_use]
    pub fn current_path(&self) -> PathBuf {
        self.status().path
    }

    /// Read the tail of the newest file. Returns at most `limit` lines
    /// (capped at `MAX_TAIL_LINES`) and at most `MAX_TAIL_BYTES`, oldest
    /// first.
    ///
    /// # Errors
    /// Open, read, or UTF-8 decode failure.
    pub fn tail(&self, limit: usize) -> Result<Vec<String>, LogError> {
        let Some(path) = newest_file(&self.inner.dir, &self.inner.prefix)? else {
            return Ok(Vec::new());
        };
        let mut file = fs::File::open(&path)?;
        let metadata = file.metadata()?;
        let size = metadata.len();
        let want = MAX_TAIL_BYTES.min(size);
        let mut buf = String::new();
        if size > want {
            use std::io::Seek as _;
            file.seek(io::SeekFrom::Start(size - want))?;
        }
        file.read_to_string(&mut buf).ok();
        let cap = limit.min(MAX_TAIL_LINES);
        Ok(buf
            .lines()
            .rev()
            .take(cap)
            .map(str::to_owned)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect())
    }
}

/// Upper bound on bytes read from the newest log for `tail`.
pub const MAX_TAIL_BYTES: u64 = 256 * 1024;
/// Upper bound on returned lines from `tail`.
pub const MAX_TAIL_LINES: usize = 2000;

const DATE_FORMAT: &[FormatItem<'_>] = format_description!("[year]-[month]-[day]");

fn today_path(dir: &Path, prefix: &str) -> PathBuf {
    let today = OffsetDateTime::now_utc()
        .format(DATE_FORMAT)
        .unwrap_or_else(|_| "0000-00-00".to_owned());
    dir.join(format!("{prefix}.{today}.log"))
}

fn newest_file(dir: &Path, prefix: &str) -> io::Result<Option<PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut newest: Option<PathBuf> = None;
    let want_prefix = format!("{prefix}.");
    for entry in entries {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !(name.starts_with(&want_prefix) && name.ends_with(".log")) {
            continue;
        }
        match &newest {
            Some(current) if current >= &path => {}
            _ => newest = Some(path),
        }
    }
    Ok(newest)
}

fn prune_old_files(dir: &Path, prefix: &str, keep_days: usize) -> io::Result<()> {
    if keep_days == 0 {
        return Ok(());
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let want_prefix = format!("{prefix}.");
    let mut matching: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with(&want_prefix) && name.ends_with(".log") {
            matching.push(path);
        }
    }
    matching.sort();
    if matching.len() > keep_days {
        let drop_count = matching.len() - keep_days;
        for path in matching.into_iter().take(drop_count) {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

fn install_panic_hook(writer: NonBlocking) {
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str));
        let summary = if payload.is_some() {
            "redacted_string_payload"
        } else {
            "non_string_payload"
        };
        let location = info.location();
        let file = location.map(|at| at.file()).unwrap_or_default();
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        let hash = payload.map(crate::diagnostic_hash);
        tracing::error!(target: "lumakit_panic",
            code = "RUST_PANIC",
            summary,
            payload_hash = hash.as_deref().unwrap_or(""),
            panic_file = file,
            panic_line = location.map(|at| at.line()).unwrap_or(0),
            panic_column = location.map(|at| at.column()).unwrap_or(0),
            thread_id = ?std::thread::current().id(),
            backtrace = %backtrace,
            "app.panic");
        let mut writer = writer.clone();
        let _ = writer.flush();
    }));
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn prune_keeps_newest_n_files() {
        let dir = tempfile::tempdir().unwrap();
        for day in ["2024-01-01", "2024-01-02", "2024-01-03", "2024-01-04"] {
            fs::write(dir.path().join(format!("demo.{day}.log")), b"x").unwrap();
        }
        prune_old_files(dir.path(), "demo", 2).unwrap();
        let mut remaining: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        remaining.sort();
        assert_eq!(
            remaining,
            vec!["demo.2024-01-03.log", "demo.2024-01-04.log"]
        );
    }

    #[test]
    fn prune_zero_days_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("demo.2024-01-01.log"), b"x").unwrap();
        prune_old_files(dir.path(), "demo", 0).unwrap();
        assert!(dir.path().join("demo.2024-01-01.log").exists());
    }

    #[test]
    fn newest_returns_the_lexicographic_maximum() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("app.2024-01-01.log"), b"x").unwrap();
        fs::write(dir.path().join("app.2024-02-01.log"), b"x").unwrap();
        let newest = newest_file(dir.path(), "app").unwrap().unwrap();
        assert!(newest.ends_with("app.2024-02-01.log"));
    }

    #[test]
    fn today_path_uses_prefix_and_dir() {
        let dir = Path::new("/tmp");
        let path = today_path(dir, "autoterm");
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("autoterm."));
        assert!(name.ends_with(".log"));
    }
}
