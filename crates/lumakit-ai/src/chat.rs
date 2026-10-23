//! 与界面无关的文本对话；仅把每个问题末次已完成的回答送入后续上下文。
use crate::{AiConnection, AiError, ChatMessage, ModelRef, history::TurnStatus};
use lumakit_support::{FileLock, load_versioned, new_id, save_versioned};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const VERSION: u32 = 1;
const MAX_TEXT: usize = 1024 * 1024;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct History {
    pub conversations: Vec<Conversation>,
    pub selected: Option<String>,
    pub model: Option<ModelRef>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub draft: String,
    pub turns: Vec<Turn>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Turn {
    pub id: String,
    pub input: String,
    pub attempts: Vec<Attempt>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Attempt {
    pub id: String,
    pub model: ModelRef,
    pub text: String,
    pub status: TurnStatus,
    pub error: Option<String>,
}

enum Event {
    Delta(String),
    End(Result<String, (TurnStatus, String)>),
}
struct Job {
    conversation: usize,
    turn: usize,
    attempt: usize,
    cancel: CancellationToken,
    rx: mpsc::Receiver<Event>,
    thread: JoinHandle<()>,
}

/// 历史的唯一可变持有者；宿主只通过动作修改，不向渲染器暴露连接和密钥。
pub struct ChatSession {
    path: PathBuf,
    _lock: FileLock,
    history: History,
    job: Option<Job>,
    save_error: bool,
    notice: String,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl ChatSession {
    /// path 是完整的 chat-history.json 路径；同目录锁持有到 session 释放。
    pub fn open(path: &Path, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        let lock = FileLock::acquire(&path.with_file_name("chat-history.lock"))
            .map_err(|_| "无法独占聊天记录；请关闭其它使用此数据目录的实例，并检查目录权限。".to_owned())?;
        let history = match load_versioned::<History>(path, VERSION)
            .map_err(|_| "无法读取聊天记录或版本不受支持；请检查权限或恢复备份，原文件未修改。".to_owned())? {
            None => History::default(),
            Some((VERSION, value)) => value,
            Some(_) => return Err("聊天记录版本不支持，原文件未修改。".into()),
        };
        let mut session = Self {
            path: path.to_owned(), _lock: lock, history, job: None,
            save_error: false, notice: String::new(), wake,
        };
        let recovered = recover(&mut session.history);
        let invalid_selection = session.selected().is_none()
            && !session.history.conversations.is_empty();
        if invalid_selection {
            session.history.selected = session.history.conversations.first().map(|c| c.id.clone());
        }
        if recovered || invalid_selection { session.save(); }
        Ok(session)
    }

    pub fn history(&self) -> &History { &self.history }
    pub fn selected(&self) -> Option<&Conversation> {
        self.selected_index().map(|index| &self.history.conversations[index])
    }
    pub fn busy(&self) -> bool { self.job.is_some() }
    pub fn save_failed(&self) -> bool { self.save_error }
    pub fn notice(&self) -> &str { &self.notice }

    fn selected_index(&self) -> Option<usize> {
        self.history.conversations.iter().position(|c| Some(&c.id) == self.history.selected.as_ref())
    }
    fn mutable(&self) -> bool { !self.busy() && !self.save_error }
    fn save(&mut self) -> bool {
        // ponytail: 每次操作原子重写整个文件；记录规模明显拖慢界面时再迁移 SQLite。
        self.save_error = save_versioned(&self.path, VERSION, &self.history).is_err();
        if self.save_error {
            self.notice = "聊天记录保存失败。内存中的问题、部分回答和草稿仍保留；请检查目录权限或磁盘空间后重试保存。保存成功前禁止更改和发送。".into();
        } else {
            self.notice.clear();
        }
        !self.save_error
    }
    pub fn retry_save(&mut self) {
        if !self.busy() { self.save(); }
    }
    pub fn report(&mut self, message: String) {
        if self.mutable() { self.notice = message; }
    }
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
    pub fn select_conversation(&mut self, id: &str) {
        if !self.mutable() || !self.history.conversations.iter().any(|c| c.id == id)
            || self.history.selected.as_deref() == Some(id) { return; }
        self.history.selected = Some(id.into());
        self.save();
    }
    pub fn delete_conversation(&mut self, id: &str) {
        if !self.mutable() { return; }
        let Some(index) = self.history.conversations.iter().position(|c| c.id == id) else { return; };
        self.history.conversations.remove(index);
        if self.history.selected.as_deref() == Some(id) {
            self.history.selected = self.history.conversations.last().map(|c| c.id.clone());
        }
        self.save();
    }
    pub fn select_model(&mut self, model: ModelRef) {
        if !self.mutable() { return; }
        self.history.model = Some(model);
        self.save();
    }
    pub fn set_draft(&mut self, text: String) {
        if !self.mutable() { return; }
        if let Some(index) = self.selected_index() {
            self.history.conversations[index].draft = text;
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
            self.history.model.clone().ok_or_else(|| "请先选择已保存且已启用的模型。".into())
        }
    }
    pub fn send(&mut self, connection: AiConnection) { self.start(connection, false, false); }
    pub fn retry(&mut self, connection: AiConnection) { self.start(connection, true, false); }
    /// 显式换模型重答：宿主先 select_model，再按 history.model 解析真实连接。
    pub fn retry_with(&mut self, connection: AiConnection) { self.start(connection, true, true); }

    fn start(&mut self, connection: AiConnection, retry: bool, override_model: bool) {
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
            let messages = context(&conversation.turns[..ti], &input)?;
            let attempt = Attempt {
                id: new_id().map_err(|_| "无法生成回答标识，请重试。".to_owned())?,
                model, text: String::new(), status: TurnStatus::Running, error: None,
            };
            let turn_id = if retry { None } else {
                Some(new_id().map_err(|_| "无法生成问题标识，请重试。".to_owned())?)
            };
            Ok((ci, ti, input, messages, attempt, turn_id))
        })();
        let (ci, ti, input, messages, attempt, turn_id) = match prepared {
            Ok(value) => value,
            Err(error) => { self.notice = error; return; }
        };
        let conversation = &mut self.history.conversations[ci];
        let previous_title = if !retry && conversation.turns.is_empty() {
            Some(std::mem::replace(&mut conversation.title,
                input.chars().take(28).map(|c| if c.is_control() { ' ' } else { c }).collect()))
        } else { None };
        let draft = if retry { None } else { Some(std::mem::take(&mut conversation.draft)) };
        if let Some(id) = turn_id {
            conversation.turns.push(Turn { id, input, attempts: Vec::new() });
        }
        let ai = conversation.turns[ti].attempts.len();
        conversation.turns[ti].attempts.push(attempt);
        // 接受请求前必须落盘；失败时完整撤销新增记录，不能留下重复问题或 Running。
        if !self.save() {
            let conversation = &mut self.history.conversations[ci];
            if retry { conversation.turns[ti].attempts.pop(); }
            else { conversation.turns.pop(); }
            if let Some(title) = previous_title { conversation.title = title; }
            if let Some(draft) = draft { conversation.draft = draft; }
            return;
        }
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let wake = self.wake.clone();
        let (tx, rx) = mpsc::sync_channel(64);
        let worker = thread::Builder::new().name("lumakit-chat".into()).spawn(move || {
            let result = run_worker(connection, messages, worker_cancel, &tx, &wake);
            let _ = tx.send(Event::End(result));
            wake();
        });
        match worker {
            Ok(thread) => self.job = Some(Job { conversation: ci, turn: ti, attempt: ai, cancel, rx, thread }),
            Err(_) => {
                let attempt = &mut self.history.conversations[ci].turns[ti].attempts[ai];
                attempt.status = TurnStatus::Failed;
                attempt.error = Some("无法启动聊天线程，请重试。".into());
                self.save();
            }
        }
    }

    fn active_attempt(&mut self) -> Option<&mut Attempt> {
        let job = self.job.as_ref()?;
        self.history.conversations.get_mut(job.conversation)?.turns.get_mut(job.turn)?.attempts.get_mut(job.attempt)
    }
    /// 每次最多处理 128 个事件；余下事件通过 wake 继续驱动宿主。
    pub fn poll(&mut self) {
        let mut ended = false;
        let mut processed = 0;
        for _ in 0..128 {
            let event = match self.job.as_ref().map(|job| job.rx.try_recv()) {
                Some(Ok(event)) => event,
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    if let Some(attempt) = self.active_attempt() {
                        attempt.status = TurnStatus::Failed;
                        attempt.error = Some("聊天线程意外结束；已保留部分回答，可重试。".into());
                    }
                    ended = true;
                    break;
                }
                _ => break,
            };
            processed += 1;
            if let Some(attempt) = self.active_attempt() {
                match event {
                    Event::Delta(text) => {
                        if text.len() <= MAX_TEXT.saturating_sub(attempt.text.len()) { attempt.text.push_str(&text); }
                    }
                    Event::End(result) => {
                        match result {
                            Ok(text) => { attempt.text = text; attempt.status = TurnStatus::Completed; attempt.error = None; }
                            Err((status, error)) => { attempt.status = status; attempt.error = Some(error); }
                        }
                        ended = true;
                    }
                }
            }
            if ended { break; }
        }
        if ended {
            if let Some(job) = self.job.take() { let _ = job.thread.join(); }
            self.save();
        } else if processed == 128 && self.busy() {
            // 达到每帧上限时补唤醒；队列已空则等待 worker，避免空转。
            (self.wake)();
        }
    }
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
) -> Result<String, (TurnStatus, String)> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()
        .map_err(|_| (TurnStatus::Failed, "无法启动聊天网络运行时，请重试。".into()))?;
    let mut bytes = 0usize;
    let mut overflow = false;
    let result = runtime.block_on(crate::stream_chat(connection, messages, cancel.clone(), |text| {
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
    }));
    if overflow {
        return Err((TurnStatus::Failed, "回答超过 1 MiB，已停止；保留已有内容。".into()));
    }
    match result {
        Ok(text) if text.trim().is_empty() => Err((TurnStatus::Failed, "服务返回了空回答，请重试；空回答不会用于后续上下文。".into())),
        Ok(text) if text.len() <= MAX_TEXT => Ok(text),
        Ok(_) => Err((TurnStatus::Failed, "回答超过 1 MiB，已停止。".into())),
        Err(AiError::Cancelled) => Err((TurnStatus::Cancelled, "已停止；部分回答不会用于后续上下文。".into())),
        Err(error) => Err((TurnStatus::Failed, format!("请求失败：{error}"))),
    }
}

fn context(turns: &[Turn], input: &str) -> Result<Vec<ChatMessage>, String> {
    if input.trim().is_empty() { return Err("请输入问题；支持多行，Ctrl+Enter 发送。".into()); }
    let mut bytes = input.len();
    if bytes > MAX_TEXT { return Err("问题超过 1 MiB；草稿已保留，请缩短后再发送。".into()); }
    let mut messages = Vec::new();
    for turn in turns {
        if let Some(attempt) = turn.attempts.last().filter(|a| a.status == TurnStatus::Completed) {
            // 与 stream_chat 的非空消息约束一致；不静默丢弃或截断已有上下文。
            if turn.input.trim().is_empty() || attempt.text.trim().is_empty() {
                return Err("已完成的历史回答为空，无法形成有效上下文；请重答或新建对话。".into());
            }
            bytes = bytes.saturating_add(turn.input.len()).saturating_add(attempt.text.len());
            if bytes > MAX_TEXT {
                return Err("完整上下文超过 1 MiB；不会自动截断，请新建对话后继续。草稿已保留。".into());
            }
            messages.push(ChatMessage::User(turn.input.clone()));
            messages.push(ChatMessage::Assistant(attempt.text.clone()));
        }
    }
    messages.push(ChatMessage::User(input.into()));
    Ok(messages)
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
            text: text.into(), status, error: None,
        }
    }
    fn turn(input: &str, attempts: Vec<Attempt>) -> Turn {
        Turn { id: input.into(), input: input.into(), attempts }
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
}
