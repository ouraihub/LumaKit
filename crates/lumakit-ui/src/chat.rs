//! 可复用纯文本聊天动作；不暴露密钥、连接或持久化路径。
use lumakit_ai::ModelRef;

/// 模型目录中可公开展示的来源信息。
#[derive(Debug, Clone)]
pub struct ModelOption {
    pub model: ModelRef,
    pub name: String,
    pub provider: String,
    pub endpoint: String,
}

/// 绘制后交给宿主执行；渲染器不直接修改会话。
#[derive(Debug, Clone)]
pub enum ChatAction {
    New,
    Select(String),
    Delete(String),
    SelectModel(ModelRef),
    Draft(String),
    Send,
    Retry,
    RetryWith(ModelRef),
    Stop,
    Save,
    OpenSettings,
}
