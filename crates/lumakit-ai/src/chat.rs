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

#[path = "chat_action.rs"]
mod action;
#[path = "chat_activity.rs"]
mod activity;
#[path = "chat_composer.rs"]
mod composer;
#[path = "chat_generation.rs"]
mod generation;
#[path = "chat_history.rs"]
mod history;
#[path = "chat_model_picker.rs"]
mod model_picker;
#[path = "chat_request.rs"]
mod request;
#[path = "chat_runtime.rs"]
mod runtime;
#[path = "chat_store.rs"]
pub(crate) mod store;
#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
#[path = "chat_types.rs"]
mod types;
#[path = "chat_usage.rs"]
mod usage;
pub use action::{ChatAction, ModelOption};
pub use composer::{Attachment, ComposerSettings, ToolbarTool};
pub use model_picker::{ModelPair, ModelPickerPreferences};
pub use types::{
    Attempt, AttemptKind, ChatEvent, Conversation, EventIdentity, History, ImageRef, SearchState,
    SearchStatus, Turn,
};
const MAX_TEXT: usize = 1024 * 1024;

#[derive(Debug)]
enum Event {
    Delta(String),
    End(Result<crate::AiRun, (TurnStatus, String)>),
    Generated {
        images: Vec<ImageRef>,
        rounds: Vec<crate::RoundUsage>,
    },
    Runtime(crate::RunEvent),
}
#[derive(Debug)]
struct Envelope {
    generation: u64,
    event: Event,
}
#[derive(Clone)]
struct EventSender {
    generation: u64,
    tx: mpsc::SyncSender<Envelope>,
}
impl EventSender {
    fn send(&self, event: Event) -> Result<(), mpsc::SendError<Envelope>> {
        self.tx.send(Envelope {
            generation: self.generation,
            event,
        })
    }
}
struct Job {
    conversation: usize,
    turn: usize,
    attempt: usize,
    identity: EventIdentity,
    generation: u64,
    terminal: bool,
    cancel: CancellationToken,
    rx: mpsc::Receiver<Envelope>,
    thread: JoinHandle<()>,
    draft: Option<String>,
    context: Option<crate::model_config::ContextAnchor>,
}

/// Lifecycle state of a live chat session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatPhase {
    /// No request is currently running.
    Idle,
    /// A request is actively producing a response.
    Running,
    /// A request is waiting for tool approval.
    Waiting,
    /// The session was explicitly closed.
    Closed,
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
    phase: ChatPhase,
    generation: u64,
    save_error: bool,
    notice: String,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl ChatSession {
    /// path 是完整的 chat.db 路径；首次打开导入同目录 chat-history.json，旧文件仅作备份。
    /// 同目录独占锁持有到 session 释放；SQLite 是唯一读写数据源。
    pub fn open(path: &Path, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        let lock = FileLock::acquire(&path.with_file_name("chat-history.lock")).map_err(|_| {
            "无法独占聊天记录；请关闭其它使用此数据目录的实例，并检查目录权限。".to_owned()
        })?;
        let (connection, history) = store::open(path).map_err(|error| {
            format!("无法读取或迁移聊天记录：{error}；请检查权限或恢复备份，旧文件未修改。")
        })?;
        let composer_data = store::load_composer(&connection)
            .map_err(|error| format!("无法读取输入附件和控制：{error}"))?;
        let mut session = Self {
            default_composer: composer_data.default_composer.clone(),
            connection,
            _lock: lock,
            history,
            composer_data,
            media_root: path.with_file_name("chat-media"),
            job: None,
            phase: ChatPhase::Idle,
            generation: 0,
            save_error: false,
            notice: String::new(),
            wake,
        };
        for conversation in &mut session.history.conversations {
            if let Some(turn) = conversation.turns.last().filter(|turn| {
                turn.attempts
                    .last()
                    .is_some_and(|attempt| attempt.status == TurnStatus::Running)
            }) {
                if let Some(snapshot) = session.composer_data.turns.get(&turn.id).cloned() {
                    if conversation.draft.is_empty() {
                        conversation.draft = turn.input.clone();
                    }
                    session
                        .composer_data
                        .drafts
                        .insert(conversation.id.clone(), snapshot);
                }
            }
        }
        let recovered = recover(&mut session.history);
        let invalid_selection =
            session.selected().is_none() && !session.history.conversations.is_empty();
        if invalid_selection {
            session.history.selected = session.history.conversations.first().map(|c| c.id.clone());
        }
        if recovered || invalid_selection {
            session.save();
        }
        Ok(session)
    }

    /// 返回聊天历史的只读引用。
    pub fn history(&self) -> &History {
        &self.history
    }
    /// 返回当前选中的对话；选择无效时为空。
    pub fn selected(&self) -> Option<&Conversation> {
        self.selected_index()
            .map(|index| &self.history.conversations[index])
    }
    /// 最近一次保存是否失败；失败期间禁止修改和发送。
    pub fn save_failed(&self) -> bool {
        self.save_error
    }
    /// 返回当前面向用户的提示。
    pub fn notice(&self) -> &str {
        &self.notice
    }

    fn selected_index(&self) -> Option<usize> {
        self.history
            .conversations
            .iter()
            .position(|c| Some(&c.id) == self.history.selected.as_ref())
    }
    fn mutable(&self) -> bool {
        self.phase == ChatPhase::Idle && !self.save_error
    }
    fn save(&mut self) -> bool {
        self.composer_data.default_composer = self.default_composer.clone();
        self.save_error =
            store::save_composer(&mut self.connection, &self.history, &self.composer_data).is_err();
        if self.save_error {
            self.notice = "聊天记录保存失败。内存中的问题、部分回答和草稿仍保留；请检查目录权限或磁盘空间后重试保存。保存成功前禁止更改和发送。".into();
        } else {
            self.notice.clear();
        }
        !self.save_error
    }
    /// 空闲时重新保存当前历史，并更新保存失败状态与提示。
    pub fn retry_save(&mut self) {
        if !self.busy() {
            self.save();
        }
    }
    /// 空闲且无保存错误时设置面向用户的提示。
    pub fn report(&mut self, message: String) {
        if self.mutable() {
            self.notice = message;
        }
    }
    /// 只选择模型，不指定请求推理强度；与最近记录一并原子保存。
    /// 需要显式强度和能力校验的调用方应使用 `select_model_pair`。
    pub fn select_model(&mut self, model: ModelRef) {
        let _ = self.update_model_picker(|session| session.apply_model_pair(model, None));
    }
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
