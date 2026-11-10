//! 聊天上下文与生命周期行为回归。
use super::*;
type TestError = Box<dyn std::error::Error + Send + Sync>;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self, TestError> {
        let path = std::env::temp_dir().join(format!("lumakit-lifecycle-{}", new_id()?));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[cfg(test)]
fn context(turns: &[Turn], input: &str) -> Result<Vec<ChatMessage>, String> {
    composer::context(
        turns,
        input,
        &[],
        &composer::ComposerData::default(),
        false,
        Path::new(""),
    )
}
fn attempt(status: TurnStatus, text: &str) -> Attempt {
    Attempt {
        id: "a".into(),
        model: ModelRef {
            provider: "p".into(),
            endpoint: "e".into(),
            model: "m".into(),
            protocol: crate::Protocol::OpenAiChat,
        },
        text: text.into(),
        status,
        error: None,
        trace_id: None,
        rounds: Vec::new(),
        kind: AttemptKind::Reply,
        source_attempt_id: None,
        parent_attempt_id: None,
        search: SearchState::default(),
        tool_events: Vec::new(),
        reasoning: Vec::new(),
        generated: Vec::new(),
    }
}
fn turn(input: &str, attempts: Vec<Attempt>) -> Turn {
    Turn {
        id: input.into(),
        input: input.into(),
        attempts,
        selected_attempt: None,
    }
}
#[test]
fn context_uses_only_last_completed_attempt_and_enforces_message_bounds() -> Result<(), TestError> {
    let turns = vec![
        turn(
            "第一问",
            vec![
                attempt(TurnStatus::Completed, "旧答案"),
                attempt(TurnStatus::Completed, "新答案"),
            ],
        ),
        turn(
            "第二问",
            vec![
                attempt(TurnStatus::Completed, "不再使用"),
                attempt(TurnStatus::Failed, "部分内容"),
            ],
        ),
        turn("第三问", vec![attempt(TurnStatus::Cancelled, "部分内容")]),
        turn("第四问", vec![attempt(TurnStatus::Running, "部分内容")]),
    ];
    let messages = context(&turns, "现在的问题")?;
    let text: Vec<_> = messages
        .into_iter()
        .map(|message| match message {
            ChatMessage::User(text) => format!("U:{text}"),
            ChatMessage::UserImages { text, .. } => format!("U:{text}"),
            ChatMessage::Assistant(text) => format!("A:{text}"),
        })
        .collect();
    assert_eq!(text, ["U:第一问", "A:新答案", "U:现在的问题"]);
    assert!(context(&[], "  \n").is_err());
    assert!(context(&[], &"x".repeat(MAX_TEXT)).is_ok());
    assert!(context(&[], &"x".repeat(MAX_TEXT + 1)).is_err());
    let large = vec![turn(
        "q",
        vec![attempt(TurnStatus::Completed, &"x".repeat(MAX_TEXT - 2))],
    )];
    assert!(context(&large, "n").is_ok());
    assert!(context(&large, "nn").is_err());
    let empty = vec![turn("q", vec![attempt(TurnStatus::Completed, " \n")])];
    assert!(context(&empty, "n").is_err());
    Ok(())
}
#[test]
fn stale_events_cannot_mutate_active_request_and_close_is_terminal() -> Result<(), TestError> {
    let root = Directory::new()?;
    let mut session = ChatSession::open(&root.path().join("chat.db"), Arc::new(|| {}))?;
    session.new_conversation();
    let conversation_id = session
        .selected()
        .ok_or("missing selected conversation")?
        .id
        .clone();
    session.history.conversations[0]
        .turns
        .push(turn("t", vec![attempt(TurnStatus::Running, "partial")]));
    let (tx, rx) = session.event_channel()?;
    let stale = EventSender {
        generation: tx.generation - 1,
        tx: tx.tx.clone(),
    };
    session.job = Some(Job {
        conversation: 0,
        turn: 0,
        attempt: 0,
        identity: EventIdentity {
            conversation_id,
            turn_id: "t".into(),
            attempt_id: "a".into(),
        },
        generation: tx.generation,
        terminal: false,
        cancel: CancellationToken::new(),
        rx,
        thread: thread::spawn(|| {}),
        draft: None,
        context: None,
    });
    session.phase = ChatPhase::Running;
    stale.send(Event::Delta("stale".into()))?;
    stale.send(Event::End(Err((TurnStatus::Failed, "stale".into()))))?;
    tx.send(Event::Runtime(crate::RunEvent::Waiting { waiting: true }))?;
    session.poll();
    assert_eq!(session.phase(), ChatPhase::Waiting);
    assert_eq!(
        session
            .selected()
            .ok_or("missing selected conversation")?
            .turns[0]
            .attempts[0]
            .text,
        "partial"
    );
    session.cancel();
    assert!(session.busy());
    assert!(session.close().is_err());
    tx.send(Event::Runtime(crate::RunEvent::Waiting { waiting: false }))?;
    session.poll();
    assert_eq!(session.phase(), ChatPhase::Running);
    tx.send(Event::End(Err((TurnStatus::Cancelled, "cancelled".into()))))?;
    tx.send(Event::Delta("late".into()))?;
    session.poll();
    assert_eq!(session.phase(), ChatPhase::Idle);
    assert_eq!(
        session
            .selected()
            .ok_or("missing selected conversation")?
            .turns[0]
            .attempts[0]
            .status,
        TurnStatus::Cancelled
    );
    assert_eq!(
        session
            .selected()
            .ok_or("missing selected conversation")?
            .turns[0]
            .attempts[0]
            .text,
        "partial"
    );
    assert_eq!(
        session
            .selected()
            .ok_or("missing selected conversation")?
            .turns[0]
            .selected_attempt
            .as_deref(),
        Some("a")
    );
    session.close()?;
    assert_eq!(session.phase(), ChatPhase::Closed);
    let snapshot = serde_json::to_value(session.history())?;
    session.set_draft("closed edit".into());
    session.new_conversation();
    session.poll();
    assert_eq!(serde_json::to_value(session.history())?, snapshot);
    assert_eq!(session.phase(), ChatPhase::Closed);
    Ok(())
}
#[test]
fn failed_close_preserves_idle_session_for_save_recovery() -> Result<(), TestError> {
    let root = Directory::new()?;
    let path = root.path().join("chat.db");
    let mut session = ChatSession::open(&path, Arc::new(|| {}))?;
    session.new_conversation();
    let database = rusqlite::Connection::open(&path)?;
    database.execute_batch("CREATE TRIGGER reject_close BEFORE UPDATE ON conversations BEGIN SELECT RAISE(ABORT,'disk failure'); END;")?;
    session.set_draft("retained draft".into());
    assert!(session.close().is_err());
    assert_eq!(session.phase(), ChatPhase::Idle);
    assert_eq!(
        session
            .selected()
            .ok_or("missing selected conversation")?
            .draft,
        "retained draft"
    );
    database.execute_batch("DROP TRIGGER reject_close;")?;
    session.retry_save();
    session.close()?;
    drop(session);
    let reopened = ChatSession::open(&path, Arc::new(|| {}))?;
    assert_eq!(
        reopened
            .selected()
            .ok_or("missing selected conversation")?
            .draft,
        "retained draft"
    );
    Ok(())
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("log buffer poisoned"))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

fn capture_logs(
    body: impl FnOnce() -> Result<(), TestError>,
) -> Result<Vec<lumakit_observability::LogLine>, TestError> {
    use tracing_subscriber::layer::SubscriberExt;
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::Registry::default()
        .with(lumakit_observability::SpanFields)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .event_format(lumakit_observability::GrepFormat::utc())
                .with_writer(buffer.clone()),
        );
    tracing::subscriber::with_default(subscriber, body)?;
    let bytes = buffer.0.lock().map_err(|_| "log buffer poisoned")?.clone();
    Ok(String::from_utf8(bytes)?
        .lines()
        .map(lumakit_observability::parse_line)
        .collect())
}

#[test]
fn persistence_failure_logs_once_until_recovery_and_retains_draft() -> Result<(), TestError> {
    let root = Directory::new()?;
    let path = root.path().join("chat.db");
    let mut session = ChatSession::open(&path, Arc::new(|| {}))?;
    session.new_conversation();
    let database = rusqlite::Connection::open(&path)?;
    let reject = "CREATE TRIGGER reject_save BEFORE UPDATE ON conversations BEGIN SELECT RAISE(ABORT,'PRIVATE_DATABASE_ERROR'); END;";
    database.execute_batch(reject)?;
    let mut first_notice = String::new();
    let mut second_notice = String::new();
    let lines = capture_logs(|| {
        session.set_draft("PRIVATE_DRAFT_BODY".into());
        assert!(session.save_failed());
        first_notice = session.notice().to_owned();
        for _ in 0..3 {
            session.retry_save();
            assert_eq!(session.notice(), first_notice);
        }
        assert!(session.close().is_err());
        assert_eq!(session.phase(), ChatPhase::Idle);
        assert_eq!(
            session.selected().ok_or("missing conversation")?.draft,
            "PRIVATE_DRAFT_BODY"
        );
        database.execute_batch("DROP TRIGGER reject_save;")?;
        session.retry_save();
        assert!(!session.save_failed());
        assert!(session.notice().is_empty());
        database.execute_batch(reject)?;
        session.set_draft("PRIVATE_SECOND_DRAFT".into());
        second_notice = session.notice().to_owned();
        session.retry_save();
        database.execute_batch("DROP TRIGGER reject_save;")?;
        session.retry_save();
        Ok(())
    })?;
    let failures: Vec<_> = lines.iter().filter(|line| line.level == "ERROR").collect();
    assert_eq!(failures.len(), 2);
    for (failure, notice) in failures.iter().zip([first_notice, second_notice]) {
        assert_eq!(failure.operation.as_deref(), Some("chat.persistence.save"));
        assert_eq!(failure.event.as_deref(), Some("ERROR"));
        let entry = lines
            .iter()
            .position(|line| {
                line.operation == failure.operation
                    && line.trace_id == failure.trace_id
                    && line.event.as_deref() == Some("ENTRY")
            })
            .ok_or("missing save entry")?;
        let terminal = lines
            .iter()
            .position(|line| line == *failure)
            .ok_or("missing save terminal")?;
        assert!(entry < terminal);
        assert!(failure.duration_ms.is_some());
        let trace = failure.trace_id.as_deref().ok_or("missing real trace")?;
        assert_eq!(trace.len(), 32);
        assert!(notice.contains(trace));
        assert!(notice.contains("CHAT_STORAGE_DATABASE_FAILED"));
    }
    assert_ne!(failures[0].trace_id, failures[1].trace_id);
    assert!(lines.iter().all(|line| !line.raw.contains("PRIVATE_")));
    assert!(
        lines
            .iter()
            .all(|line| line.event.as_deref() != Some("UNFINISHED"))
    );
    session.close()?;
    drop(session);
    let reopened = ChatSession::open(&path, Arc::new(|| {}))?;
    assert_eq!(
        reopened.selected().ok_or("missing conversation")?.draft,
        "PRIVATE_SECOND_DRAFT"
    );
    Ok(())
}

#[test]
fn rejected_request_save_has_one_final_error_and_rolls_back_request() -> Result<(), TestError> {
    let root = Directory::new()?;
    let path = root.path().join("chat.db");
    let mut session = ChatSession::open(&path, Arc::new(|| {}))?;
    session.select_model(attempt(TurnStatus::Completed, "").model);
    session.set_draft("PRIVATE_UNSENT_DRAFT".into());
    let database = rusqlite::Connection::open(&path)?;
    database.execute_batch("CREATE TRIGGER reject_request BEFORE UPDATE ON conversations BEGIN SELECT RAISE(ABORT,'PRIVATE_SQL_ERROR'); END;")?;
    let lines = capture_logs(|| {
        session.send(AiConnection {
            base_url: "http://127.0.0.1:1".into(),
            protocol: crate::Protocol::OpenAiChat,
            model: "m".into(),
            api_key: None,
            settings: Default::default(),
        });
        Ok(())
    })?;
    assert_eq!(session.phase(), ChatPhase::Idle);
    assert!(session.save_failed());
    let conversation = session.selected().ok_or("missing conversation")?;
    assert_eq!(conversation.draft, "PRIVATE_UNSENT_DRAFT");
    assert!(conversation.turns.is_empty());
    let errors: Vec<_> = lines.iter().filter(|line| line.level == "ERROR").collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].operation.as_deref(), Some("ai.request"));
    let trace = errors[0]
        .trace_id
        .as_deref()
        .ok_or("missing request trace")?;
    assert!(session.notice().contains(trace));
    assert!(lines.iter().any(|line| line.level == "WARN"
        && line.operation.as_deref() == Some("chat.persistence.save")
        && line.trace_id.as_deref() == Some(trace)));
    assert!(lines.iter().any(|line| line.level == "DEBUG"
        && line.operation.as_deref() == Some("chat.persistence.save")
        && line.event.as_deref() == Some("ENTRY")
        && line.trace_id.as_deref() == Some(trace)));
    assert!(lines.iter().all(|line| !line.raw.contains("PRIVATE_")));
    assert!(
        lines
            .iter()
            .all(|line| line.event.as_deref() != Some("UNFINISHED"))
    );
    database.execute_batch("DROP TRIGGER reject_request;")?;
    session.retry_save();
    Ok(())
}

#[test]
fn open_does_not_expose_corrupt_storage_contents() -> Result<(), TestError> {
    let root = Directory::new()?;
    let path = root.path().join("chat.db");
    std::fs::write(
        root.path().join("chat-history.json"),
        b"PRIVATE_INVALID_JSON",
    )?;
    let error = ChatSession::open(&path, Arc::new(|| {}))
        .err()
        .ok_or("corrupt legacy data unexpectedly accepted")?;
    assert!(error.contains("CHAT_STORAGE_JSON_INVALID"));
    assert!(!error.contains("PRIVATE_INVALID_JSON"));
    assert!(!error.contains(&root.path().to_string_lossy().to_string()));
    Ok(())
}

#[test]
fn internal_prepare_failure_preserves_draft_and_has_a_real_trace() -> Result<(), TestError> {
    let root = Directory::new()?;
    let mut session = ChatSession::open(&root.path().join("chat.db"), Arc::new(|| {}))?;
    session.select_model(attempt(TurnStatus::Completed, "").model);
    session.set_draft("PRIVATE_PREPARE_DRAFT".into());
    let lines = capture_logs(|| {
        session.send(AiConnection {
            base_url: "http://127.0.0.1:1".into(),
            protocol: crate::Protocol::OpenAiChat,
            model: "mismatched".into(),
            api_key: None,
            settings: Default::default(),
        });
        Ok(())
    })?;
    assert!(!session.busy());
    assert_eq!(
        session.selected().ok_or("missing conversation")?.draft,
        "PRIVATE_PREPARE_DRAFT"
    );
    assert!(
        session
            .selected()
            .ok_or("missing conversation")?
            .turns
            .is_empty()
    );
    let errors: Vec<_> = lines.iter().filter(|line| line.level == "ERROR").collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].operation.as_deref(), Some("chat.prepare"));
    let trace = errors[0]
        .trace_id
        .as_deref()
        .ok_or("missing prepare trace")?;
    assert!(session.notice().contains(trace));
    assert!(
        lines
            .iter()
            .all(|line| !line.raw.contains("PRIVATE_PREPARE_DRAFT"))
    );
    Ok(())
}

#[test]
fn completed_network_result_does_not_hide_final_save_failure() -> Result<(), TestError> {
    let root = Directory::new()?;
    let path = root.path().join("chat.db");
    let mut session = ChatSession::open(&path, Arc::new(|| {}))?;
    session.new_conversation();
    let conversation_id = session.selected().ok_or("missing conversation")?.id.clone();
    session.history.conversations[0]
        .turns
        .push(turn("pending", vec![attempt(TurnStatus::Running, "")]));
    let (tx, rx) = session.event_channel()?;
    session.job = Some(Job {
        conversation: 0,
        turn: 0,
        attempt: 0,
        identity: EventIdentity {
            conversation_id,
            turn_id: "pending".into(),
            attempt_id: "a".into(),
        },
        generation: tx.generation,
        terminal: false,
        cancel: CancellationToken::new(),
        rx,
        thread: thread::spawn(|| {}),
        draft: Some("PRIVATE_ORIGINAL_DRAFT".into()),
        context: None,
    });
    session.phase = ChatPhase::Running;
    let database = rusqlite::Connection::open(&path)?;
    // 对话字段未变化时 upsert 不执行 UPDATE；终态新增 attempt 才是必经写入。
    database.execute_batch("CREATE TRIGGER reject_completion BEFORE INSERT ON attempts BEGIN SELECT RAISE(ABORT,'PRIVATE_SQL'); END;")?;
    let lines = capture_logs(|| {
        tx.send(Event::Delta("PRIVATE_PARTIAL_REPLY".into()))?;
        tx.send(Event::End(Ok(crate::AiRun {
            text: "PRIVATE_COMPLETED_REPLY".into(),
            tool_calls: Vec::new(),
            rounds: Vec::new(),
            reasoning_blocks: Vec::new(),
        })))?;
        session.poll();
        session.retry_save();
        Ok(())
    })?;
    assert_eq!(session.phase(), ChatPhase::Idle);
    assert!(session.save_failed());
    let completed = &session.selected().ok_or("missing conversation")?.turns[0].attempts[0];
    assert_eq!(completed.status, TurnStatus::Completed);
    assert_eq!(completed.text, "PRIVATE_COMPLETED_REPLY");
    assert!(completed.error.is_none());
    let errors: Vec<_> = lines.iter().filter(|line| line.level == "ERROR").collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0].operation.as_deref(),
        Some("chat.persistence.save")
    );
    assert!(
        session.notice().contains(
            errors[0]
                .trace_id
                .as_deref()
                .ok_or("missing persistence trace")?
        )
    );
    assert!(lines.iter().all(|line| !line.raw.contains("PRIVATE_")));
    database.execute_batch("DROP TRIGGER reject_completion;")?;
    session.retry_save();
    assert!(!session.save_failed());
    session.close()?;
    drop(session);
    let reopened = ChatSession::open(&path, Arc::new(|| {}))?;
    assert_eq!(
        reopened.selected().ok_or("missing conversation")?.turns[0].attempts[0].text,
        "PRIVATE_COMPLETED_REPLY"
    );
    Ok(())
}
