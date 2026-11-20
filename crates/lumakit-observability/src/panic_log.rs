//! Synchronous, payload-safe Rust panic diagnostics using the installed file formatter.
use std::io::Write as _;

use crate::{diagnostic_hash, source, writer::SharedWriter};

pub(crate) const TARGET: &str = "lumakit_panic";

pub(crate) fn install(writer: SharedWriter) {
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
        let file = location
            .map(|at| source::source_path(at.file()))
            .unwrap_or_default();
        let backtrace = source::backtrace();
        tracing::error!(target: "lumakit_panic",
            code = "RUST_PANIC", summary,
            payload_hash = payload.map(diagnostic_hash),
            panic_file = file, panic_line = location.map(|at| at.line()),
            panic_column = location.map(|at| at.column()),
            thread_id = ?std::thread::current().id(), backtrace = %backtrace,
            "app.panic");
        // The writer owns safe, non-recursive failure reporting. Never invoke the old hook:
        // the default stderr hook prints arbitrary payloads. Returning preserves unwind/abort.
        let _ = writer.clone().flush();
    }));
}
