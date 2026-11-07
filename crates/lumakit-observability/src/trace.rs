use std::fmt;

use lumakit_support::new_id;
use tracing::{Span, field};

/// Immutable diagnostic context shared by one top-level operation and its child work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceContext {
    trace_id: String,
}

impl TraceContext {
    /// Creates a new root trace and its entry span.
    pub fn root(operation: &'static str) -> Result<(Self, Span), String> {
        let trace_id = new_id()?;
        let span =
            tracing::info_span!("operation", trace_id = %trace_id, operation, event = field::Empty);
        span.record("event", "entry");
        Ok((Self { trace_id }, span))
    }

    /// Creates a child span that preserves this context's trace id.
    pub fn child(&self, operation: &'static str) -> Span {
        let span = tracing::debug_span!("operation", trace_id = %self.trace_id, operation, event = field::Empty);
        span.record("event", "entry");
        span
    }

    /// Returns the stable id used to search the complete operation chain.
    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }
}

impl fmt::Display for TraceContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.trace_id)
    }
}
