//! 请求准备、输入快照、翻译与后台任务启动。
use super::*;

impl ChatSession {
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
        let cancel = CancellationToken::new(); let worker_cancel = cancel.clone(); let wake = self.wake.clone();
        let (tx, rx) = self.event_channel();
        let worker = match thread::Builder::new().name("lumakit-translate".into()).spawn(move || { let result = run_worker(connection, messages, worker_cancel, &tx, &wake); let _ = tx.send(Event::End(result)); wake(); }) {
            Ok(worker) => worker,
            Err(_) => {
                let attempt = &mut self.history.conversations[ci].turns[ti].attempts[ai];
                attempt.status = TurnStatus::Failed;
                attempt.error = Some("无法启动翻译线程。".into());
                self.save();
                return Err("无法启动翻译线程。".into());
            }
        };
        self.job = Some(Job { conversation: ci, turn: ti, attempt: ai, identity: EventIdentity { conversation_id: conversation_id.into(), turn_id: turn_id.into(), attempt_id: self.history.conversations[ci].turns[ti].attempts[ai].id.clone() }, generation: self.generation, terminal: false, cancel, rx, thread: worker, draft: None, context: None });
        self.phase = ChatPhase::Running;
        Ok(())
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
        if !retry {
            self.ensure_conversation();
            if !self.mutable() { return; }
        }
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
            connection.settings = crate::model_reference::effective_settings(&connection.model, connection.protocol, &connection.settings);
            if connection.settings.model_type != crate::model_config::ModelType::Chat { return Err("所选模型不是聊天模型。".into()); }
            if snapshot.settings.web_search && connection.protocol == crate::Protocol::OpenAiChat { return Err("Chat Completions 不支持原生联网搜索，请选择 Responses 或 Anthropic 线路。".into()); }
            connection.settings.native_web_search = snapshot.settings.web_search;
            connection.settings.reasoning_effort = None;
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
        let painting_model = snapshot.settings.image_model.clone();
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
        let (tx, rx) = self.event_channel();
        let media_root = self.media_root.clone();
        let context = (!image_mode).then(|| usage::context_anchor(&messages, &self.history.conversations[ci].turns[ti].attempts[ai].model, connection.settings.native_web_search));
        let span = tracing::info_span!("ai.request", conversation_id = %identity.conversation_id, turn_id = %identity.turn_id, attempt_id = %identity.attempt_id, model = %connection.model, protocol = ?connection.protocol, image_mode);
        let worker = thread::Builder::new().name("lumakit-chat".into()).spawn(move || {
            // worker 独占同步运行时；span 仅在线程入口进入，不跨异步任务持有 guard。
            let _entered = span.enter();
            let result = run_session_worker(connection, painting, painting_model, messages, worker_cancel, &tx, &wake, media_root);
            match &result {
                Ok(_) => tracing::info!(event = "ai.request.completed", "AI request completed"),
                Err((TurnStatus::Cancelled, _)) => tracing::info!(event = "ai.request.cancelled", "AI request cancelled"),
                Err(_) => tracing::error!(event = "ai.request.failed", "AI 请求失败；详情仅保存到会话，不记录私有正文"),
            }
            let _ = tx.send(Event::End(result));
            wake();
        });
        match worker {
            Ok(thread) => { self.job = Some(Job { conversation: ci, turn: ti, attempt: ai, identity, generation: self.generation, terminal: false, cancel, rx, thread, draft, context }); self.phase = ChatPhase::Running; }
            Err(_) => {
                let attempt = &mut self.history.conversations[ci].turns[ti].attempts[ai];
                attempt.status = TurnStatus::Failed;
                attempt.error = Some("无法启动聊天线程，请重试。".into());
                self.restore_draft(ci, ti, draft);
                self.save();
            }
        }
    }
}
fn run_worker(
    connection: AiConnection,
    messages: Vec<ChatMessage>,
    cancel: CancellationToken,
    tx: &EventSender,
    wake: &Arc<dyn Fn() + Send + Sync>,
) -> Result<crate::AiRun, (TurnStatus, String)> {
    run_session_worker(connection, None, None, messages, cancel, tx, wake, PathBuf::new())
}

fn run_session_worker(
    connection: AiConnection,
    painting: Option<AiConnection>,
    painting_model: Option<ModelRef>,
    messages: Vec<ChatMessage>,
    cancel: CancellationToken,
    tx: &EventSender,
    wake: &Arc<dyn Fn() + Send + Sync>,
    media_root: PathBuf,
) -> Result<crate::AiRun, (TurnStatus, String)> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()
        .map_err(|_| (TurnStatus::Failed, "无法启动聊天网络运行时，请重试。".into()))?;
    let mut bytes = 0usize;
    let mut overflow = false;
    let host = generation::PaintingHost {
        model: ModelRef { provider: "snapshot".into(), endpoint: "snapshot".into(), model: connection.model.clone(), protocol: connection.protocol },
        chat: connection, painting, painting_model, root: media_root, tx: tx.clone(), wake: wake.clone(), executions: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
