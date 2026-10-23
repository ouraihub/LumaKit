//! Minimal ports defined by desktop feature consumers.
use lumakit_support::SupportError;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemMetrics {
    pub cpu_percent: u8,
    pub memory_percent: u8,
}
#[derive(Debug, Error)]
pub enum PortError {
    #[error("port unavailable: {0}")]
    Unavailable(String),
    #[error(transparent)]
    Support(#[from] SupportError),
}

pub trait Clock: Send + Sync {
    fn now_unix_ms(&self) -> i64;
}
pub trait SystemSnapshot: Send + Sync {
    fn sample(&self) -> Result<SystemMetrics, PortError>;
}
pub trait StateStore<T>: Send + Sync {
    fn load(&self) -> Result<Option<T>, PortError>;
    fn save(&self, value: &T) -> Result<(), PortError>;
}
pub trait LogPort: Send + Sync {
    fn level(&self) -> LogLevel;
    fn set_level(&self, level: LogLevel) -> Result<LogLevel, PortError>;
    fn tail(&self, limit: usize) -> Result<Vec<String>, PortError>;
}

pub struct FixedClock(pub i64);
impl Clock for FixedClock {
    fn now_unix_ms(&self) -> i64 {
        self.0
    }
}
pub struct FakeSystemSnapshot(pub SystemMetrics);
impl SystemSnapshot for FakeSystemSnapshot {
    fn sample(&self) -> Result<SystemMetrics, PortError> {
        Ok(self.0)
    }
}
pub type Shared<T> = Arc<T>;
