//! Window-close noise from GPUI's Windows backend, demoted once the host reports it is closing.
//!
//! `gpui-pre-windows` (Zed's `gpui_windows`) keeps calling Win32 on a destroyed window during
//! close and logs every `ERROR_INVALID_WINDOW_HANDLE` at ERROR through `log_err()`; see
//! longbridge/gpui-component#3795. Only that backend, only those two codes and only after
//! [`LogHandle::mark_closing`](crate::LogHandle::mark_closing) are demoted; the same failure
//! while the window is open stays ERROR. Remove this module once the backend checks `IsWindow`.
use std::{
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use tracing::{
    Event, Level, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::layer::{Context, Layer};

use crate::LogLevel;

/// Fixed reason written on a demoted line.
pub(crate) const REASON: &str = "UI_WINDOW_HANDLE_CLOSED";
/// `ERROR_INVALID_WINDOW_HANDLE` as HRESULT, and the OLE variant DestroyWindow/RevokeDragDrop return.
const CODES: [&str; 2] = ["(0x80070578)", "(0x80040102)"];

/// Whether `event` is one of the backend's close-time window-handle errors.
pub(crate) fn is_window_handle_noise(event: &Event<'_>) -> bool {
    let metadata = event.metadata();
    if *metadata.level() != Level::ERROR || metadata.target() != "log" {
        return false;
    }
    let mut fields = Fields::default();
    event.record(&mut fields);
    fields.file.contains("gpui-pre-windows")
        && CODES.iter().any(|code| fields.message.contains(code))
}

#[derive(Default)]
struct Fields {
    file: String,
    message: String,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "log.file" => value.clone_into(&mut self.file),
            "message" => value.clone_into(&mut self.message),
            _ => {}
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        match field.name() {
            "log.file" => self.file = format!("{value:?}"),
            "message" => self.message = format!("{value:?}"),
            _ => {}
        }
    }
}

/// Drops demoted events the current level would not record as DEBUG.
pub(crate) struct ShutdownNoise {
    pub(crate) closing: Arc<AtomicBool>,
    pub(crate) level: Arc<Mutex<LogLevel>>,
}

impl<S: Subscriber> Layer<S> for ShutdownNoise {
    fn event_enabled(&self, event: &Event<'_>, _: Context<'_, S>) -> bool {
        if !self.closing.load(Ordering::Acquire) || !is_window_handle_noise(event) {
            return true;
        }
        let level = *self
            .level
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        matches!(level, LogLevel::Debug | LogLevel::Trace)
    }
}
