use std::time::Instant;

use tracing::{Level, Span};

use crate::TraceContext;

/// 生命周期 span 与事件的 target；格式器据此识别操作 span，并省略 `Target:` 段。
pub(crate) const TARGET: &str = module_path!();
/// 操作 span 的名称。
pub(crate) const SPAN: &str = "operation";

/// 一个操作的生命周期：创建时写 `EV:ENTRY`，结束时恰好写一条终态，`RT` 为结束减开始的毫秒数。
///
/// 级别：顶层（[`TraceContext::root`]）ENTRY/EXIT/CANCEL 为 INFO、ERROR 为 ERROR；
/// 下层（[`operation`]）ENTRY/EXIT/CANCEL 为 DEBUG、ERROR 为 WARN（错误只在顶层记一次 ERROR）；
/// TIMEOUT 与 UNFINISHED（guard 未结束即被丢弃）一律 WARN。
pub struct OperationGuard {
    root: bool,
    started: Instant,
    span: Span,
    finished: bool,
}

#[derive(Clone, Copy)]
enum End<'a> {
    Exit(&'static str),
    Error(&'a str),
    Cancel(&'a str),
    Timeout(u64),
    Unfinished,
}

/// 在当前 span 下开始一个下层操作（service、repository、external 等），以 DEBUG 写出 ENTRY。
///
/// 跨线程或进入 async 任务时，先在目标处进入上层 guard 的 [`OperationGuard::span`]，
/// 子操作才能记录 `ParentSpanId`；TraceId 由 `trace` 保证，不依赖当前 span。
pub fn operation(
    trace: &TraceContext,
    operation: &'static str,
    layer: &'static str,
) -> OperationGuard {
    start(trace, operation, layer, false)
}

pub(crate) fn start(
    trace: &TraceContext,
    operation: &'static str,
    layer: &'static str,
    root: bool,
) -> OperationGuard {
    // span 取 ERROR 级别只为在任何过滤级别下都存在，内部普通日志因而继承 TraceId；span 本身不输出行。
    let span = if root {
        tracing::span!(target: TARGET, parent: None, Level::ERROR, SPAN,
            trace_id = %trace.trace_id(), layer, operation)
    } else {
        tracing::span!(target: TARGET, Level::ERROR, SPAN,
     trace_id = %trace.trace_id(), layer, operation)
    };
    let guard = OperationGuard {
        root,
        started: Instant::now(),
        span,
        finished: false,
    };
    guard.emit(if root { Level::INFO } else { Level::DEBUG }, None);
    guard
}

macro_rules! lifecycle {
    ($level:expr, $span:expr, $event:expr, $rt:expr, $result:expr, $error:expr, $reason:expr, $limit:expr) => {
        tracing::event!(
     target: TARGET,
            parent: $span,
            $level,
            event = $event,
     duration_ms = $rt,
     result = $result,
     error_code = $error,
            reason = $reason,
     limit_ms = $limit,
        )
    };
}

impl OperationGuard {
    /// 本操作的 span；克隆后在其他线程或 async 任务中进入，那里的日志即带同一 TraceId。
    pub fn span(&self) -> &Span {
        &self.span
    }

    /// 记录成功出口 `EV:EXIT|RT|Result:<result>`；`result` 是有限摘要，不写业务正文。
    pub fn finish(mut self, result: &'static str) {
        self.end(End::Exit(result));
    }

    /// 记录失败出口 `EV:ERROR|RT|Result:FAILED|Error:<error_code>`；只传稳定错误码，不传错误正文。
    pub fn fail(mut self, error_code: &str) {
        self.end(End::Error(error_code));
    }

    /// 记录取消 `EV:CANCEL|RT|Result:CANCELLED|Reason:<reason>`；取消不按错误记录。
    pub fn cancel(mut self, reason: &str) {
        self.end(End::Cancel(reason));
    }

    /// 记录超时 `EV:TIMEOUT|RT|Result:TIMEOUT|Limit:<limit_ms>`。
    ///
    /// 实际耗时 `RT` 由结束减开始自动得出；`limit_ms` 是调用方配置的时限，用于对照是否按时触发。
    pub fn timeout(mut self, limit_ms: u64) {
        self.end(End::Timeout(limit_ms));
    }

    fn end(&mut self, end: End<'_>) {
        if self.finished {
            return;
        }
        self.finished = true;
        let level = match end {
            End::Exit(_) | End::Cancel(_) if self.root => Level::INFO,
            End::Exit(_) | End::Cancel(_) => Level::DEBUG,
            End::Error(_) if self.root => Level::ERROR,
            End::Error(_) | End::Timeout(_) | End::Unfinished => Level::WARN,
        };
        self.emit(level, Some(end));
    }

    fn emit(&self, level: Level, end: Option<End<'_>>) {
        let (event, result, error, reason, limit) = match end {
            None => ("ENTRY", None, None, None, None),
            Some(End::Exit(result)) => ("EXIT", Some(result), None, None, None),
            Some(End::Error(code)) => ("ERROR", Some("FAILED"), Some(code), None, None),
            Some(End::Cancel(reason)) => ("CANCEL", Some("CANCELLED"), None, Some(reason), None),
            Some(End::Timeout(limit)) => ("TIMEOUT", Some("TIMEOUT"), None, None, Some(limit)),
            Some(End::Unfinished) => (
                "UNFINISHED",
                Some("UNFINISHED"),
                None,
                Some("guard_dropped"),
                None,
            ),
        };
        let rt = end.map(|_| u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX));
        let span = &self.span;
        match level {
            Level::ERROR => lifecycle!(Level::ERROR, span, event, rt, result, error, reason, limit),
            Level::WARN => lifecycle!(Level::WARN, span, event, rt, result, error, reason, limit),
            Level::INFO => lifecycle!(Level::INFO, span, event, rt, result, error, reason, limit),
            _ => lifecycle!(Level::DEBUG, span, event, rt, result, error, reason, limit),
        }
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        self.end(End::Unfinished);
    }
}
