//! Minimal ports defined by desktop feature consumers.
use lumakit_support::SupportError;
use std::sync::Arc;
use thiserror::Error;

/// 日志端口使用的记录级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// 错误级别。
    Error,
    /// 警告级别。
    Warn,
    /// 信息级别。
    Info,
    /// 调试级别。
    Debug,
}
/// 一次系统资源采样中的百分比；具体采样口径由端口实现决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemMetrics {
    /// CPU 使用百分比。
    pub cpu_percent: u8,
    /// 内存使用百分比。
    pub memory_percent: u8,
}
/// 宿主端口调用失败的原因。
#[derive(Debug, Error)]
pub enum PortError {
    /// 端口不可用及其说明。
    #[error("port unavailable: {0}")]
    Unavailable(String),
    /// 底层支持功能返回的错误。
    #[error(transparent)]
    Support(#[from] SupportError),
}

/// 提供 Unix 时间的可共享时钟。
pub trait Clock: Send + Sync {
    /// 返回自 Unix 纪元起的毫秒数。
    fn now_unix_ms(&self) -> i64;
}
/// 系统资源快照的采样入口。
pub trait SystemSnapshot: Send + Sync {
    /// 获取一次资源采样；不可用时返回端口错误。
    fn sample(&self) -> Result<SystemMetrics, PortError>;
}
/// 按值类型提供加载和保存能力；存储位置及格式由实现决定。
pub trait StateStore<T>: Send + Sync {
    /// 加载已保存的值；尚无值时返回 `None`，读取失败时返回错误。
    fn load(&self) -> Result<Option<T>, PortError>;
    /// 保存给定值；无法写入时返回错误。
    fn save(&self, value: &T) -> Result<(), PortError>;
}
/// 面向功能消费者的日志控制与读取接口。
pub trait LogPort: Send + Sync {
    /// 返回当前日志级别。
    fn level(&self) -> LogLevel;
    /// 设置日志级别并返回生效级别；无法更新时返回错误。
    fn set_level(&self, level: LogLevel) -> Result<LogLevel, PortError>;
    /// 读取末尾日志行；数量上限、顺序及存储边界由实现决定。
    fn tail(&self, limit: usize) -> Result<Vec<String>, PortError>;
}

/// 每次读取均返回同一个时间戳的时钟。
pub struct FixedClock(/// 固定返回的 Unix 毫秒时间戳。
    pub i64);
impl Clock for FixedClock {
    fn now_unix_ms(&self) -> i64 {
        self.0
    }
}
/// 每次采样均成功返回固定指标的系统快照。
pub struct FakeSystemSnapshot(/// 固定返回的资源指标。
    pub SystemMetrics);
impl SystemSnapshot for FakeSystemSnapshot {
    fn sample(&self) -> Result<SystemMetrics, PortError> {
        Ok(self.0)
    }
}
/// 通过原子引用计数共享的端口或其它值。
pub type Shared<T> = Arc<T>;
