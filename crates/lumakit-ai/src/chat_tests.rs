//! 聊天上下文与生命周期行为回归。
use super::*;
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("lumakit-lifecycle-{}", new_id().unwrap()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &Path { &self.0 }
}
impl Drop for Directory {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
#[cfg(test)]
fn context(turns: &[Turn], input: &str) -> Result<Vec<ChatMessage>, String> {
    composer::context(turns, input, &[], &composer::ComposerData::default(), false, Path::new(""))
}
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
    fn stale_events_cannot_mutate_active_request_and_close_is_terminal() {
        let root = Directory::new();
        let mut session = ChatSession::open(&root.path().join("chat.db"), Arc::new(|| {})).unwrap();
        session.new_conversation();
        let conversation_id = session.selected().unwrap().id.clone();
        session.history.conversations[0].turns.push(turn("t", vec![attempt(TurnStatus::Running, "partial")]));
        let (tx, rx) = session.event_channel();
        let stale = EventSender { generation: tx.generation - 1, tx: tx.tx.clone() };
        session.job = Some(Job { conversation: 0, turn: 0, attempt: 0, identity: EventIdentity { conversation_id, turn_id: "t".into(), attempt_id: "a".into() }, generation: tx.generation, terminal: false, cancel: CancellationToken::new(), rx, thread: thread::spawn(|| {}), draft: None, context: None });
        session.phase = ChatPhase::Running;
        stale.send(Event::Delta("stale".into())).unwrap();
        stale.send(Event::End(Err((TurnStatus::Failed, "stale".into())))).unwrap();
        tx.send(Event::Runtime(crate::RunEvent::Waiting { waiting: true })).unwrap();
        session.poll();
        assert_eq!(session.phase(), ChatPhase::Waiting);
        assert_eq!(session.selected().unwrap().turns[0].attempts[0].text, "partial");
        session.cancel();
        assert!(session.busy());
        assert!(session.close().is_err());
        tx.send(Event::Runtime(crate::RunEvent::Waiting { waiting: false })).unwrap();
        session.poll();
        assert_eq!(session.phase(), ChatPhase::Running);
        tx.send(Event::End(Err((TurnStatus::Cancelled, "cancelled".into())))).unwrap();
        tx.send(Event::Delta("late".into())).unwrap();
        session.poll();
        assert_eq!(session.phase(), ChatPhase::Idle);
        assert_eq!(session.selected().unwrap().turns[0].attempts[0].status, TurnStatus::Cancelled);
        assert_eq!(session.selected().unwrap().turns[0].attempts[0].text, "partial");
        assert_eq!(session.selected().unwrap().turns[0].selected_attempt.as_deref(), Some("a"));
        session.close().unwrap();
        assert_eq!(session.phase(), ChatPhase::Closed);
        let snapshot = serde_json::to_value(session.history()).unwrap();
        session.set_draft("closed edit".into());
        session.new_conversation();
        session.poll();
        assert_eq!(serde_json::to_value(session.history()).unwrap(), snapshot);
        assert_eq!(session.phase(), ChatPhase::Closed);
    }
    #[test]
    fn failed_close_preserves_idle_session_for_save_recovery() {
        let root = Directory::new();
        let path = root.path().join("chat.db");
        let mut session = ChatSession::open(&path, Arc::new(|| {})).unwrap();
        session.new_conversation();
        let database = rusqlite::Connection::open(&path).unwrap();
        database.execute_batch("CREATE TRIGGER reject_close BEFORE UPDATE ON conversations BEGIN SELECT RAISE(ABORT,'disk failure'); END;").unwrap();
        session.set_draft("retained draft".into());
        assert!(session.close().is_err());
        assert_eq!(session.phase(), ChatPhase::Idle);
        assert_eq!(session.selected().unwrap().draft, "retained draft");
        database.execute_batch("DROP TRIGGER reject_close;").unwrap();
        session.retry_save();
        session.close().unwrap();
        drop(session);
        let reopened = ChatSession::open(&path, Arc::new(|| {})).unwrap();
        assert_eq!(reopened.selected().unwrap().draft, "retained draft");
    }
