//! 与界面无关的聊天会话；历史和运行事件由同一 session 归约。
use crate::{AiConnection, AiError, ChatMessage, ModelRef, history::TurnStatus};
use lumakit_support::{FileLock, new_id};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[path = "chat_store.rs"]
pub(crate) mod store;
#[path = "chat_composer.rs"]
mod composer;
#[path = "chat_generation.rs"]
mod generation;
#[path = "chat_action.rs"]
mod action;
pub use action::{ChatAction, ModelOption};
pub use composer::{Attachment, ComposerSettings, ToolbarTool};
const MAX_TEXT: usize = 1024 * 1024;

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

enum Event {
    Delta(String),
    End(Result<crate::AiRun, (TurnStatus, String)>),
    Generated { images: Vec<ImageRef>, rounds: Vec<crate::RoundUsage> },
    Runtime(crate::RunEvent),
}
struct Job {
    conversation: usize,
    turn: usize,
    attempt: usize,
    identity: EventIdentity,
    terminal: bool,
    cancel: CancellationToken,
    rx: mpsc::Receiver<Event>,
    thread: JoinHandle<()>,
    draft: Option<String>,
}

/// 历史的唯一可变持有者；宿主只通过动作修改，不向渲染器暴露连接和密钥。
pub struct ChatSession {
    connection: rusqlite::Connection,
    _lock: FileLock,
    history: History,
    composer_data: composer::ComposerData,
    default_composer: ComposerSettings,
    media_root: PathBuf,
    job: Option<Job>,
    save_error: bool,
    notice: String,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl ChatSession {
    /// path 是完整的 chat.db 路径；首次打开导入同目录 chat-history.json，旧文件仅作备份。
    /// 同目录独占锁持有到 session 释放；SQLite 是唯一读写数据源。
    pub fn open(path: &Path, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        let lock = FileLock::acquire(&path.with_file_name("chat-history.lock"))
            .map_err(|_| "无法独占聊天记录；请关闭其它使用此数据目录的实例，并检查目录权限。".to_owned())?;
        let (connection, history) = store::open(path)
            .map_err(|error| format!("无法读取或迁移聊天记录：{error}；请检查权限或恢复备份，旧文件未修改。"))?;
        let composer_data = store::load_composer(&connection).map_err(|error| format!("无法读取输入附件和控制：{error}"))?;
        let mut session = Self {
            connection, _lock: lock, history, composer_data, default_composer: ComposerSettings::default(), media_root: path.with_file_name("chat-media"), job: None,
            save_error: false, notice: String::new(), wake,
        };
        for conversation in &mut session.history.conversations {
            if let Some(turn) = conversation.turns.last().filter(|turn| turn.attempts.last().is_some_and(|attempt| attempt.status == TurnStatus::Running)) {
                if let Some(snapshot) = session.composer_data.turns.get(&turn.id).cloned() {
                    if conversation.draft.is_empty() { conversation.draft = turn.input.clone(); }
                    session.composer_data.drafts.insert(conversation.id.clone(), snapshot);
                }
            }
        }
        let recovered = recover(&mut session.history);
        let invalid_selection = session.selected().is_none()
            && !session.history.conversations.is_empty();
        if invalid_selection {
            session.history.selected = session.history.conversations.first().map(|c| c.id.clone());
        }
        if recovered || invalid_selection { session.save(); }
        Ok(session)
    }

    /// 返回聊天历史的只读引用。
    pub fn history(&self) -> &History { &self.history }
    /// 返回当前选中的对话；选择无效时为空。
    pub fn selected(&self) -> Option<&Conversation> {
        self.selected_index().map(|index| &self.history.conversations[index])
    }
    /// 是否仍持有尚未处理完终态的后台请求。
    pub fn busy(&self) -> bool { self.job.is_some() }
    /// 最近一次保存是否失败；失败期间禁止修改和发送。
    pub fn save_failed(&self) -> bool { self.save_error }
    /// 返回当前面向用户的提示。
    pub fn notice(&self) -> &str { &self.notice }

    fn selected_index(&self) -> Option<usize> {
        self.history.conversations.iter().position(|c| Some(&c.id) == self.history.selected.as_ref())
    }
    fn mutable(&self) -> bool { !self.busy() && !self.save_error }
    fn save(&mut self) -> bool {
        self.save_error = store::save_composer(&mut self.connection, &self.history, &self.composer_data).is_err();
        if self.save_error {
            self.notice = "聊天记录保存失败。内存中的问题、部分回答和草稿仍保留；请检查目录权限或磁盘空间后重试保存。保存成功前禁止更改和发送。".into();
        } else {
            self.notice.clear();
        }
        !self.save_error
    }
    /// 空闲时重新保存当前历史，并更新保存失败状态与提示。
    pub fn retry_save(&mut self) {
        if !self.busy() { self.save(); }
    }
    /// 空闲且无保存错误时设置面向用户的提示。
    pub fn report(&mut self, message: String) {
        if self.mutable() { self.notice = message; }
    }
    /// 空闲且无保存错误时新建并选中对话，然后保存历史。
    pub fn new_conversation(&mut self) {
        if !self.mutable() { return; }
        let id = match new_id() {
            Ok(id) => id,
            Err(_) => { self.notice = "无法生成对话标识，请重试。".into(); return; }
        };
        self.history.selected = Some(id.clone());
        self.history.conversations.push(Conversation {
            id, title: "新对话".into(), draft: String::new(), turns: Vec::new(),
        });
        self.save();
    }
    /// 空闲且无保存错误时切换到存在的对话，并保存选择。
    pub fn select_conversation(&mut self, id: &str) {
        if !self.mutable() || !self.history.conversations.iter().any(|c| c.id == id)
            || self.history.selected.as_deref() == Some(id) { return; }
        self.history.selected = Some(id.into());
        self.save();
    }
    /// 空闲且无保存错误时删除指定对话并保存；删除当前项后选择最后一项。
    pub fn delete_conversation(&mut self, id: &str) {
        if !self.mutable() { return; }
        let Some(index) = self.history.conversations.iter().position(|c| c.id == id) else { return; };
        let before_history = self.history.clone();
        let before_composer = self.composer_data.clone();
        self.composer_data.drafts.remove(id);
        for turn in &self.history.conversations[index].turns { self.composer_data.turns.remove(&turn.id); }
        self.history.conversations.remove(index);
        if self.history.selected.as_deref() == Some(id) {
            self.history.selected = self.history.conversations.last().map(|c| c.id.clone());
        }
        if !self.save() { self.history = before_history; self.composer_data = before_composer; }
    }
    /// 编辑指定回答，使用期望正文做 CAS 冲突保护。
    pub fn edit_attempt(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str, expected_text: &str, next_text: &str) -> Result<(), String> {
        if !self.mutable() { return Err("当前聊天不可修改。".into()); }
        if next_text.trim().is_empty() || next_text.len() > MAX_TEXT { return Err("回答正文为空或超过 1 MiB。".into()); }
        let before = self.history.clone();
        let attempt = self.find_attempt_mut(conversation_id, turn_id, attempt_id)?;
        if attempt.text != expected_text { return Err("回答已被其他操作修改。".into()); }
        attempt.text = next_text.into();
        if self.save() { Ok(()) } else { self.history = before; Err(self.notice.clone()) }
    }

    /// 删除指定回答候选，不删除同问题的其它候选。
    pub fn delete_attempt(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str) -> Result<(), String> {
        if !self.mutable() { return Err("当前聊天不可修改。".into()); }
        let before = self.history.clone();
        let turn = self.find_turn_mut(conversation_id, turn_id)?;
        let Some(index) = turn.attempts.iter().position(|attempt| attempt.id == attempt_id) else { return Err("回答不存在。".into()); };
        turn.attempts.remove(index);
        if turn.selected_attempt.as_deref() == Some(attempt_id) { turn.selected_attempt = turn.attempts.last().map(|attempt| attempt.id.clone()); }
        if self.save() { Ok(()) } else { self.history = before; Err(self.notice.clone()) }
    }

    /// 选择指定候选作为该问题的后续上下文来源。
    pub fn select_attempt(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str) -> Result<(), String> {
        if !self.mutable() { return Err("当前聊天不可修改。".into()); }
        let before = self.history.clone();
        let turn = self.find_turn_mut(conversation_id, turn_id)?;
        if !turn.attempts.iter().any(|attempt| attempt.id == attempt_id) { return Err("回答不存在。".into()); }
        turn.selected_attempt = Some(attempt_id.into());
        if self.save() { Ok(()) } else { self.history = before; Err(self.notice.clone()) }
    }

    fn find_turn_mut(&mut self, conversation_id: &str, turn_id: &str) -> Result<&mut Turn, String> {
        let conversation = self.history.conversations.iter_mut().find(|conversation| conversation.id == conversation_id).ok_or("对话不存在。")?;
        conversation.turns.iter_mut().find(|turn| turn.id == turn_id).ok_or("问题不存在。".into())
    }
    fn find_attempt_mut(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str) -> Result<&mut Attempt, String> {
        let turn = self.find_turn_mut(conversation_id, turn_id)?;
        turn.attempts.iter_mut().find(|attempt| attempt.id == attempt_id).ok_or_else(|| "回答不存在。".into())
    }
    /// 复制指定候选之前的线性历史到新对话，并重新生成全部 ID。
    pub fn fork_attempt(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str) -> Result<String, String> {
        if !self.mutable() { return Err("当前聊天不可修改。".into()); }
        let before_history = self.history.clone();
        let before_composer = self.composer_data.clone();
        let mut snapshots = Vec::new();
        let source = self.history.conversations.iter().find(|conversation| conversation.id == conversation_id).ok_or("对话不存在。")?;
        let turn_index = source.turns.iter().position(|turn| turn.id == turn_id).ok_or("问题不存在。")?;
        let _source_attempt = source.turns[turn_index].attempts.iter().find(|attempt| attempt.id == attempt_id).ok_or("回答不存在。")?;
        let mut conversation = Conversation { id: new_id().map_err(|_| "无法生成对话标识。")?, title: format!("{}（分支）", source.title), draft: String::new(), turns: Vec::new() };
        for source_turn in source.turns.iter().take(turn_index + 1) {
            let mut turn = Turn { id: new_id().map_err(|_| "无法生成问题标识。")?, input: source_turn.input.clone(), attempts: Vec::new(), selected_attempt: None };
            for source_attempt in &source_turn.attempts {
                if source_turn.id == turn_id && source_attempt.id != attempt_id { continue; }
                turn.attempts.push(Attempt { id: new_id().map_err(|_| "无法生成回答标识。")?, ..source_attempt.clone() });
            }
            turn.selected_attempt = turn.attempts.last().map(|attempt| attempt.id.clone());
            if let Some(snapshot) = self.composer_data.turns.get(&source_turn.id) { snapshots.push((turn.id.clone(), snapshot.clone())); }
            conversation.turns.push(turn);
        }
        let id = conversation.id.clone();
        self.composer_data.turns.extend(snapshots);
        if let Some(draft) = self.composer_data.drafts.get(conversation_id).cloned() { self.composer_data.drafts.insert(id.clone(), composer::InputSnapshot { attachments: Vec::new(), ..draft }); }
        self.history.selected = Some(id.clone());
        self.history.conversations.push(conversation);
        if self.save() { Ok(id) } else { self.history = before_history; self.composer_data = before_composer; Err(self.notice.clone()) }
    }

    /// 复制完整对话并重新生成全部 ID。
    pub fn copy_conversation(&mut self, conversation_id: &str) -> Result<String, String> {
        if !self.mutable() { return Err("当前聊天不可修改。".into()); }
        let before_history = self.history.clone();
        let before_composer = self.composer_data.clone();
        let source = self.history.conversations.iter().find(|conversation| conversation.id == conversation_id).ok_or("对话不存在。")?.clone();
        let mut copy = Conversation { id: new_id().map_err(|_| "无法生成对话标识。")?, title: format!("{}（副本）", source.title), draft: source.draft, turns: Vec::new() };
        for source_turn in source.turns {
            let mut turn = Turn { id: new_id().map_err(|_| "无法生成问题标识。")?, input: source_turn.input, attempts: Vec::new(), selected_attempt: None };
            for source_attempt in source_turn.attempts { turn.attempts.push(Attempt { id: new_id().map_err(|_| "无法生成回答标识。")?, ..source_attempt }); }
            turn.selected_attempt = turn.attempts.last().map(|attempt| attempt.id.clone());
            if let Some(snapshot) = self.composer_data.turns.get(&source_turn.id).cloned() { self.composer_data.turns.insert(turn.id.clone(), snapshot); }
            copy.turns.push(turn);
        }
        let id = copy.id.clone();
        if let Some(draft) = self.composer_data.drafts.get(conversation_id).cloned() { self.composer_data.drafts.insert(id.clone(), draft); }
        self.history.selected = Some(id.clone());
        self.history.conversations.push(copy);
        if self.save() { Ok(id) } else { self.history = before_history; self.composer_data = before_composer; Err(self.notice.clone()) }
    }
    /// 将已完成回答翻译为新候选；源回答不变。
    pub fn translate(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str, language: &str, connection: AiConnection) -> Result<(), String> {
        if !self.mutable() { return Err("当前聊天不可修改。".into()); }
        if language.trim().is_empty() || language.chars().count() > 80 || language.chars().any(char::is_control) { return Err("目标语言无效。".into()); }
        let source = self.history.conversations.iter().find(|conversation| conversation.id == conversation_id).and_then(|conversation| conversation.turns.iter().find(|turn| turn.id == turn_id)).and_then(|turn| turn.attempts.iter().find(|attempt| attempt.id == attempt_id)).ok_or("回答不存在。")?;
        if source.status != TurnStatus::Completed { return Err("只能翻译已完成的回答。".into()); }
        if connection.protocol != source.model.protocol { return Err("翻译连接协议不匹配。".into()); }
        let messages = vec![ChatMessage::User(format!("请将以下内容翻译为 {language}，只返回译文：\n\n{}", source.text))];
        let model = ModelRef { provider: source.model.provider.clone(), endpoint: source.model.endpoint.clone(), model: connection.model.clone(), protocol: connection.protocol };
        let attempt = Attempt { id: new_id().map_err(|_| "无法生成翻译回答标识。")?, model, text: String::new(), status: TurnStatus::Running, error: None, rounds: Vec::new(), reasoning: Vec::new(), kind: AttemptKind::Translation, source_attempt_id: Some(attempt_id.into()), parent_attempt_id: Some(attempt_id.into()), search: SearchState::default(), tool_events: Vec::new(), generated: Vec::new() };
        let ci = self.history.conversations.iter().position(|conversation| conversation.id == conversation_id).unwrap();
        let ti = self.history.conversations[ci].turns.iter().position(|turn| turn.id == turn_id).unwrap();
        let ai = self.history.conversations[ci].turns[ti].attempts.len();
        self.history.conversations[ci].turns[ti].attempts.push(attempt);
        if !self.save() { self.history.conversations[ci].turns[ti].attempts.pop(); return Err(self.notice.clone()); }
        let cancel = CancellationToken::new(); let worker_cancel = cancel.clone(); let wake = self.wake.clone(); let (tx, rx) = mpsc::sync_channel(64);
        let worker = thread::Builder::new().name("lumakit-translate".into()).spawn(move || { let result = run_worker(connection, messages, worker_cancel, &tx, &wake); let _ = tx.send(Event::End(result)); wake(); }).map_err(|_| "无法启动翻译线程。".to_owned())?;
        self.job = Some(Job { conversation: ci, turn: ti, attempt: ai, identity: EventIdentity { conversation_id: conversation_id.into(), turn_id: turn_id.into(), attempt_id: self.history.conversations[ci].turns[ti].attempts[ai].id.clone() }, terminal: false, cancel, rx, thread: worker, draft: None });
        Ok(())
    }
    /// 空闲且无保存错误时更新并保存模型选择。
    pub fn select_model(&mut self, model: ModelRef) {
        if !self.mutable() { return; }
        self.history.model = Some(model);
        self.save();
    }
    /// 标记指定回答开始联网搜索。
    pub fn search_start(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str, query: &str, provider: &str) -> Result<(), String> {
        if !self.mutable() { return Err("当前聊天不可修改。".into()); }
        if query.trim().is_empty() || provider.trim().is_empty() { return Err("搜索查询或提供方为空。".into()); }
        let before = self.history.clone();
        let attempt = self.find_attempt_mut(conversation_id, turn_id, attempt_id)?;
        attempt.search = SearchState { status: SearchStatus::Searching, query: Some(query.into()), provider: Some(provider.into()), hits: Vec::new(), error: None };
        if self.save() { Ok(()) } else { self.history = before; Err(self.notice.clone()) }
    }

    /// 持久化搜索结果；来源保持在正文之外。
    pub fn search_result(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str, result: crate::search::SearchResult) -> Result<(), String> {
        if !self.mutable() { return Err("当前聊天不可修改。".into()); }
        let mut result = crate::search::process_result(result, &crate::search::SearchPreferences::default());
        for hit in &result.hits { crate::search::validate_search_url(&hit.url).map_err(|_| "搜索来源 URL 无效。".to_owned())?; }
        let before = self.history.clone();
        let attempt = self.find_attempt_mut(conversation_id, turn_id, attempt_id)?;
        attempt.search = SearchState { status: SearchStatus::Complete, query: Some(result.query), provider: Some(result.provider), hits: std::mem::take(&mut result.hits), error: None };
        if self.save() { Ok(()) } else { self.history = before; Err(self.notice.clone()) }
    }

    /// 记录搜索失败，不伪装成无结果。
    pub fn search_failed(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str, error: &str) -> Result<(), String> {
        self.finish_search(conversation_id, turn_id, attempt_id, SearchStatus::Failed, Some(error.into()))
    }

    /// 记录搜索取消，不伪装成无结果。
    pub fn search_cancelled(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str) -> Result<(), String> {
        self.finish_search(conversation_id, turn_id, attempt_id, SearchStatus::Cancelled, None)
    }

    fn finish_search(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str, status: SearchStatus, error: Option<String>) -> Result<(), String> {
        if !self.mutable() { return Err("当前聊天不可修改。".into()); }
        let before = self.history.clone();
        let attempt = self.find_attempt_mut(conversation_id, turn_id, attempt_id)?;
        attempt.search.status = status;
        attempt.search.error = error;
        if self.save() { Ok(()) } else { self.history = before; Err(self.notice.clone()) }
    }
    /// 记录模型发起的工具调用；调用状态与回答正文分开保存。
    pub fn tool_start(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str, call: crate::ToolCall) -> Result<(), String> {
        if !self.mutable() { return Err("当前聊天不可修改。".into()); }
        if call.id.trim().is_empty() || call.name.trim().is_empty() || !call.arguments.is_object() { return Err("工具调用无效。".into()); }
        let before = self.history.clone();
        let attempt = self.find_attempt_mut(conversation_id, turn_id, attempt_id)?;
        if attempt.tool_events.iter().any(|event| event.id == call.id) { return Err("工具调用标识重复。".into()); }
        attempt.tool_events.push(crate::ToolEvent { id: call.id, name: call.name, arguments: call.arguments, approval: None, result: None, is_error: false, error: None });
        if self.save() { Ok(()) } else { self.history = before; Err(self.notice.clone()) }
    }
    /// 保存工具授权决定。
    pub fn tool_approval(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str, call_id: &str, approval: crate::Approval) -> Result<(), String> {
        self.update_tool_event(conversation_id, turn_id, attempt_id, call_id, |event| event.approval = Some(approval))
    }
    /// 保存工具输出，包括工具自身报告的业务错误。
    pub fn tool_result(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str, call_id: &str, output: crate::ToolOutput) -> Result<(), String> {
        self.update_tool_event(conversation_id, turn_id, attempt_id, call_id, |event| { event.result = Some(output.text); event.is_error = output.is_error; event.error = None; })
    }
    /// 保存未知工具、拒绝或执行失败的错误，不伪装成成功结果。
    pub fn tool_error(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str, call_id: &str, error: &str) -> Result<(), String> {
        if error.trim().is_empty() { return Err("工具错误为空。".into()); }
        self.update_tool_event(conversation_id, turn_id, attempt_id, call_id, |event| { event.error = Some(error.into()); event.is_error = true; })
    }
    fn update_tool_event(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str, call_id: &str, update: impl FnOnce(&mut crate::ToolEvent)) -> Result<(), String> {
        if !self.mutable() { return Err("当前聊天不可修改。".into()); }
        let before = self.history.clone();
        let attempt = self.find_attempt_mut(conversation_id, turn_id, attempt_id)?;
        let event = attempt.tool_events.iter_mut().find(|event| event.id == call_id).ok_or("工具调用不存在。")?;
        update(event);
        if self.save() { Ok(()) } else { self.history = before; Err(self.notice.clone()) }
    }
    /// 空闲且无保存错误时更新当前对话草稿并保存。
    pub fn set_draft(&mut self, text: String) {
        if !self.mutable() { return; }
        if let Some(index) = self.selected_index() {
            self.history.conversations[index].draft = text;
            self.save();
        }
    }
    /// 普通重试锁定末次尝试的来源，不受当前模型下拉选择影响。
    pub fn request_model(&self, retry: bool) -> Result<ModelRef, String> {
        if self.busy() { return Err("请先等待当前请求结束。".into()); }
        if self.save_error { return Err("请先重试保存聊天记录。".into()); }
        let conversation = self.selected().ok_or("请先新建对话。")?;
        if retry {
            conversation.turns.last().and_then(|t| t.attempts.last())
                .map(|a| a.model.clone()).ok_or_else(|| "没有可重试的问题。".into())
        } else {
            self.history.model.clone().ok_or_else(|| "请先选择已保存且已启用的聊天模型。".into())
        }
    }
    /// 新请求读取草稿，重试锁定原问题的绘图开关与模型；旧直接绘图记录沿用原来源。
    pub fn request_painting_model(&self, retry: bool) -> Result<Option<ModelRef>, String> {
        self.request_model(retry)?;
        let conversation = self.selected().ok_or("请先新建对话。")?;
        let settings = if retry {
            let turn = conversation.turns.last().ok_or("没有可重试的问题。")?;
            self.composer_data.turns.get(&turn.id).map(|snapshot| &snapshot.settings)
        } else { Some(self.composer()) };
        let Some(settings) = settings.filter(|settings| settings.image_mode) else { return Ok(None); };
        let legacy_model = if retry { conversation.turns.last().and_then(|turn| turn.attempts.first()).map(|attempt| attempt.model.clone()) } else { None };
        settings.image_model.clone().or(legacy_model).map(Some).ok_or_else(|| "请先选择已保存的绘图模型。".into())
    }
    /// 使用给定连接发送当前草稿；先保存请求记录，再启动后台流式请求。
    ///
    /// 忙碌或保存失败时不操作；校验失败时保留草稿并设置提示。
    pub fn send(&mut self, connection: AiConnection) { self.start(connection, None, false, false); }
    /// 使用末次尝试的模型重答最后一个问题，保留已有尝试记录。
    ///
    /// 给定连接必须匹配原模型及协议；忙碌或保存失败时不操作。
    pub fn retry(&mut self, connection: AiConnection) { self.start(connection, None, true, false); }
    /// 显式换模型重答：宿主先 select_model，再按 history.model 解析真实连接。
    pub fn retry_with(&mut self, connection: AiConnection) { self.start(connection, None, true, true); }
    /// 发送聊天请求，模型自行决定是否调用已选择的绘图工具。
    pub fn send_with_painting(&mut self, chat: AiConnection, painting: AiConnection) { self.start(chat, Some(painting), false, false); }
    /// 使用原问题的聊天来源、绘图来源与输入控制重试。
    pub fn retry_with_painting(&mut self, chat: AiConnection, painting: AiConnection) { self.start(chat, Some(painting), true, false); }
    /// 使用已 select_model 的聊天来源重试；绘图模型与控制仍取原问题快照，可迁移旧直接绘图失败。
    pub fn retry_with_model_and_painting(&mut self, chat: AiConnection, painting: AiConnection) { self.start(chat, Some(painting), true, true); }

    fn start(&mut self, connection: AiConnection, painting: Option<AiConnection>, retry: bool, override_model: bool) {
        if !self.mutable() { return; }
        let prepared = (|| {
            let model = self.request_model(retry && !override_model)?;
            if connection.model != model.model || connection.protocol != model.protocol {
                return Err("连接与请求模型不匹配，请重新选择已保存的模型。".to_owned());
            }
            let ci = self.selected_index().ok_or("请先新建对话。")?;
            let conversation = &self.history.conversations[ci];
            let input = if retry {
                conversation.turns.last().ok_or("没有可重试的问题。")?.input.clone()
            } else {
                conversation.draft.trim().to_owned()
            };
            let ti = if retry { conversation.turns.len() - 1 } else { conversation.turns.len() };
            let mut snapshot = if retry { self.composer_data.turns.get(&conversation.turns[ti].id).cloned().unwrap_or_default() }
                else { self.composer_data.drafts.get(&conversation.id).cloned().unwrap_or_default() };
            if snapshot.settings.image_mode {
                let painting_model = self.request_painting_model(retry)?.ok_or("请先配置绘图模型。")?;
                let painting = painting.as_ref().ok_or("绘图已启用：宿主需同时解析聊天与绘图模型，并调用 send_with_painting / retry_with_painting；不再直接发送原始提示到图片接口。")?;
                if connection.settings.supports_tools == Some(false) { return Err("所选聊天模型声明不支持工具，请更换聊天模型或关闭生成图片。".into()); }
                if painting.model != painting_model.model || painting.protocol != painting_model.protocol { return Err("绘图连接与原问题选择的模型不匹配。".into()); }
                generation::validate(painting)?;
                snapshot.settings.image_model = Some(painting_model);
            } else if painting.is_some() { return Err("本问题未启用绘图工具，请使用普通聊天入口。".into()); }
            let mut connection = connection;
            if connection.settings.model_type != crate::model_config::ModelType::Chat { return Err("所选模型不是聊天模型。".into()); }
            if snapshot.settings.web_search && connection.protocol == crate::Protocol::OpenAiChat { return Err("Chat Completions 不支持原生联网搜索，请选择 Responses 或 Anthropic 线路。".into()); }
            connection.settings.native_web_search = snapshot.settings.web_search;
            if let Some(effort) = snapshot.settings.reasoning { connection = connection.with_reasoning_effort(effort)?; }
            connection.settings.validate(connection.protocol)?;
            let messages = composer::context(&conversation.turns[..ti], &input, &snapshot.attachments, &self.composer_data, connection.settings.supports_vision, &self.media_root)?;
            let attempt = Attempt {
                id: new_id().map_err(|_| "无法生成回答标识，请重试。".to_owned())?,
                model, text: String::new(), status: TurnStatus::Running, error: None, rounds: Vec::new(), reasoning: Vec::new(),
                kind: AttemptKind::Reply, source_attempt_id: None, parent_attempt_id: None,
                search: SearchState::default(), tool_events: Vec::new(), generated: Vec::new(),
            };
            let turn_id = if retry { None } else {
                Some(new_id().map_err(|_| "无法生成问题标识，请重试。".to_owned())?)
            };
            Ok((ci, ti, input, messages, attempt, turn_id, snapshot, connection))
        })();
        let (ci, ti, input, messages, attempt, turn_id, snapshot, connection) = match prepared {
            Ok(value) => value,
            Err(error) => { self.notice = error; return; }
        };
        let previous_composer = self.composer_data.clone();
        let image_mode = snapshot.settings.image_mode;
        let conversation = &mut self.history.conversations[ci];
        let previous_title = if !retry && conversation.turns.is_empty() {
            Some(std::mem::replace(&mut conversation.title,
                input.chars().take(28).map(|c| if c.is_control() { ' ' } else { c }).collect()))
        } else { None };
        let draft = if retry { None } else { Some(std::mem::take(&mut conversation.draft)) };
        if let Some(id) = turn_id.clone() {
            conversation.turns.push(Turn { id, input, attempts: Vec::new(), selected_attempt: None });
        }
        let ai = conversation.turns[ti].attempts.len();
        conversation.turns[ti].attempts.push(attempt);
        self.composer_data.turns.insert(self.history.conversations[ci].turns[ti].id.clone(), snapshot);
        if !retry {
            if let Some(draft) = self.composer_data.drafts.get_mut(&self.history.conversations[ci].id) { draft.attachments.clear(); }
        }
        // 接受请求前必须落盘；失败时完整撤销新增记录，不能留下重复问题或 Running。
        if !self.save() {
            let conversation = &mut self.history.conversations[ci];
            if retry { conversation.turns[ti].attempts.pop(); }
            else { conversation.turns.pop(); }
            if let Some(title) = previous_title { conversation.title = title; }
            if let Some(draft) = draft { conversation.draft = draft; }
            self.composer_data = previous_composer;
            return;
        }
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let wake = self.wake.clone();
        let identity = EventIdentity {
            conversation_id: self.history.conversations[ci].id.clone(),
            turn_id: self.history.conversations[ci].turns[ti].id.clone(),
            attempt_id: self.history.conversations[ci].turns[ti].attempts[ai].id.clone(),
        };
        let (tx, rx) = mpsc::sync_channel(64);
        let media_root = self.media_root.clone();
        let span = tracing::info_span!("ai.request", conversation_id = %identity.conversation_id, turn_id = %identity.turn_id, attempt_id = %identity.attempt_id, model = %connection.model, protocol = ?connection.protocol, image_mode);
        let worker = thread::Builder::new().name("lumakit-chat".into()).spawn(move || {
            // worker 独占同步运行时；span 仅在线程入口进入，不跨异步任务持有 guard。
            let _entered = span.enter();
            let result = run_session_worker(connection, painting, messages, worker_cancel, &tx, &wake, media_root);
            match &result {
                Ok(_) => tracing::info!(event = "ai.request.completed", "AI request completed"),
                Err((TurnStatus::Cancelled, _)) => tracing::info!(event = "ai.request.cancelled", "AI request cancelled"),
                Err(_) => tracing::error!(event = "ai.request.failed", "AI 请求失败；详情仅保存到会话，不记录私有正文"),
            }
            let _ = tx.send(Event::End(result));
            wake();
        });
        match worker {
            Ok(thread) => self.job = Some(Job { conversation: ci, turn: ti, attempt: ai, identity, terminal: false, cancel, rx, thread, draft }),
            Err(_) => {
                let attempt = &mut self.history.conversations[ci].turns[ti].attempts[ai];
                attempt.status = TurnStatus::Failed;
                attempt.error = Some("无法启动聊天线程，请重试。".into());
                self.restore_draft(ci, ti, draft);
                self.save();
            }
        }
    }

    fn restore_draft(&mut self, ci: usize, ti: usize, draft: Option<String>) {
        let Some(draft) = draft else { return; };
        let conversation = &mut self.history.conversations[ci];
        conversation.draft = draft;
        if let Some(snapshot) = self.composer_data.turns.get(&conversation.turns[ti].id).cloned() { self.composer_data.drafts.insert(conversation.id.clone(), snapshot); }
    }

    fn active_attempt(&mut self) -> Option<&mut Attempt> {
        let job = self.job.as_ref()?;
        self.history.conversations.get_mut(job.conversation)?.turns.get_mut(job.turn)?.attempts.get_mut(job.attempt)
    }
    fn active_identity_matches(&self) -> bool {
        let Some(job) = &self.job else { return false; };
        self.history.conversations.get(job.conversation).and_then(|conversation| conversation.turns.get(job.turn)).and_then(|turn| turn.attempts.get(job.attempt)).is_some_and(|attempt| attempt.id == job.identity.attempt_id)
    }
    /// 每次最多处理 128 个事件；余下事件通过 wake 继续驱动宿主。
    pub fn poll(&mut self) {
        let mut ended = false;
        let mut processed = 0;
        for _ in 0..128 {
            let event = match self.job.as_ref().map(|job| job.rx.try_recv()) {
                Some(Ok(event)) => event,
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    if self.active_identity_matches() && let Some(attempt) = self.active_attempt() {
                        attempt.status = TurnStatus::Failed;
                        attempt.error = Some("聊天线程意外结束；已保留部分回答，可重试。".into());
                    }
                    ended = true;
                    break;
                }
                _ => break,
            };
            processed += 1;
            if !self.active_identity_matches() { continue; }
            let terminal = self.job.as_ref().is_some_and(|job| job.terminal);
            if terminal { continue; }
            let persist = !matches!(&event, Event::Delta(_) | Event::End(_));
            if let Some(attempt) = self.active_attempt() {
                match event {
                    Event::Delta(text) => { if text.len() <= MAX_TEXT.saturating_sub(attempt.text.len()) { attempt.text.push_str(&text); } }
                    Event::Generated { images, rounds } => { attempt.generated.extend(images); attempt.rounds.extend(rounds); }
                    Event::Runtime(crate::RunEvent::Tool(event)) => {
                        if let Some(previous) = attempt.tool_events.iter_mut().find(|previous| previous.id == event.id) { *previous = event; }
                        else { attempt.tool_events.push(event); }
                    }
                    Event::Runtime(crate::RunEvent::Round { usage, reasoning }) => { attempt.rounds.extend(usage); attempt.reasoning.extend(reasoning); }
                    Event::End(result) => {
                        let attempt_id = self.job.as_ref().map(|job| job.attempt).and_then(|attempt| self.history.conversations.get(self.job.as_ref()?.conversation)?.turns.get(self.job.as_ref()?.turn)?.attempts.get(attempt)).map(|attempt| attempt.id.clone());
                        if let Some(attempt) = self.active_attempt() {
                            match result {
                                Ok(result) => { attempt.text = result.text; attempt.status = TurnStatus::Completed; attempt.error = None; }
                                Err((status, error)) => { attempt.status = status; attempt.error = Some(error); }
                            }
                        }
                        if let (Some(job), Some(attempt_id)) = (&self.job, attempt_id) {
                            if let Some(turn) = self.history.conversations.get_mut(job.conversation).and_then(|conversation| conversation.turns.get_mut(job.turn)) { turn.selected_attempt = Some(attempt_id); }
                        }
                        if let Some(job) = &mut self.job { job.terminal = true; }
                        ended = true;
                    }
                }
            }
            // 工具与媒体先落盘，后续聊天失败或进程重启也不会丢失已完成的绘图。
            if persist { self.save(); }
            if ended { break; }
        }
        if ended {
            if let Some(job) = self.job.take() {
                if self.history.conversations[job.conversation].turns[job.turn].attempts[job.attempt].status != TurnStatus::Completed { self.restore_draft(job.conversation, job.turn, job.draft); }
                let _ = job.thread.join();
            }
            self.save();
        } else if processed == 128 && self.busy() { (self.wake)(); }
    }
    /// 请求取消当前后台任务；终态及部分输出由后续轮询处理。
    pub fn cancel(&mut self) {
        if let Some(job) = &self.job { job.cancel.cancel(); }
    }
    /// 宿主拦截关闭：取消后等待终态，再确认草稿和回答全部保存。
    pub fn can_close(&mut self) -> bool {
        if self.busy() {
            self.cancel();
            self.notice = "正在停止请求并保存记录，请稍后再次关闭窗口。".into();
            return false;
        }
        if self.save_error { return false; }
        self.save()
    }
}

impl Drop for ChatSession {
    fn drop(&mut self) {
        self.cancel();
        // 有界队列必须边排空边等待，不能直接 join 被背压阻塞的 worker。
        while self.busy() {
            self.poll();
            if self.busy() { thread::sleep(Duration::from_millis(5)); }
        }
        self.save();
    }
}


fn run_worker(
    connection: AiConnection,
    messages: Vec<ChatMessage>,
    cancel: CancellationToken,
    tx: &mpsc::SyncSender<Event>,
    wake: &Arc<dyn Fn() + Send + Sync>,
) -> Result<crate::AiRun, (TurnStatus, String)> {
    run_session_worker(connection, None, messages, cancel, tx, wake, PathBuf::new())
}

fn run_session_worker(
    connection: AiConnection,
    painting: Option<AiConnection>,
    messages: Vec<ChatMessage>,
    cancel: CancellationToken,
    tx: &mpsc::SyncSender<Event>,
    wake: &Arc<dyn Fn() + Send + Sync>,
    media_root: PathBuf,
) -> Result<crate::AiRun, (TurnStatus, String)> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()
        .map_err(|_| (TurnStatus::Failed, "无法启动聊天网络运行时，请重试。".into()))?;
    let mut bytes = 0usize;
    let mut overflow = false;
    let host = generation::PaintingHost {
        model: ModelRef { provider: "snapshot".into(), endpoint: "snapshot".into(), model: connection.model.clone(), protocol: connection.protocol },
        chat: connection, painting, root: media_root, tx: tx.clone(), wake: wake.clone(), executions: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    let engine = crate::AiRuntime::new(Arc::new(host)).map_err(|error| (TurnStatus::Failed, error.to_string()))?;
    let result = runtime.block_on(engine.run_with_history(crate::RunRequest { input: String::new(), model: None, purpose: "chat".into() }, messages, cancel.clone(), |text| {
        if overflow { return; }
        if text.len() > MAX_TEXT.saturating_sub(bytes) {
            overflow = true;
            cancel.cancel();
            return;
        }
        bytes += text.len();
        let mut remaining = text.as_str();
        while !remaining.is_empty() {
            let mut end = remaining.len().min(16 * 1024);
            while !remaining.is_char_boundary(end) { end -= 1; }
            // 有界背压不丢字；Drop 会继续排空，因此取消后也不会卡在 join。
            if tx.send(Event::Delta(remaining[..end].to_owned())).is_err() {
                cancel.cancel();
                return;
            }
            wake();
            remaining = &remaining[end..];
        }
    }, |event| { if tx.send(Event::Runtime(event)).is_err() { cancel.cancel(); } wake(); }));
    if overflow {
        return Err((TurnStatus::Failed, "回答超过 1 MiB，已停止；保留已有内容。".into()));
    }
    match result {
        Ok(result) if result.text.trim().is_empty() => Err((TurnStatus::Failed, "服务返回了空回答，请重试；空回答不会用于后续上下文。".into())),
        Ok(result) if result.text.len() <= MAX_TEXT => Ok(result),
        Ok(_) => Err((TurnStatus::Failed, "回答超过 1 MiB，已停止。".into())),
        Err(AiError::Cancelled) => Err((TurnStatus::Cancelled, "已停止；部分回答不会用于后续上下文。".into())),
        Err(error) => Err((TurnStatus::Failed, format!("请求失败：{error}"))),
    }
}

#[cfg(test)]
fn context(turns: &[Turn], input: &str) -> Result<Vec<ChatMessage>, String> {
    composer::context(turns, input, &[], &composer::ComposerData::default(), false, Path::new(""))
}

fn recover(history: &mut History) -> bool {
    let mut changed = false;
    for conversation in &mut history.conversations {
        for turn in &mut conversation.turns {
            for attempt in &mut turn.attempts {
                if attempt.status == TurnStatus::Running {
                    attempt.status = TurnStatus::Failed;
                    attempt.error = Some("上次请求被中断；已保留部分回答，可显式重试。".into());
                    changed = true;
                }
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt(status: TurnStatus, text: &str) -> Attempt {
        Attempt {
            id: "a".into(),
            model: ModelRef { provider: "p".into(), endpoint: "e".into(), model: "m".into(), protocol: crate::Protocol::OpenAiChat },
            text: text.into(), status, error: None, rounds: Vec::new(), kind: AttemptKind::Reply,
            source_attempt_id: None, parent_attempt_id: None, search: SearchState::default(), tool_events: Vec::new(), reasoning: Vec::new(), generated: Vec::new(),
        }
    }
    fn turn(input: &str, attempts: Vec<Attempt>) -> Turn {
        Turn { id: input.into(), input: input.into(), attempts, selected_attempt: None }
    }
    #[test]
    fn context_uses_only_last_completed_attempt_and_enforces_message_bounds() {
        let turns = vec![
            turn("第一问", vec![attempt(TurnStatus::Completed, "旧答案"), attempt(TurnStatus::Completed, "新答案")]),
            turn("第二问", vec![attempt(TurnStatus::Completed, "不再使用"), attempt(TurnStatus::Failed, "部分内容")]),
            turn("第三问", vec![attempt(TurnStatus::Cancelled, "部分内容")]),
            turn("第四问", vec![attempt(TurnStatus::Running, "部分内容")]),
        ];
        let messages = context(&turns, "现在的问题").unwrap();
        let text: Vec<_> = messages.into_iter().map(|message| match message {
            ChatMessage::User(text) => format!("U:{text}"),
            ChatMessage::UserImages { text, .. } => format!("U:{text}"),
            ChatMessage::Assistant(text) => format!("A:{text}"),
        }).collect();
        assert_eq!(text, ["U:第一问", "A:新答案", "U:现在的问题"]);
        assert!(context(&[], "  \n").is_err());
        assert!(context(&[], &"x".repeat(MAX_TEXT)).is_ok());
        assert!(context(&[], &"x".repeat(MAX_TEXT + 1)).is_err());
        let large = vec![turn("q", vec![attempt(TurnStatus::Completed, &"x".repeat(MAX_TEXT - 2))])];
        assert!(context(&large, "n").is_ok());
        assert!(context(&large, "nn").is_err());
        let empty = vec![turn("q", vec![attempt(TurnStatus::Completed, " \n")])];
        assert!(context(&empty, "n").is_err());
    }
    #[test]
    fn event_identity_rejects_wrong_attempt() {
        let identity = EventIdentity { conversation_id: "c".into(), turn_id: "t".into(), attempt_id: "a".into() };
        assert_ne!(identity, EventIdentity { conversation_id: "c".into(), turn_id: "t".into(), attempt_id: "late".into() });
    }
    #[test]
    fn terminal_event_is_only_applied_once() {
        let mut attempt = attempt(TurnStatus::Running, "部分");
        attempt.status = TurnStatus::Completed;
        attempt.text = "第一次终态".into();
        let snapshot = attempt.text.clone();
        if attempt.status != TurnStatus::Running { /* session guard ignores late End */ }
        assert_eq!(attempt.text, snapshot);
    }
}
