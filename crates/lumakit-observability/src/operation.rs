use std::time::Instant;

use tracing::{Span, field};

use crate::TraceContext;

/// A structured operation lifecycle: entry, one terminal event, and elapsed milliseconds.
pub struct OperationGuard {
    started: Instant,
    span: Span,
    finished: bool,
}

/// Starts a child operation at the given logical layer.
pub fn operation(
    trace: &TraceContext,
    operation: &'static str,
    layer: &'static str,
) -> OperationGuard {
    let span = tracing::debug_span!(
        "operation",
        trace_id = %trace.trace_id(),
        operation,
        layer,
        event = field::Empty,
        duration_ms = field::Empty,
        result = field::Empty,
        error_code = field::Empty,
        reason = field::Empty,
    );
    span.record("event", "entry");
    OperationGuard {
        started: Instant::now(),
        span,
        finished: false,
    }
}

impl OperationGuard {
    /// Records a successful exit.
    pub fn finish(mut self, result: &'static str) {
        self.finish_event("exit", result, None, None);
    }

    /// Records an error exit with a stable error code.
    pub fn fail(mut self, error_code: &str) {
        self.finish_event("error", "failed", Some(error_code), None);
    }

    /// Records a user/system cancellation without classifying it as an error.
    pub fn cancel(mut self, reason: &str) {
        self.finish_event("cancel", "cancelled", None, Some(reason));
    }

    /// Records a timeout without losing the operation duration.
    pub fn timeout(mut self, timeout_ms: u64) {
        self.span.record("timeout_ms", timeout_ms);
        self.finish_event("timeout", "timed_out", None, Some("deadline"));
    }

    fn finish_event(
        &mut self,
        event: &'static str,
        result: &'static str,
        error: Option<&str>,
        reason: Option<&str>,
    ) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.span.record("event", event);
        self.span
            .record("duration_ms", self.started.elapsed().as_millis() as u64);
        self.span.record("result", result);
        if let Some(error) = error {
            self.span.record("error_code", error);
        }
        if let Some(reason) = reason {
            self.span.record("reason", reason);
        }
        match event {
            "error" => tracing::error!(parent: &self.span, event, result, "operation failed"),
            "cancel" | "timeout" => {
                tracing::warn!(parent: &self.span, event, result, "operation ended without success")
            }
            _ => tracing::debug!(parent: &self.span, event, result, "operation completed"),
        }
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.finish_event("unfinished", "unfinished", None, Some("guard_dropped"));
        }
    }
}
