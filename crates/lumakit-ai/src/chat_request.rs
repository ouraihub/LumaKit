//! 请求准备、输入快照与后台任务启动。
use super::worker::run_session_worker;
use super::*;

impl ChatSession {
    /// 普通重试锁定末次尝试的来源，不受当前模型下拉选择影响。
    ///
    /// # Errors
    /// Returns unavailable session, conversation, retry source or selected model errors.
    pub fn request_model(&self, retry: bool) -> Result<ModelRef, String> {
        if self.busy() {
            return Err("请先等待当前请求结束。".into());
        }
        if self.save_error {
            return Err("请先重试保存聊天记录。".into());
        }
        let conversation = self.selected().ok_or("请先新建对话。")?;
        if retry {
            conversation
                .turns
                .last()
                .and_then(|t| t.attempts.last())
                .map(|a| a.model.clone())
                .ok_or_else(|| "没有可重试的问题。".into())
        } else {
            self.history
                .model
                .clone()
                .ok_or_else(|| "请先选择已保存且已启用的聊天模型。".into())
        }
    }
    /// 新请求读取草稿，重试锁定原问题的绘图开关与模型；旧直接绘图记录沿用原来源。
    ///
    /// # Errors
    /// Returns unavailable request sources or a missing configured painting model.
    pub fn request_painting_model(&self, retry: bool) -> Result<Option<ModelRef>, String> {
        self.request_model(retry)?;
        let conversation = self.selected().ok_or("请先新建对话。")?;
        let settings = if retry {
            let turn = conversation.turns.last().ok_or("没有可重试的问题。")?;
            self.composer_data
                .turns
                .get(&turn.id)
                .map(|snapshot| &snapshot.settings)
        } else {
            Some(self.composer())
        };
        let Some(settings) = settings.filter(|settings| settings.image_mode) else {
            return Ok(None);
        };
        let legacy_model = if retry {
            conversation
                .turns
                .last()
                .and_then(|turn| turn.attempts.first())
                .map(|attempt| attempt.model.clone())
        } else {
            None
        };
        settings
            .image_model
            .clone()
            .or(legacy_model)
            .map(Some)
            .ok_or_else(|| "请先选择已保存的绘图模型。".into())
    }
    /// 使用给定连接发送当前草稿；先保存请求记录，再启动后台流式请求。
    ///
    /// 忙碌或保存失败时不操作；校验失败时保留草稿并设置提示。
    pub fn send(&mut self, connection: AiConnection) {
        self.start(connection, None, false, false);
    }
    /// 使用末次尝试的模型重答最后一个问题，保留已有尝试记录。
    ///
    /// 给定连接必须匹配原模型及协议；忙碌或保存失败时不操作。
    pub fn retry(&mut self, connection: AiConnection) {
        self.start(connection, None, true, false);
    }
    /// 显式换模型重答：宿主先 select_model，再按 history.model 解析真实连接。
    pub fn retry_with(&mut self, connection: AiConnection) {
        self.start(connection, None, true, true);
    }
    /// 发送聊天请求，模型自行决定是否调用已选择的绘图工具。
    pub fn send_with_painting(&mut self, chat: AiConnection, painting: AiConnection) {
        self.start(chat, Some(painting), false, false);
    }
    /// 使用原问题的聊天来源、绘图来源与输入控制重试。
    pub fn retry_with_painting(&mut self, chat: AiConnection, painting: AiConnection) {
        self.start(chat, Some(painting), true, false);
    }
    /// 使用已 select_model 的聊天来源重试；绘图模型与控制仍取原问题快照，可迁移旧直接绘图失败。
    pub fn retry_with_model_and_painting(&mut self, chat: AiConnection, painting: AiConnection) {
        self.start(chat, Some(painting), true, true);
    }

    fn start(
        &mut self,
        connection: AiConnection,
        painting: Option<AiConnection>,
        retry: bool,
        override_model: bool,
    ) {
        if !self.mutable() {
            return;
        }
        if !retry {
            self.ensure_conversation();
            if !self.mutable() {
                return;
            }
        }
        let prepared = (|| {
            let model = self.request_model(retry && !override_model)?;
            if connection.model != model.model || connection.protocol != model.protocol {
                return Err("连接与请求模型不匹配，请重新选择已保存的模型。".to_owned());
            }
            let ci = self.selected_index().ok_or("请先新建对话。")?;
            let conversation = &self.history.conversations[ci];
            let input = if retry {
                conversation
                    .turns
                    .last()
                    .ok_or("没有可重试的问题。")?
                    .input
                    .clone()
            } else {
                conversation.draft.trim().to_owned()
            };
            let ti = if retry {
                conversation.turns.len() - 1
            } else {
                conversation.turns.len()
            };
            let mut snapshot = if retry {
                self.composer_data
                    .turns
                    .get(&conversation.turns[ti].id)
                    .cloned()
                    .unwrap_or_default()
            } else {
                self.composer_data
                    .drafts
                    .get(&conversation.id)
                    .cloned()
                    .unwrap_or_default()
            };
            if snapshot.settings.image_mode {
                let painting_model = self
                    .request_painting_model(retry)?
                    .ok_or("请先配置绘图模型。")?;
                let painting = painting.as_ref().ok_or("绘图已启用：宿主需同时解析聊天与绘图模型，并调用 send_with_painting / retry_with_painting；不再直接发送原始提示到图片接口。")?;
                if connection.settings.supports_tools == Some(false) {
                    return Err("所选聊天模型声明不支持工具，请更换聊天模型或关闭生成图片。".into());
                }
                if painting.model != painting_model.model
                    || painting.protocol != painting_model.protocol
                {
                    return Err("绘图连接与原问题选择的模型不匹配。".into());
                }
                generation::validate(painting)?;
                snapshot.settings.image_model = Some(painting_model);
            } else if painting.is_some() {
                return Err("本问题未启用绘图工具，请使用普通聊天入口。".into());
            }
            let mut connection = connection;
            connection.settings = crate::model_reference::effective_settings(
                &connection.model,
                connection.protocol,
                &connection.settings,
            );
            if connection.settings.model_type != crate::model_config::ModelType::Chat {
                return Err("所选模型不是聊天模型。".into());
            }
            if snapshot.settings.web_search && connection.protocol == crate::Protocol::OpenAiChat {
                return Err(
                    "Chat Completions 不支持原生联网搜索，请选择 Responses 或 Anthropic 线路。"
                        .into(),
                );
            }
            connection.settings.native_web_search = snapshot.settings.web_search;
            connection.settings.reasoning_effort = None;
            if let Some(effort) = snapshot.settings.reasoning {
                connection = connection.with_reasoning_effort(effort)?;
            }
            connection.settings.validate(connection.protocol)?;
            let messages = composer::context(
                &conversation.turns[..ti],
                &input,
                &snapshot.attachments,
                &self.composer_data,
                connection.settings.supports_vision,
                &self.media_root,
            )?;
            let attempt = Attempt {
                id: new_id().map_err(|_| "无法生成回答标识，请重试。".to_owned())?,
                model,
                text: String::new(),
                status: TurnStatus::Running,
                error: None,
                trace_id: None,
                rounds: Vec::new(),
                reasoning: Vec::new(),
                kind: AttemptKind::Reply,
                source_attempt_id: None,
                parent_attempt_id: None,
                search: SearchState::default(),
                tool_events: Vec::new(),
                generated: Vec::new(),
            };
            let turn_id = if retry {
                None
            } else {
                Some(new_id().map_err(|_| "无法生成问题标识，请重试。".to_owned())?)
            };
            Ok((
                ci, ti, input, messages, attempt, turn_id, snapshot, connection,
            ))
        })();
        let (ci, ti, input, messages, mut attempt, turn_id, snapshot, connection) = match prepared {
            Ok(value) => value,
            Err(error) => {
                self.notice = diagnostics::prepare_failure(&error, "CHAT_PREPARE_FAILED");
                return;
            }
        };
        let (tx, rx) = match self.event_channel() {
            Ok(channel) => channel,
            Err(error) => {
                self.notice = diagnostics::prepare_failure(&error, "CHAT_GENERATION_EXHAUSTED");
                return;
            }
        };
        let (trace, root) = lumakit_observability::TraceContext::root("ai.request", "service");
        attempt.trace_id = Some(trace.trace_id().to_owned());
        let previous_composer = self.composer_data.clone();
        let image_mode = snapshot.settings.image_mode;
        let painting_model = snapshot.settings.image_model.clone();
        let conversation = &mut self.history.conversations[ci];
        let previous_title = if !retry && conversation.turns.is_empty() {
            Some(std::mem::replace(
                &mut conversation.title,
                input
                    .chars()
                    .take(28)
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .collect(),
            ))
        } else {
            None
        };
        let draft = if retry {
            None
        } else {
            Some(std::mem::take(&mut conversation.draft))
        };
        if let Some(id) = turn_id.clone() {
            conversation.turns.push(Turn {
                id,
                input,
                attempts: Vec::new(),
                selected_attempt: None,
            });
        }
        let ai = conversation.turns[ti].attempts.len();
        conversation.turns[ti].attempts.push(attempt);
        self.composer_data.turns.insert(
            self.history.conversations[ci].turns[ti].id.clone(),
            snapshot,
        );
        if !retry
            && let Some(draft) = self
                .composer_data
                .drafts
                .get_mut(&self.history.conversations[ci].id)
        {
            draft.attachments.clear();
        }
        // 接受请求前必须落盘；失败时完整撤销新增记录，不能留下重复问题或 Running。
        let Some(root) = self.save_request(&trace, root) else {
            let conversation = &mut self.history.conversations[ci];
            if retry {
                conversation.turns[ti].attempts.pop();
            } else {
                conversation.turns.pop();
            }
            if let Some(title) = previous_title {
                conversation.title = title;
            }
            if let Some(draft) = draft {
                conversation.draft = draft;
            }
            self.composer_data = previous_composer;
            return;
        };
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let wake = self.wake.clone();
        let identity = EventIdentity {
            conversation_id: self.history.conversations[ci].id.clone(),
            turn_id: self.history.conversations[ci].turns[ti].id.clone(),
            attempt_id: self.history.conversations[ci].turns[ti].attempts[ai]
                .id
                .clone(),
        };
        let media_root = self.media_root.clone();
        let context = (!image_mode).then(|| {
            usage::context_anchor(
                &messages,
                &self.history.conversations[ci].turns[ti].attempts[ai].model,
                connection.settings.native_web_search,
            )
        });
        let span = tracing::debug_span!(parent: root.span(), "ai.request", conversation_id = %identity.conversation_id, turn_id = %identity.turn_id, attempt_id = %identity.attempt_id, protocol = ?connection.protocol, image_mode);
        let pending_root = super::worker::PendingRoot::new(root);
        let worker = thread::Builder::new()
            .name("lumakit-chat".into())
            .spawn(move || {
                let root = match pending_root.take() {
                    Ok(root) => root,
                    Err(error) => {
                        let _ = tx.send(Event::End(Err((TurnStatus::Failed, error.into()))));
                        wake();
                        return;
                    }
                };
                // worker 独占同步运行时；span 仅在线程入口进入，不跨异步任务持有 guard。
                let _entered = span.enter();
                let result = run_session_worker(
                    connection,
                    painting,
                    painting_model,
                    messages,
                    (&trace, root),
                    worker_cancel,
                    &tx,
                    &wake,
                    media_root,
                );
                if tx.send(Event::End(result)).is_err() {
                    tracing::debug!(
                        event = "chat.event.drop",
                        reason = "receiver_closed",
                        "聊天终态接收端已关闭"
                    );
                }
                wake();
            });
        match worker {
            Ok(thread) => {
                self.job = Some(Job {
                    conversation: ci,
                    turn: ti,
                    attempt: ai,
                    identity,
                    generation: self.generation,
                    terminal: false,
                    cancel,
                    rx,
                    thread,
                    draft,
                    context,
                });
                self.phase = ChatPhase::Running;
            }
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
