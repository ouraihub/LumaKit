//! 渲染器交给宿主执行的聊天动作与可展示的模型来源；不暴露密钥、连接或持久化路径。
use super::{ComposerSettings, ToolbarTool};
use crate::ModelRef;

/// 模型目录中可公开展示的来源信息。
#[derive(Debug, Clone)]
pub struct ModelOption {
    /// 用于执行请求的完整模型引用。
    pub model: ModelRef,
    /// 模型显示名称。
    pub name: String,
    /// 提供方显示名称。
    pub provider: String,
    /// 端点显示名称。
    pub endpoint: String,
    /// 已保存的模型能力、分组和推理配置；不含凭据。
    pub settings: crate::model_config::ModelSettings,
    /// 仅在已存储凭据存在时为 true，不代表订阅或认证声明。
    pub has_key: bool,
}

/// 绘制后交给宿主执行；渲染器不直接修改会话。
/// 宿主执行的聊天动作。
#[allow(missing_docs)]
#[derive(Debug, Clone)]
pub enum ChatAction {
    /// 创建新对话。
    New,
    /// 按对话标识切换当前对话。
    Select(String),
    /// 按对话标识删除对话。
    Delete(String),
    /// 选择后续请求使用的模型。
    SelectModel(ModelRef),
    /// 原子选择模型与推理强度；None 不发送推理参数，使用服务商默认行为。
    SelectModelPair {
        model: ModelRef,
        effort: Option<crate::model_config::ReasoningEffort>,
    },
    /// 按完整模型身份切换收藏。
    ToggleFavorite(ModelRef),
    /// 保存仅供新对话使用的默认模型与推理强度。
    SetDefaultModelPair {
        model: ModelRef,
        effort: Option<crate::model_config::ReasoningEffort>,
    },
    /// false 重读本地目录，true 由宿主从端点刷新目录。
    RefreshModels { force: bool },
    /// 将当前草稿替换为给定文本。
    Draft(String),
    /// 保存输入区的搜索、推理和图片模式设置。
    SetComposer(ComposerSettings),
    /// 保存所有对话共用的工具栏显示偏好，不改变请求设置。
    SetToolbar(Vec<ToolbarTool>),
    /// 由宿主选择本地文件，再交给会话校验和保存。
    UploadAttachment,
    /// 移除尚未发送的附件。
    RemoveAttachment(String),
    /// 显式打开已保存的生成图片。
    OpenMedia(String),
    /// 发送当前草稿。
    Send,
    /// 使用当前模型重试最后一轮回答。
    Retry,
    /// 使用给定模型重试最后一轮回答。
    RetryWith(ModelRef),
    /// 请求停止正在运行的回答。
    Stop,
    /// 请求宿主保存原始 Markdown 到笔记。
    SaveNote {
        attempt_id: String,
        markdown: String,
    },
    /// 请求宿主插入原始 Markdown。
    InsertMarkdown {
        attempt_id: String,
        markdown: String,
    },
    /// 请求宿主导出原始 Markdown。
    ExportMarkdown {
        attempt_id: String,
        markdown: String,
    },
    /// 开始编辑指定回答。
    EditAttempt {
        conversation_id: String,
        turn_id: String,
        attempt_id: String,
    },
    /// 提交回答编辑并使用原文做 CAS。
    SubmitEdit {
        conversation_id: String,
        turn_id: String,
        attempt_id: String,
        expected_text: String,
        next_text: String,
    },
    /// 删除指定回答候选。
    DeleteAttempt {
        conversation_id: String,
        turn_id: String,
        attempt_id: String,
    },
    /// 从指定回答建立分支对话。
    ForkAttempt {
        conversation_id: String,
        turn_id: String,
        attempt_id: String,
    },
    /// 复制完整对话。
    CopyConversation { conversation_id: String },
    /// 选择指定回答作为上下文来源。
    SelectAttempt {
        conversation_id: String,
        turn_id: String,
        attempt_id: String,
    },
    /// 翻译指定回答。
    TranslateAttempt {
        conversation_id: String,
        turn_id: String,
        attempt_id: String,
        language: String,
    },
    /// 提交翻译请求并指定模型。
    SubmitTranslation {
        conversation_id: String,
        turn_id: String,
        attempt_id: String,
        language: String,
        model: ModelRef,
    },
    /// 请求保存会话，可用于重试失败的保存。
    Save,
    /// 请求宿主打开模型设置。
    OpenSettings,
}
