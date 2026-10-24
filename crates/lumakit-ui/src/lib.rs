//! UI-facing contracts. No egui, Tauri, WebView, or browser dependency.
pub mod ai;
pub mod chat;
use lumakit_ai::{AiHost, ModelRef, RunRequest, ToolSpec};
use lumakit_ports::{LogLevel, LogPort};
use std::sync::Arc;

/// 提供给界面的日志状态。
pub struct UiLogStatus {
    /// 当前日志级别。
    pub level: LogLevel,
    /// 日志功能是否可用。
    pub available: bool,
    /// 可展示的日志路径；宿主未提供路径时为空字符串。
    pub path: String,
}

/// 界面会话依赖的宿主能力；实现负责模型选择和工具配置。
pub trait UiBackend: Send + Sync {
    /// 返回共享日志端口。
    fn logs(&self) -> Arc<dyn LogPort>;
    /// 返回共享 AI 宿主。
    fn ai_host(&self) -> Arc<dyn AiHost>;
    /// 为请求选择模型；无法选择时返回可展示的错误说明。
    fn select_model(&self, request: &RunRequest) -> Result<ModelRef, String>;
    /// 返回此请求可使用的工具规格。
    fn tools(&self, request: &RunRequest) -> Vec<ToolSpec>;
}

/// 持有共享后端的轻量界面会话，不自行持久化状态。
pub struct UiSession<B: UiBackend> {
    backend: Arc<B>,
}
impl<B: UiBackend> UiSession<B> {
    /// 使用给定共享后端创建会话。
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
    /// 读取日志级别；当前固定报告可用且路径为空，不探测日志文件。
    pub fn log_status(&self) -> UiLogStatus {
        let log = self.backend.logs();
        UiLogStatus {
            level: log.level(),
            available: true,
            path: String::new(),
        }
    }
    /// 委托后端返回请求可用的工具。
    pub fn available_tools(&self, request: &RunRequest) -> Vec<ToolSpec> {
        self.backend.tools(request)
    }
    /// 借用会话持有的共享后端。
    pub fn backend(&self) -> &Arc<B> {
        &self.backend
    }
}
