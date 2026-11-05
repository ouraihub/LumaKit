//! 聊天历史、候选、媒体及事件的稳定数据合同。
use super::*;
#[allow(missing_docs)]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct History {
    /// 保存的全部对话。
    pub conversations: Vec<Conversation>,
    /// 当前选中对话的标识。
    pub selected: Option<String>,
    /// 新请求或显式换模型重答所使用的模型引用。
    pub model: Option<ModelRef>,
}
/// 包含草稿和各轮问题的文本对话。
#[allow(missing_docs)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    /// 对话标识。
    pub id: String,
    /// 对话标题；首次发送时由问题开头生成。
    pub title: String,
    /// 尚未发送的输入草稿。
    #[serde(default)]
    pub draft: String,
    /// 按提问顺序保存的轮次。
    pub turns: Vec<Turn>,
}
/// 一次问题及其回答候选。
#[allow(missing_docs)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    /// 问题轮次标识。
    pub id: String,
    /// 用户问题文本。
    pub input: String,
    /// 按执行顺序保存的回答尝试。
    pub attempts: Vec<Attempt>,
    /// 后续上下文使用的候选标识；缺省时兼容旧历史的末次候选规则。
    #[serde(default)]
    pub selected_attempt: Option<String>,
}
/// 一次回答尝试及其来源、输出和状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attempt {
    /// 回答尝试标识。
    pub id: String,
    /// 本次请求使用的模型引用。
    pub model: ModelRef,
    /// 已接收的回答文本；失败或取消时可保留部分内容。
    pub text: String,
    /// 本次尝试的执行状态。
    pub status: TurnStatus,
    /// 失败、取消或中断的说明。
    pub error: Option<String>,
    /// 每轮响应保存的用量、计费快照；旧历史为空。
    #[serde(default)]
    pub rounds: Vec<crate::RoundUsage>,
    /// 已完成请求返回的推理内容块；不混入回答正文。
    #[serde(default)]
    pub reasoning: Vec<serde_json::Value>,
    /// 回答类型；旧历史缺省为普通回复。
    #[serde(default)]
    pub kind: AttemptKind,
    /// 翻译或分支候选的来源候选。
    #[serde(default)]
    pub source_attempt_id: Option<String>,
    /// 候选树中的父候选。
    #[serde(default)]
    pub parent_attempt_id: Option<String>,
    /// 独立保存的搜索来源状态。
    #[serde(default)]
    pub search: SearchState,
    /// 工具调用、审批和结果事件；正文失败时仍保留。
    #[serde(default)]
    pub tool_events: Vec<crate::ToolEvent>,
    /// 生成图片的持久化引用。
    #[serde(default)]
    pub generated: Vec<ImageRef>,
}
/// 回答候选的业务类型。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum AttemptKind {
    /// 普通回答。
    #[default]
    Reply,
    /// 从已有回答生成的翻译。
    Translation,
}

/// 搜索来源的归约状态。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchState {
    /// 当前搜索状态。
    pub status: SearchStatus,
    /// 本次搜索查询。
    pub query: Option<String>,
    /// 搜索提供方。
    pub provider: Option<String>,
    /// 已校验的来源列表。
    pub hits: Vec<crate::search::SearchHit>,
    /// 失败说明。
    pub error: Option<String>,
}

/// 搜索状态。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum SearchStatus {
    /// 尚未发起搜索。
    #[default]
    Idle,
    /// 搜索进行中。
    Searching,
    /// 搜索成功完成。
    Complete,
    /// 搜索失败。
    Failed,
    /// 搜索被取消。
    Cancelled,
}

/// 媒体文件的持久化引用；历史不保存 Base64 正文。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageRef {
    /// 稳定媒体标识。
    pub id: String,
    /// 图片 MIME。
    pub mime: String,
    /// 解码后字节数。
    pub bytes: u64,
    /// 可选宽度。
    pub width: Option<u32>,
    /// 可选高度。
    pub height: Option<u32>,
    /// 数据根内的受控存储键。
    pub storage_key: String,
}

/// 运行事件的稳定业务定位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventIdentity {
    /// 所属对话。
    pub conversation_id: String,
    /// 所属问题。
    pub turn_id: String,
    /// 所属回答候选。
    pub attempt_id: String,
}

/// 文本运行事件；高级事件沿同一身份继续扩展。
#[derive(Debug)]
pub enum ChatEvent {
    /// 增量回答正文。
    Delta {
        /// 事件所属运行。
        identity: EventIdentity,
        /// 增量文本。
        text: String,
    },
    /// 唯一运行终态。
    End {
        /// 事件所属运行。
        identity: EventIdentity,
        /// 运行结果。
        result: Result<crate::AiRun, (TurnStatus, String)>,
    },
}
