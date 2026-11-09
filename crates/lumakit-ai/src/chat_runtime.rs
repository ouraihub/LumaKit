//! 有界事件传输、generation 过滤、唯一终态归约与关闭。
use super::*;

impl ChatSession {
    /// Returns the current lifecycle phase while the session is alive.
    pub fn phase(&self) -> ChatPhase {
        self.phase
    }
    /// Closes the session after pending work reaches its terminal event.
    pub fn close(&mut self) -> Result<(), String> {
        if self.phase == ChatPhase::Closed {
            return Ok(());
        }
        if self.busy() {
            self.cancel();
            return Err("聊天请求仍在停止，请稍后再次关闭。".into());
        }
        if !self.save() {
            return Err(self.notice.clone());
        }
        self.phase = ChatPhase::Closed;
        Ok(())
    }
    /// 是否仍持有尚未处理完终态的后台请求。
    pub fn busy(&self) -> bool {
        matches!(self.phase, ChatPhase::Running | ChatPhase::Waiting)
    }
    pub(super) fn event_channel(&mut self) -> (EventSender, mpsc::Receiver<Envelope>) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("chat generation exhausted");
        let (tx, rx) = mpsc::sync_channel(64);
        (
            EventSender {
                generation: self.generation,
                tx,
            },
            rx,
        )
    }
    pub(super) fn restore_draft(&mut self, ci: usize, ti: usize, draft: Option<String>) {
        let Some(draft) = draft else {
            return;
        };
        let conversation = &mut self.history.conversations[ci];
        conversation.draft = draft;
        if let Some(snapshot) = self
            .composer_data
            .turns
            .get(&conversation.turns[ti].id)
            .cloned()
        {
            self.composer_data
                .drafts
                .insert(conversation.id.clone(), snapshot);
        }
    }

    fn active_attempt(&mut self) -> Option<&mut Attempt> {
        let job = self.job.as_ref()?;
        self.history
            .conversations
            .get_mut(job.conversation)?
            .turns
            .get_mut(job.turn)?
            .attempts
            .get_mut(job.attempt)
    }
    fn active_identity_matches(&self) -> bool {
        let Some(job) = &self.job else {
            return false;
        };
        self.history
            .conversations
            .get(job.conversation)
            .filter(|conversation| conversation.id == job.identity.conversation_id)
            .and_then(|conversation| conversation.turns.get(job.turn))
            .filter(|turn| turn.id == job.identity.turn_id)
            .and_then(|turn| turn.attempts.get(job.attempt))
            .is_some_and(|attempt| attempt.id == job.identity.attempt_id)
    }
    /// 每次最多处理 128 个事件；余下事件通过 wake 继续驱动宿主。
    pub fn poll(&mut self) {
        let mut ended = false;
        let mut processed = 0;
        let mut waiting_phase = None;
        for _ in 0..128 {
            let event = match self.job.as_ref().map(|job| job.rx.try_recv()) {
                Some(Ok(envelope)) => {
                    if !self.job.as_ref().is_some_and(|job| {
                        job.generation == envelope.generation
                            && envelope.generation == self.generation
                    }) {
                        continue;
                    }
                    envelope.event
                }
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    if self.active_identity_matches()
                        && let Some(attempt) = self.active_attempt()
                    {
                        attempt.status = TurnStatus::Failed;
                        attempt.error = Some("聊天线程意外结束；已保留部分回答，可重试。".into());
                    }
                    ended = true;
                    break;
                }
                _ => break,
            };
            processed += 1;
            if !self.active_identity_matches() {
                continue;
            }
            let terminal = self.job.as_ref().is_some_and(|job| job.terminal);
            if terminal {
                continue;
            }
            let persist = !matches!(&event, Event::Delta(_) | Event::End(_));
            let context = self.job.as_ref().and_then(|job| job.context.clone());
            if let Some(attempt) = self.active_attempt() {
                match event {
                    Event::Delta(text) => {
                        if text.len() <= MAX_TEXT.saturating_sub(attempt.text.len()) {
                            attempt.text.push_str(&text);
                        }
                    }
                    Event::Generated { images, rounds } => {
                        attempt.generated.extend(images);
                        attempt.rounds.extend(rounds);
                    }
                    Event::Runtime(crate::RunEvent::Tool(event)) => {
                        if let Some(previous) = attempt
                            .tool_events
                            .iter_mut()
                            .find(|previous| previous.id == event.id)
                        {
                            *previous = event;
                        } else {
                            attempt.tool_events.push(event);
                        }
                    }
                    Event::Runtime(crate::RunEvent::Waiting { waiting }) => {
                        waiting_phase = Some(waiting);
                    }
                    Event::Runtime(crate::RunEvent::Round {
                        mut usage,
                        reasoning,
                    }) => {
                        for round in &mut usage {
                            round.model = Some(attempt.model.clone());
                        }
                        attempt.rounds.extend(usage);
                        attempt.reasoning.extend(reasoning);
                    }
                    Event::End(result) => {
                        let attempt_id = self
                            .job
                            .as_ref()
                            .map(|job| job.attempt)
                            .and_then(|attempt| {
                                self.history
                                    .conversations
                                    .get(self.job.as_ref()?.conversation)?
                                    .turns
                                    .get(self.job.as_ref()?.turn)?
                                    .attempts
                                    .get(attempt)
                            })
                            .map(|attempt| attempt.id.clone());
                        if let Some(attempt) = self.active_attempt() {
                            match result {
                                Ok(result) => {
                                    if attempt.rounds.len() == 1 && attempt.tool_events.is_empty() {
                                        attempt.rounds[0].context = context;
                                    }
                                    attempt.text = result.text;
                                    attempt.status = TurnStatus::Completed;
                                    attempt.error = None;
                                }
                                Err((status, error)) => {
                                    attempt.status = status;
                                    attempt.error = Some(error);
                                }
                            }
                        }
                        if let (Some(job), Some(attempt_id)) = (&self.job, attempt_id) {
                            if let Some(turn) = self
                                .history
                                .conversations
                                .get_mut(job.conversation)
                                .and_then(|conversation| conversation.turns.get_mut(job.turn))
                            {
                                turn.selected_attempt = Some(attempt_id);
                            }
                        }
                        if let Some(job) = &mut self.job {
                            job.terminal = true;
                        }
                        ended = true;
                    }
                }
            }
            if let Some(waiting) = waiting_phase.take() {
                self.phase = if waiting {
                    ChatPhase::Waiting
                } else {
                    ChatPhase::Running
                };
            }
            // 工具与媒体先落盘，后续聊天失败或进程重启也不会丢失已完成的绘图。
            if persist {
                self.save();
            }
            if ended {
                break;
            }
        }
        if ended {
            if let Some(job) = self.job.take() {
                if self.history.conversations[job.conversation].turns[job.turn].attempts
                    [job.attempt]
                    .status
                    != TurnStatus::Completed
                {
                    self.restore_draft(job.conversation, job.turn, job.draft);
                }
                let _ = job.thread.join();
            }
            self.phase = ChatPhase::Idle;
            self.save();
        } else if processed == 128 && self.busy() {
            (self.wake)();
        }
    }
    /// 请求取消当前后台任务；终态及部分输出由后续轮询处理。
    pub fn cancel(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.cancel();
        }
    }
    /// 宿主拦截关闭：取消后等待终态，再确认草稿和回答全部保存。
    pub fn can_close(&mut self) -> bool {
        if self.busy() {
            self.cancel();
            self.notice = "正在停止请求并保存记录，请稍后再次关闭窗口。".into();
            return false;
        }
        if self.save_error {
            return false;
        }
        self.save()
    }
}
impl Drop for ChatSession {
    fn drop(&mut self) {
        self.cancel();
        // 有界队列必须边排空边等待，不能直接 join 被背压阻塞的 worker。
        while self.busy() {
            self.poll();
            if self.busy() {
                thread::sleep(Duration::from_millis(5));
            }
        }
        self.save();
    }
}
