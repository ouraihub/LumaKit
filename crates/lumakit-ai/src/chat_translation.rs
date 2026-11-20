//! 翻译候选的准备、持久化与后台请求启动。
use super::worker::run_session_worker;
use super::*;

impl ChatSession {
    /// 将已完成回答翻译为新候选；源回答不变。
    ///
    /// # Errors
    /// Returns invalid language/source/connection, identifier generation, busy session or persistence errors.
    pub fn translate(
        &mut self,
        conversation_id: &str,
        turn_id: &str,
        attempt_id: &str,
        language: &str,
        connection: AiConnection,
    ) -> Result<(), String> {
        if !self.mutable() {
            return Err("当前聊天不可修改。".into());
        }
        let prepared = (|| {
            if language.trim().is_empty()
                || language.chars().count() > 80
                || language.chars().any(char::is_control)
            {
                return Err("目标语言无效。".into());
            }
            let source = self
                .history
                .conversations
                .iter()
                .find(|conversation| conversation.id == conversation_id)
                .and_then(|conversation| conversation.turns.iter().find(|turn| turn.id == turn_id))
                .and_then(|turn| {
                    turn.attempts
                        .iter()
                        .find(|attempt| attempt.id == attempt_id)
                })
                .ok_or("回答不存在。")?;
            if source.status != TurnStatus::Completed {
                return Err("只能翻译已完成的回答。".into());
            }
            if connection.protocol != source.model.protocol {
                return Err("翻译连接协议不匹配。".into());
            }
            let messages = vec![ChatMessage::User(format!(
                "请将以下内容翻译为 {language}，只返回译文：\n\n{}",
                source.text
            ))];
            let model = ModelRef {
                provider: source.model.provider.clone(),
                endpoint: source.model.endpoint.clone(),
                model: connection.model.clone(),
                protocol: connection.protocol,
            };
            let attempt = Attempt {
                id: new_id().map_err(|_| "无法生成翻译回答标识。")?,
                model,
                text: String::new(),
                status: TurnStatus::Running,
                error: None,
                trace_id: None,
                rounds: Vec::new(),
                reasoning: Vec::new(),
                kind: AttemptKind::Translation,
                source_attempt_id: Some(attempt_id.into()),
                parent_attempt_id: Some(attempt_id.into()),
                search: SearchState::default(),
                tool_events: Vec::new(),
                generated: Vec::new(),
            };
            let ci = self
                .history
                .conversations
                .iter()
                .position(|conversation| conversation.id == conversation_id)
                .ok_or("对话不存在。")?;
            let ti = self.history.conversations[ci]
                .turns
                .iter()
                .position(|turn| turn.id == turn_id)
                .ok_or("问题不存在。")?;
            let ai = self.history.conversations[ci].turns[ti].attempts.len();
            let (tx, rx) = self.event_channel()?;
            Ok::<_, String>((messages, attempt, ci, ti, ai, tx, rx))
        })();
        let (messages, attempt, ci, ti, ai, tx, rx) = prepared.map_err(|message| {
            diagnostics::prepare_failure(&message, "CHAT_TRANSLATION_PREPARE_FAILED")
        })?;
        let (trace, root) = lumakit_observability::TraceContext::root("ai.request", "service");
        let mut attempt = attempt;
        attempt.trace_id = Some(trace.trace_id().to_owned());
        self.history.conversations[ci].turns[ti]
            .attempts
            .push(attempt);
        let Some(root) = self.save_request(&trace, root) else {
            self.history.conversations[ci].turns[ti].attempts.pop();
            return Err(self.notice.clone());
        };
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let wake = self.wake.clone();
        let pending_root = super::worker::PendingRoot::new(root);
        let worker = match thread::Builder::new()
            .name("lumakit-translate".into())
            .spawn(move || {
                let root = match pending_root.take() {
                    Ok(root) => root,
                    Err(error) => {
                        let _ = tx.send(Event::End(Err((TurnStatus::Failed, error.into()))));
                        wake();
                        return;
                    }
                };
                let _entered = root.span().clone().entered();
                let result = run_session_worker(
                    connection,
                    None,
                    None,
                    messages,
                    (&trace, root),
                    worker_cancel,
                    &tx,
                    &wake,
                    PathBuf::new(),
                );
                if tx.send(Event::End(result)).is_err() {
                    tracing::debug!(
                        event = "chat.event.drop",
                        reason = "receiver_closed",
                        "聊天终态接收端已关闭"
                    );
                }
                wake();
            }) {
            Ok(worker) => worker,
            Err(_) => {
                let attempt = &mut self.history.conversations[ci].turns[ti].attempts[ai];
                attempt.status = TurnStatus::Failed;
                attempt.error = Some("无法启动翻译线程。".into());
                self.save();
                return Err("无法启动翻译线程。".into());
            }
        };
        self.job = Some(Job {
            conversation: ci,
            turn: ti,
            attempt: ai,
            identity: EventIdentity {
                conversation_id: conversation_id.into(),
                turn_id: turn_id.into(),
                attempt_id: self.history.conversations[ci].turns[ti].attempts[ai]
                    .id
                    .clone(),
            },
            generation: self.generation,
            terminal: false,
            cancel,
            rx,
            thread: worker,
            draft: None,
            context: None,
        });
        self.phase = ChatPhase::Running;
        Ok(())
    }
}
