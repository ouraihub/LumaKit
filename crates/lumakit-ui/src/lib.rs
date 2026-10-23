//! UI-facing contracts. No egui, Tauri, WebView, or browser dependency.
pub mod ai;
pub mod chat;
use lumakit_ai::{AiHost, ModelRef, RunRequest, ToolSpec};
use lumakit_ports::{LogLevel, LogPort};
use std::sync::Arc;

pub struct UiLogStatus {
    pub level: LogLevel,
    pub available: bool,
    pub path: String,
}

pub trait UiBackend: Send + Sync {
    fn logs(&self) -> Arc<dyn LogPort>;
    fn ai_host(&self) -> Arc<dyn AiHost>;
    fn select_model(&self, request: &RunRequest) -> Result<ModelRef, String>;
    fn tools(&self, request: &RunRequest) -> Vec<ToolSpec>;
}

pub struct UiSession<B: UiBackend> {
    backend: Arc<B>,
}
impl<B: UiBackend> UiSession<B> {
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
    pub fn log_status(&self) -> UiLogStatus {
        let log = self.backend.logs();
        UiLogStatus {
            level: log.level(),
            available: true,
            path: String::new(),
        }
    }
    pub fn available_tools(&self, request: &RunRequest) -> Vec<ToolSpec> {
        self.backend.tools(request)
    }
    pub fn backend(&self) -> &Arc<B> {
        &self.backend
    }
}
