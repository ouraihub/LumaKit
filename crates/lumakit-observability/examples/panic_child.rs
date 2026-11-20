//! Isolated process fixture for the panic logging regression; never used by a host.
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "isolated panic regression fixture"
)]

use std::path::PathBuf;

use lumakit_observability::{LogHandle, LogLevel, TraceContext, operation};

struct UnwindMarker(PathBuf);

impl Drop for UnwindMarker {
    fn drop(&mut self) {
        std::fs::write(&self.0, b"unwound").expect("write unwind marker");
    }
}

fn main() {
    let dir = PathBuf::from(std::env::args_os().nth(1).expect("fixture log directory"));
    let logs = LogHandle::init(&dir, LogLevel::Info, 16 * 1024 * 1024).expect("initialize logging");
    logs.set_level(LogLevel::Error)
        .expect("reload Error filter");
    let (trace, root) = TraceContext::root("panic.fixture.root", "worker");
    std::fs::write(dir.join("trace.txt"), trace.trace_id()).expect("write expected trace");
    let _root = root.span().enter();
    let child = operation(&trace, "panic.fixture.child", "service");
    let _child = child.span().enter();
    tracing::info!("panic.fixture.filtered");
    let _unwind = UnwindMarker(dir.join("unwound.txt"));
    if std::env::args_os().nth(2).is_some() {
        std::panic::panic_any(vec!["SYNTHETIC_NON_STRING_SECRET"]);
    }
    panic!("SYNTHETIC_PANIC_SECRET body=PRIVATE_BODY C:/PRIVATE_USER/PAYLOAD_PATH");
}
