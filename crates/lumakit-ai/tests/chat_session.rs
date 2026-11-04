//! 聊天会话状态、流式交互与历史持久化的集成测试。
use lumakit_ai::{
    AiConnection, ModelRef, Protocol,
    chat::ChatSession,
    history::TurnStatus,
};
use lumakit_support::new_id;
use serde_json::{Value, json};
use rusqlite::Connection;
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{Arc, atomic::{AtomicUsize, Ordering}, mpsc},
    thread,
    time::{Duration, Instant},
};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("lumakit-chat-{}", new_id().unwrap()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn path(&self) -> PathBuf { self.0.join("chat.db") }
}
impl Drop for Directory {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}
fn open(path: &Path) -> ChatSession { ChatSession::open(path, Arc::new(|| {})).unwrap() }
fn model(name: &str) -> ModelRef {
    ModelRef { provider: "provider-id".into(), endpoint: "endpoint-id-not-url".into(), model: name.into(), protocol: Protocol::OpenAiChat }
}
fn connection(url: &str, name: &str) -> AiConnection {
    AiConnection { base_url: url.into(), model: name.into(), protocol: Protocol::OpenAiChat, api_key: None, settings: Default::default() }
}
fn setup(path: &Path) -> ChatSession {
    let mut session = open(path);
    session.new_conversation();
    session.select_model(model("source"));
    session
}
fn wait_until(session: &mut ChatSession, condition: impl Fn(&ChatSession) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        session.poll();
        if condition(session) { return; }
        assert!(Instant::now() < deadline, "聊天 worker 未及时完成：{}", session.notice());
        thread::sleep(Duration::from_millis(2));
    }
}
fn complete(session: &mut ChatSession) { wait_until(session, |session| !session.busy()); }
fn delta(text: &str) -> String {
    format!("data: {}\n\n", json!({"choices":[{"delta":{"content":text}}]}))
}
fn response(text: &str) -> String { format!("{}data: [DONE]\n\n", delta(text)) }

fn block_writes(path: &Path) {
    Connection::open(path).unwrap().execute_batch("CREATE TRIGGER reject_attempt BEFORE INSERT ON attempts BEGIN SELECT RAISE(ABORT, 'injected disk failure'); END;").unwrap();
}
fn restore_writes(path: &Path) {
    Connection::open(path).unwrap().execute_batch("DROP TRIGGER reject_attempt;").unwrap();
}
fn count(path: &Path, table: &str) -> i64 {
    Connection::open(path).unwrap().query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0)).unwrap()
}

#[test]
fn toolbar_preferences_are_global_compatible_and_atomic() {
    use lumakit_ai::chat::ToolbarTool::{Attachment, GenerateImage, NewConversation, WebSearch};
    let root = Directory::new();
    let path = root.path();
    let mut session = open(&path);
    assert_eq!(session.toolbar(), &[NewConversation, WebSearch]);
    session.set_toolbar(vec![GenerateImage, Attachment, GenerateImage]);
    assert!(session.history().conversations.is_empty());
    assert_eq!(session.toolbar(), &[GenerateImage, Attachment]);
    assert!(!session.composer().web_search && !session.composer().image_mode);
    session.new_conversation();
    session.set_draft("保留草稿".into());
    let first = session.selected().unwrap().id.clone();
    session.new_conversation();
    session.select_conversation(&first);
    assert_eq!(session.toolbar(), &[GenerateImage, Attachment]);
    drop(session);
    let mut session = open(&path);
    assert_eq!(session.toolbar(), &[GenerateImage, Attachment]);
    assert_eq!(session.selected().unwrap().draft, "保留草稿");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER reject_toolbar BEFORE UPDATE ON chat_meta WHEN NEW.key='composer' BEGIN SELECT RAISE(ABORT, 'injected toolbar failure'); END;").unwrap();
    session.set_toolbar(vec![]);
    assert!(session.save_failed());
    assert_eq!(session.toolbar(), &[GenerateImage, Attachment]);
    db.execute_batch("DROP TRIGGER reject_toolbar;").unwrap();
    session.retry_save();
    drop(session);
    let mut composer: Value = serde_json::from_str(&db.query_row::<String, _, _>("SELECT value FROM chat_meta WHERE key='composer'", [], |row| row.get(0)).unwrap()).unwrap();
    composer.as_object_mut().unwrap().remove("toolbar");
    db.execute("UPDATE chat_meta SET value=?1 WHERE key='composer'", [composer.to_string()]).unwrap();
    let session = open(&path);
    assert_eq!(session.toolbar(), &[NewConversation, WebSearch]);
    assert_eq!(session.selected().unwrap().draft, "保留草稿");
    drop(session);
    composer["toolbar"] = json!(["FutureTool", "GenerateImage", "Attachment", "GenerateImage"]);
    db.execute("UPDATE chat_meta SET value=?1 WHERE key='composer'", [composer.to_string()]).unwrap();
    let mut session = open(&path);
    assert_eq!(session.toolbar(), &[GenerateImage, Attachment]);
    assert_eq!(session.selected().unwrap().draft, "保留草稿");
    session.set_toolbar(vec![]);
    drop(session);
    assert!(open(&path).toolbar().is_empty());
}

// 真 HTTP 回环：读取宿主实际发送的正文，保持响应可覆盖停止和缺失结束帧。
fn server(responses: Vec<(String, bool)>) -> (String, mpsc::Receiver<Value>, thread::JoinHandle<()>) {
    server_path(responses, "/v1/chat/completions")
}

fn server_path(responses: Vec<(String, bool)>, path: &'static str) -> (String, mpsc::Receiver<Value>, thread::JoinHandle<()>) {
    server_steps(responses.into_iter().map(|(body, hold)| (path, 200, body, hold)).collect())
}

fn server_steps(responses: Vec<(&'static str, u16, String, bool)>) -> (String, mpsc::Receiver<Value>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        for (path, status, body, hold) in responses {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "没有收到预期 HTTP 请求");
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("HTTP accept: {error}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            socket.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            let (start, length) = loop {
                let mut bytes = [0; 4096];
                let count = socket.read(&mut bytes).unwrap();
                assert_ne!(count, 0);
                request.extend_from_slice(&bytes[..count]);
                if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    assert!(headers.starts_with(&format!("POST {path} HTTP/1.1")));
                    assert!(headers.to_ascii_lowercase().contains("user-agent: lumakit/"));
                    let length = headers.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                    }).unwrap();
                    break (end + 4, length);
                }
            };
            while request.len() < start + length {
                let mut bytes = [0; 4096];
                let count = socket.read(&mut bytes).unwrap();
                assert_ne!(count, 0);
                request.extend_from_slice(&bytes[..count]);
            }
            tx.send(serde_json::from_slice(&request[start..start + length]).unwrap()).unwrap();
            let header = format!("HTTP/1.1 {status} Test\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len() + if hold { 100 } else { 0 });
            if socket.write_all(header.as_bytes()).is_ok() && socket.write_all(body.as_bytes()).is_ok() && hold {
                let mut byte = [0];
                let _ = socket.read(&mut byte);
            }
        }
    });
    (format!("http://{address}/v1"), rx, worker)
}

#[test]
fn composer_attachments_restart_retry_and_wire_controls() {
    use lumakit_ai::{chat::ComposerSettings, model_config::{ReasoningConfig, ReasoningDialect, ReasoningEffort}};
    let root = Directory::new();
    let path = root.path();
    let text = root.0.join("source.rs");
    let image = root.0.join("reference.png");
    fs::write(&text, "fn attached() { /* 实际附件内容 */ }").unwrap();
    fs::write(&image, b"\x89PNG\r\n\x1a\n").unwrap();
    let mut session = setup(&path);
    session.set_draft("比较附件".into());
    session.add_attachment(&text).unwrap();
    session.add_attachment(&image).unwrap();
    let attachment_id = session.draft_attachments()[1].id.clone();
    assert_eq!(fs::read(session.media_path(&attachment_id).unwrap()).unwrap(), b"\x89PNG\r\n\x1a\n");
    let metadata: String = Connection::open(&path).unwrap().query_row("SELECT value FROM chat_meta WHERE key='composer'", [], |row| row.get(0)).unwrap();
    assert!(!metadata.contains("iVBORw0KGgo="));
    assert!(metadata.contains("storage_key"));
    session.set_composer(ComposerSettings { reasoning: Some(ReasoningEffort::High), ..Default::default() });
    drop(session);
    fs::remove_file(text).unwrap();
    fs::remove_file(image).unwrap();
    let mut session = open(&path);
    assert_eq!(session.draft_attachments().len(), 2);
    assert_eq!(session.selected().unwrap().draft, "比较附件");
    let media = session.media_path(&attachment_id).unwrap();
    fs::write(&media, b"invalid!").unwrap();
    assert!(session.media_path(&attachment_id).is_err());
    fs::write(&media, b"\x89PNG\r\n\x1a\n").unwrap();
    let (url, requests, worker) = server(vec![(response("回答"), false), (response("重试回答"), false)]);
    let connect = || { let mut c = connection(&url, "source"); c.settings.supports_vision = true; c.settings.reasoning = Some(ReasoningConfig { dialect: ReasoningDialect::OpenAi, efforts: vec![ReasoningEffort::Low, ReasoningEffort::High], budget_tokens: None }); c };
    session.send(connection(&url, "source"));
    assert!(!session.busy());
    assert_eq!(session.draft_attachments().len(), 2);
    session.send(connect());
    complete(&mut session);
    let first = requests.recv().unwrap();
    assert_eq!(first["reasoning_effort"], "high");
    assert!(first["messages"][0]["content"][0]["text"].as_str().unwrap().contains("实际附件内容"));
    assert!(first["messages"][0]["content"][1]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));
    let turn_id = session.selected().unwrap().turns[0].id.clone();
    assert_eq!(session.turn_attachments(&turn_id).len(), 2);
    assert!(session.media_path(&attachment_id).unwrap().is_file());
    let copy = session.copy_conversation(&session.selected().unwrap().id.clone()).unwrap();
    assert_eq!(session.turn_attachments(&session.selected().unwrap().turns[0].id).len(), 2);
    session.delete_conversation(&copy);
    drop(session);
    let mut session = open(&path);
    session.set_composer(ComposerSettings::default());
    session.retry(connect());
    complete(&mut session);
    let retry = requests.recv().unwrap();
    assert_eq!(retry["messages"], first["messages"]);
    assert_eq!(retry["reasoning_effort"], "high");
    worker.join().unwrap();

    let (url, requests, worker) = server_path(vec![("data: {\"type\":\"response.output_text.delta\",\"delta\":\"联网回答\"}\n\ndata: {\"type\":\"response.completed\"}\n\n".into(), false)], "/v1/responses");
    session.new_conversation();
    session.select_model(ModelRef { protocol: Protocol::OpenAiResponses, ..model("search") });
    session.set_draft("搜索问题".into());
    session.set_composer(ComposerSettings { web_search: true, ..Default::default() });
    let mut c = connection(&url, "search"); c.protocol = Protocol::OpenAiResponses;
    session.send(c);
    complete(&mut session);
    assert_eq!(requests.recv().unwrap()["tools"][0]["type"], "web_search");
    worker.join().unwrap();
}

fn painting_connection(url: &str, name: &str) -> AiConnection {
    let mut c = connection(url, name);
    c.settings.model_type = lumakit_ai::model_config::ModelType::Image;
    c
}
fn tool_calls(calls: &[(&str, Value)]) -> String {
    let calls: Vec<_> = calls.iter().enumerate().map(|(index, (id, args))| json!({"index":index,"id":id,"function":{"name":"generate_image","arguments":args.to_string()}})).collect();
    format!("data: {}\n\ndata: [DONE]\n\n", json!({"choices":[{"delta":{"tool_calls":calls},"finish_reason":"tool_calls"}]}))
}
fn image_response() -> String { json!({"data":[{"b64_json":"iVBORw0KGgo="}],"usage":{"input_tokens":12,"output_tokens":24}}).to_string() }

#[test]
fn painting_tool_chain_preserves_history_wire_and_restart_retry_sources() {
    use lumakit_ai::{chat::ComposerSettings, model_config::{ReasoningConfig, ReasoningDialect, ReasoningEffort}};
    let root = Directory::new();
    let path = root.path();
    let mut session = setup(&path);
    let (url, requests, worker) = server_steps(vec![
        ("/v1/chat/completions", 200, response("之前的回答"), false),
        ("/v1/chat/completions", 200, tool_calls(&[("paint-1", json!({"prompt":"模型整理的完整画面🙂","size":"1536x1024"}))]), false),
        ("/v1/images/generations", 200, image_response(), false),
        ("/v1/chat/completions", 200, response("已生成图片"), false),
        ("/v1/chat/completions", 200, tool_calls(&[("retry-1", json!({"prompt":"重试画面"}))]), false),
        ("/v1/images/generations", 200, image_response(), false),
        ("/v1/chat/completions", 200, response("重新生成完成"), false),
        ("/v1/chat/completions", 200, response("继续对话"), false),
    ]);
    session.set_draft("之前的问题".into());
    session.send(connection(&url, "source"));
    complete(&mut session);
    requests.recv().unwrap();
    session.set_composer(ComposerSettings { image_mode: true, image_model: Some(model("painting-original")), reasoning: Some(ReasoningEffort::High), ..Default::default() });
    session.set_draft("画一张图".into());
    assert_eq!(session.request_model(false).unwrap().model, "source");
    session.send(connection(&url, "source"));
    assert!(!session.busy());
    assert!(session.notice().contains("send_with_painting"));
    let chat = || { let mut c = connection(&url, "source"); c.settings.reasoning = Some(ReasoningConfig { dialect: ReasoningDialect::OpenAi, efforts: vec![ReasoningEffort::Low, ReasoningEffort::High], budget_tokens: None }); c };
    session.send_with_painting(chat(), painting_connection(&url, "painting-original"));
    complete(&mut session);
    let first = requests.recv().unwrap();
    let painted = requests.recv().unwrap();
    let continuation = requests.recv().unwrap();
    assert_eq!(first["messages"], json!([{"role":"user","content":"之前的问题"},{"role":"assistant","content":"之前的回答"},{"role":"user","content":"画一张图"}]));
    assert_eq!(first["tools"][0]["function"]["name"], "generate_image");
    assert_eq!(first["reasoning_effort"], "high");
    assert_eq!(painted, json!({"model":"painting-original","prompt":"模型整理的完整画面🙂","size":"1536x1024","n":1}));
    assert!(painted.get("response_format").is_none());
    assert_eq!(continuation["messages"][4]["role"], "tool");
    assert!(continuation["messages"][4]["content"].as_str().unwrap().contains("Generated 1 image(s)."));
    let attempt = &session.selected().unwrap().turns[1].attempts[0];
    assert_eq!(attempt.model.model, "source");
    assert_eq!(attempt.text, "已生成图片");
    assert_eq!(attempt.tool_events[0].approval, Some(lumakit_ai::Approval::Allow));
    assert!(!attempt.tool_events[0].is_error);
    assert_eq!(attempt.rounds.len(), 3);
    let id = attempt.generated[0].id.clone();
    assert_eq!(fs::read(session.media_path(&id).unwrap()).unwrap(), b"\x89PNG\r\n\x1a\n");
    drop(session);
    let mut session = open(&path);
    assert!(session.media_path(&id).unwrap().is_file());
    session.select_model(model("different-chat"));
    session.set_composer(ComposerSettings { image_model: Some(model("different-painting")), ..Default::default() });
    assert_eq!(session.request_model(true).unwrap().model, "source");
    assert_eq!(session.request_painting_model(true).unwrap().unwrap().model, "painting-original");
    session.retry_with_painting(chat(), painting_connection(&url, "painting-original"));
    complete(&mut session);
    let retry = requests.recv().unwrap();
    assert_eq!(retry["messages"], first["messages"]);
    assert_eq!(retry["reasoning_effort"], "high");
    assert_eq!(requests.recv().unwrap()["size"], "1024x1024");
    requests.recv().unwrap();
    session.set_draft("继续".into());
    session.send(connection(&url, "different-chat"));
    complete(&mut session);
    let next = requests.recv().unwrap();
    assert_eq!(next["messages"][3]["content"], "重新生成完成");
    assert!(session.media_path(&id).unwrap().is_file());
    worker.join().unwrap();
}

#[test]
fn attachment_limits_and_save_failure_preserve_draft() {
    let root = Directory::new();
    let path = root.path();
    let attachment = root.0.join("note.txt");
    fs::write(&attachment, "正文").unwrap();
    let mut session = setup(&path);
    session.set_draft("草稿".into());
    session.add_attachment(&attachment).unwrap();
    fs::write(&attachment, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    assert!(session.add_attachment(&attachment).is_err());
    block_writes(&path);
    session.send(connection("http://127.0.0.1:1/v1", "source"));
    assert!(session.save_failed());
    assert!(!session.busy());
    assert_eq!(session.selected().unwrap().draft, "草稿");
    assert_eq!(session.draft_attachments().len(), 1);
    restore_writes(&path);
    session.retry_save();
    let original = session.composer().web_search;
    Connection::open(&path).unwrap().execute_batch("CREATE TRIGGER reject_composer BEFORE UPDATE ON chat_meta WHEN NEW.key='composer' BEGIN SELECT RAISE(ABORT, 'injected composer failure'); END;").unwrap();
    session.set_composer(lumakit_ai::chat::ComposerSettings { web_search: !original, ..Default::default() });
    assert!(session.save_failed());
    assert_eq!(session.composer().web_search, original);
    assert_eq!(session.draft_attachments().len(), 1);
    Connection::open(&path).unwrap().execute_batch("DROP TRIGGER reject_composer;").unwrap();
    session.retry_save();
    session.set_composer(lumakit_ai::chat::ComposerSettings { web_search: true, ..Default::default() });
    session.send(connection("http://127.0.0.1:1/v1", "source"));
    assert!(!session.busy());
    assert!(session.notice().contains("不支持原生联网搜索"));
    assert_eq!(session.draft_attachments().len(), 1);
}

#[test]
fn painting_invalid_failed_duplicate_and_cap_results_never_fake_success() {
    use lumakit_ai::chat::ComposerSettings;
    let root = Directory::new();
    let mut session = setup(&root.path());
    session.set_composer(ComposerSettings { image_mode: true, image_model: Some(model("image")), ..Default::default() });
    session.set_draft("绘图失败".into());
    let (url, requests, worker) = server_steps(vec![
        ("/v1/chat/completions", 200, tool_calls(&[("invalid", json!({"prompt":"x","size":"bad"})), ("failed", json!({"prompt":"画面"}))]), false),
        ("/v1/images/generations", 400, "不支持的绘图参数".into(), false),
        ("/v1/chat/completions", 200, tool_calls(&[("failed", json!({"prompt":"重复不得执行"})), ("cap", json!({"prompt":"超过上限不得执行"}))]), false),
        ("/v1/chat/completions", 200, response("生成失败，未产生图片"), false),
    ]);
    session.send_with_painting(connection(&url, "source"), painting_connection(&url, "image"));
    complete(&mut session);
    let sent: Vec<_> = (0..4).map(|_| requests.recv().unwrap()).collect();
    assert_eq!(sent[1]["prompt"], "画面");
    let results = sent[2]["messages"].as_array().unwrap();
    assert!(results[2]["content"].as_str().unwrap().contains("参数无效"));
    assert!(results[3]["content"].as_str().unwrap().contains("HTTP 400"));
    assert!(results[3]["content"].as_str().unwrap().contains("不要自动重试"));
    let conversation = session.selected().unwrap();
    let attempt = &conversation.turns[0].attempts[0];
    assert_eq!(attempt.status, TurnStatus::Completed);
    assert!(attempt.generated.is_empty());
    assert_eq!(attempt.tool_events.len(), 3);
    assert!(attempt.tool_events.iter().all(|event| event.is_error && event.error.is_some() && event.result.is_some()));
    assert!(attempt.tool_events[2].result.as_ref().unwrap().contains("最多执行两次"));
    worker.join().unwrap();
}

#[test]
fn old_v1_load_recovery_lock_and_draft_round_trip() {
    let root = Directory::new();
    let path = root.path();
    let value = json!({"schemaVersion":1,"value":{
        "selected":"old-c","model":{"provider":"old-p","endpoint":"old-e","model":"old-m","protocol":"OpenAiChat"},
        "conversations":[{"id":"old-c","title":"旧标题","turns":[{"id":"old-t","input":"旧问题","attempts":[{
            "id":"old-a","model":{"provider":"old-p","endpoint":"old-e","model":"old-m","protocol":"OpenAiChat"},
            "text":"中断内容","status":"running","error":null
        }]}]}]
    }});
    let legacy = path.with_file_name("chat-history.json");
    let bytes = serde_json::to_vec(&value).unwrap();
    fs::write(&legacy, &bytes).unwrap();
    let mut session = open(&path);
    let attempt = &session.selected().unwrap().turns[0].attempts[0];
    assert_eq!(attempt.id, "old-a");
    assert_eq!(attempt.text, "中断内容");
    assert_eq!(attempt.status, TurnStatus::Failed);
    assert!(session.selected().unwrap().draft.is_empty());
    assert!(ChatSession::open(&path, Arc::new(|| {})).is_err());
    session.set_draft("重启保留草稿".into());
    assert!(session.can_close());
    drop(session);
    let session = open(&path);
    assert_eq!(session.selected().unwrap().draft, "重启保留草稿");
    assert_eq!(session.request_model(true).unwrap().model, "old-m");
    assert_eq!(fs::read(legacy).unwrap(), bytes);
    assert_eq!(count(&path, "attempts"), 1);
}

#[test]
fn schema_v2_is_idempotent_and_preserves_future_guard() {
    let root = Directory::new();
    let path = root.path();
    drop(open(&path));
    let database = Connection::open(&path).unwrap();
    assert_eq!(database.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), 4);
    for (table, column) in [("turns", "selected_attempt"), ("attempts", "kind"), ("attempts", "search"), ("attempts", "generated"), ("attempts", "tool_events"), ("attempts", "reasoning")] {
        assert!(database.query_row(&format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name='{column}'"), [], |row| row.get::<_, i64>(0)).is_ok());
    }
    drop(database);
    drop(open(&path));
    let database = Connection::open(&path).unwrap();
    assert_eq!(database.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), 4);
}
#[test]
fn unknown_versions_leave_original_bytes_untouched() {
    for version in [0, 2] {
        let root = Directory::new();
        let path = root.path();
        let legacy = path.with_file_name("chat-history.json");
        let bytes = serde_json::to_vec(&json!({"schemaVersion":version,"value":{"conversations":[],"selected":null,"model":null}})).unwrap();
        fs::write(&legacy, &bytes).unwrap();
        assert!(ChatSession::open(&path, Arc::new(|| {})).is_err());
        assert_eq!(fs::read(&legacy).unwrap(), bytes);
        assert_eq!(count(&path, "chat_meta"), 0);
    }
}

#[test]
fn preflight_save_failure_rolls_back_and_never_sends_or_duplicates() {
    let root = Directory::new();
    let path = root.path();
    let mut session = setup(&path);
    session.set_draft("  原始问题\n ".into());
    block_writes(&path);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    session.send(connection(&format!("http://{}/v1", listener.local_addr().unwrap()), "source"));
    assert!(session.save_failed());
    assert!(!session.busy());
    assert!(listener.accept().is_err());
    let conversation = session.selected().unwrap();
    assert_eq!(conversation.title, "新对话");
    assert_eq!(conversation.draft, "  原始问题\n ");
    assert!(conversation.turns.is_empty());
    session.set_draft("不能覆盖".into());
    session.new_conversation();
    assert_eq!(session.history().conversations.len(), 1);
    assert_eq!(session.selected().unwrap().draft, "  原始问题\n ");
    assert!(!session.can_close());
    assert_eq!(count(&path, "turns"), 0);
    restore_writes(&path);
    session.retry_save();
    assert!(!session.save_failed());
    let (url, requests, worker) = server(vec![(response("回答"), false)]);
    session.send(connection(&url, "source"));
    complete(&mut session);
    assert_eq!(requests.recv().unwrap()["messages"][0]["content"], "原始问题");
    assert_eq!(session.selected().unwrap().turns.len(), 1);
    assert_eq!(session.selected().unwrap().turns[0].attempts.len(), 1);
    worker.join().unwrap();
}

#[test]
fn source_retry_override_and_multi_turn_context_use_real_connections() {
    let root = Directory::new();
    let mut session = setup(&root.path());
    let (url, requests, worker) = server(vec![
        (response("旧回答"), false), (response("源模型重答"), false),
        (response("换模型重答"), false), (response("下一轮"), false),
    ]);
    session.set_draft("第一问".into());
    session.send(connection(&url, "source"));
    complete(&mut session);
    session.select_model(model("override"));
    assert_eq!(session.request_model(true).unwrap().model, "source");
    session.retry(connection(&url, "override"));
    assert!(!session.busy());
    assert_eq!(session.selected().unwrap().turns[0].attempts.len(), 1);
    session.retry(connection(&url, "source"));
    complete(&mut session);
    session.retry_with(connection(&url, "override"));
    complete(&mut session);
    session.set_draft("第二问".into());
    session.send(connection(&url, "override"));
    complete(&mut session);
    let sent: Vec<_> = (0..4).map(|_| requests.recv().unwrap()).collect();
    assert_eq!(sent[0]["model"], "source");
    assert_eq!(sent[1]["model"], "source");
    assert_eq!(sent[2]["model"], "override");
    assert_eq!(sent[1]["messages"].as_array().unwrap().len(), 1);
    assert_eq!(sent[2]["messages"].as_array().unwrap().len(), 1);
    assert_eq!(sent[3]["messages"], json!([
        {"role":"user","content":"第一问"},
        {"role":"assistant","content":"换模型重答"},
        {"role":"user","content":"第二问"},
    ]));
    assert_eq!(session.selected().unwrap().turns[0].attempts.len(), 3);
    worker.join().unwrap();
}

#[test]
fn terminal_save_failure_preserves_answer_and_guards_all_mutations() {
    let root = Directory::new();
    let path = root.path();
    let mut session = setup(&path);
    let (url, requests, worker) = server(vec![(response("完整终态内容"), false)]);
    session.set_draft("问题".into());
    session.send(connection(&url, "source"));
    requests.recv_timeout(Duration::from_secs(10)).unwrap();
    block_writes(&path);
    complete(&mut session);
    assert!(session.save_failed());
    let attempt = &session.selected().unwrap().turns[0].attempts[0];
    assert_eq!(attempt.status, TurnStatus::Completed);
    assert_eq!(attempt.text, "完整终态内容");
    let snapshot = serde_json::to_value(session.history()).unwrap();
    let id = session.selected().unwrap().id.clone();
    session.new_conversation();
    session.delete_conversation(&id);
    session.select_conversation(&id);
    session.select_model(model("blocked"));
    session.set_draft("blocked".into());
    session.report("不能隐藏保存错误".into());
    session.send(connection(&url, "source"));
    session.retry(connection(&url, "source"));
    session.retry_with(connection(&url, "source"));
    assert_eq!(serde_json::to_value(session.history()).unwrap(), snapshot);
    assert!(session.notice().contains("保存失败"));
    assert!(!session.can_close());
    restore_writes(&path);
    session.retry_save();
    assert!(!session.save_failed());
    let database = Connection::open(&path).unwrap();
    let (text, status): (String, String) = database.query_row("SELECT text,status FROM attempts", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert_eq!(text, "完整终态内容");
    assert_eq!(status, "completed");
    worker.join().unwrap();
}

#[test]
fn cancelled_and_missing_terminal_attempts_do_not_enter_context() {
    let root = Directory::new();
    let mut session = setup(&root.path());
    let (url, requests, worker) = server(vec![
        (delta("取消前的部分内容"), true),
        (delta("缺少终止帧的内容"), false),
        (response("成功"), false),
    ]);
    session.set_draft("取消的问题".into());
    session.send(connection(&url, "source"));
    wait_until(&mut session, |s| !s.selected().unwrap().turns[0].attempts[0].text.is_empty());
    let snapshot = serde_json::to_value(session.history()).unwrap();
    session.set_draft("busy blocked".into());
    session.select_model(model("blocked"));
    session.delete_conversation(&session.selected().unwrap().id.clone());
    session.new_conversation();
    assert_eq!(serde_json::to_value(session.history()).unwrap(), snapshot);
    assert!(!session.can_close());
    complete(&mut session);
    assert_eq!(session.selected().unwrap().turns[0].attempts[0].status, TurnStatus::Cancelled);
    assert_eq!(session.selected().unwrap().turns[0].attempts[0].text, "取消前的部分内容");
    session.set_draft("失败的问题".into());
    session.send(connection(&url, "source"));
    complete(&mut session);
    assert_eq!(session.selected().unwrap().turns[1].attempts[0].status, TurnStatus::Failed);
    session.set_draft("新的问题".into());
    session.send(connection(&url, "source"));
    complete(&mut session);
    for _ in 0..3 {
        let sent = requests.recv().unwrap();
        assert_eq!(sent["messages"].as_array().unwrap().len(), 1);
    }
    worker.join().unwrap();
}

#[test]
fn failed_retry_save_rolls_back_attempt_without_touching_prior_answer() {
    let root = Directory::new();
    let path = root.path();
    let mut session = setup(&path);
    let (url, requests, worker) = server(vec![(response("保留的回答"), false)]);
    session.set_draft("问题".into());
    session.send(connection(&url, "source"));
    complete(&mut session);
    requests.recv().unwrap();
    worker.join().unwrap();
    session.set_draft("还未发送的草稿".into());
    let snapshot = serde_json::to_value(session.history()).unwrap();
    block_writes(&path);
    session.retry(connection(&url, "source"));
    assert!(session.save_failed());
    assert!(!session.busy());
    assert_eq!(serde_json::to_value(session.history()).unwrap(), snapshot);
    restore_writes(&path);
    session.retry_save();
}

#[test]
fn drop_drains_a_full_bounded_queue_and_releases_lock() {
    let root = Directory::new();
    let path = root.path();
    let wakes = Arc::new(AtomicUsize::new(0));
    let wake_counter = wakes.clone();
    let mut session = ChatSession::open(&path, Arc::new(move || { wake_counter.fetch_add(1, Ordering::Relaxed); })).unwrap();
    session.new_conversation();
    session.select_model(model("source"));
    let (url, requests, worker) = server(vec![(response(&"x".repeat(1024 * 1024)), false)]);
    session.set_draft("大回答".into());
    session.send(connection(&url, "source"));
    requests.recv_timeout(Duration::from_secs(10)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while wakes.load(Ordering::Relaxed) < 64 {
        assert!(Instant::now() < deadline, "worker 未填满队列");
        thread::sleep(Duration::from_millis(2));
    }
    drop(session);
    worker.join().unwrap();
    let reopened = open(&path);
    let attempt = &reopened.selected().unwrap().turns[0].attempts[0];
    assert_ne!(attempt.status, TurnStatus::Running);
    assert_eq!(attempt.text.len(), 1024 * 1024);
}

#[test]
fn output_limit_preserves_prior_deltas_and_marks_failure() {
    let root = Directory::new();
    let mut session = setup(&root.path());
    let body = format!("{}data: [DONE]\n\n", delta(&"界".repeat(4096)).repeat(86));
    let (url, requests, worker) = server(vec![(body, false)]);
    session.set_draft("输出边界".into());
    session.send(connection(&url, "source"));
    complete(&mut session);
    requests.recv().unwrap();
    let attempt = &session.selected().unwrap().turns[0].attempts[0];
    assert_eq!(attempt.status, TurnStatus::Failed);
    assert!(!attempt.text.is_empty());
    assert!(attempt.text.len() <= 1024 * 1024);
    assert!(attempt.error.as_ref().unwrap().contains("1 MiB"));
    worker.join().unwrap();
}

#[test]
fn mismatched_connection_never_accepts_a_turn() {
    let root = Directory::new();
    let mut session = setup(&root.path());
    session.set_draft("不应发送".into());
    let mut wrong_protocol = connection("http://127.0.0.1:1/v1", "source");
    wrong_protocol.protocol = Protocol::AnthropicMessages;
    session.send(wrong_protocol);
    assert!(!session.busy());
    assert!(!session.save_failed());
    assert!(session.selected().unwrap().turns.is_empty());
    assert_eq!(session.selected().unwrap().draft, "不应发送");
    assert!(session.notice().contains("不匹配"));
}

#[test]
fn empty_terminal_reply_fails_and_does_not_poison_next_context() {
    let root = Directory::new();
    let mut session = setup(&root.path());
    let (url, requests, worker) = server(vec![(response(" \n"), false), (response("正常回答"), false)]);
    session.set_draft("空回答的问题".into());
    session.send(connection(&url, "source"));
    complete(&mut session);
    let attempt = &session.selected().unwrap().turns[0].attempts[0];
    assert_eq!(attempt.status, TurnStatus::Failed);
    assert!(attempt.error.as_ref().unwrap().contains("空回答"));
    session.set_draft("继续提问".into());
    session.send(connection(&url, "source"));
    complete(&mut session);
    requests.recv().unwrap();
    let second = requests.recv().unwrap();
    assert_eq!(second["messages"], json!([{"role":"user","content":"继续提问"}]));
    assert_eq!(session.selected().unwrap().turns[1].attempts[0].status, TurnStatus::Completed);
    worker.join().unwrap();
}

fn legacy_history() -> Value {
    json!({"schemaVersion":1,"value":{"selected":"c2","model":model("chosen"),"conversations":[
        {"id":"c2","title":"第二个标识先出现","draft":"草稿","turns":[
            {"id":"t2","input":"问题二","attempts":[
                {"id":"a2","model":model("source"),"text":"旧回答","status":"completed","error":null},
                {"id":"a1","model":model("other"),"text":"部分回答","status":"cancelled","error":"停止"}
            ]},
            {"id":"t1","input":"问题一","attempts":[]}
        ]},
        {"id":"c1","title":"另一个对话","turns":[]}
    ]}})
}

#[test]
fn migration_preserves_order_and_empty_database_does_not_revive_backup() {
    let root = Directory::new();
    let path = root.path();
    let legacy = path.with_file_name("chat-history.json");
    let bytes = serde_json::to_vec(&legacy_history()).unwrap();
    fs::write(&legacy, &bytes).unwrap();
    let mut session = open(&path);
    assert_eq!(session.history().conversations.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["c2", "c1"]);
    assert_eq!(session.history().model.as_ref().unwrap().model, "chosen");
    let conversation = session.selected().unwrap();
    assert_eq!(conversation.turns.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t2", "t1"]);
    assert_eq!(conversation.turns[0].attempts.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["a2", "a1"]);
    assert_eq!(conversation.turns[0].attempts[1].model.model, "other");
    let database = Connection::open(&path).unwrap();
    let rowid: i64 = database.query_row("SELECT rowid FROM attempts WHERE id='a2'", [], |row| row.get(0)).unwrap();
    database.execute_batch("CREATE TRIGGER no_reinsert BEFORE INSERT ON attempts WHEN NOT EXISTS(SELECT 1 FROM attempts WHERE id=NEW.id) BEGIN SELECT RAISE(ABORT,'unchanged answer was reinserted'); END;").unwrap();
    session.set_draft("新草稿".into());
    assert!(session.can_close());
    assert_eq!(database.query_row("SELECT rowid FROM attempts WHERE id='a2'", [], |row| row.get::<_, i64>(0)).unwrap(), rowid);
    session.delete_conversation("c2");
    session.delete_conversation("c1");
    assert_eq!(count(&path, "attempts"), 0);
    assert_eq!(count(&path, "turns"), 0);
    drop(session);
    // 旧文件仍存在且损坏，也不能影响已完成迁移的空数据库。
    assert_eq!(fs::read(&legacy).unwrap(), bytes);
    fs::write(&legacy, b"not JSON").unwrap();
    assert!(open(&path).history().conversations.is_empty());
}

#[test]
fn empty_first_open_records_import_decision() {
    let root = Directory::new();
    let path = root.path();
    drop(open(&path));
    fs::write(path.with_file_name("chat-history.json"), serde_json::to_vec(&legacy_history()).unwrap()).unwrap();
    assert!(open(&path).history().conversations.is_empty());
}

#[test]
fn invalid_import_and_database_write_failure_leave_no_success_marker() {
    let root = Directory::new();
    let path = root.path();
    let legacy = path.with_file_name("chat-history.json");
    let mut invalid = legacy_history();
    invalid["value"]["conversations"][1]["id"] = json!("c2");
    let invalid_bytes = serde_json::to_vec(&invalid).unwrap();
    fs::write(&legacy, &invalid_bytes).unwrap();
    assert!(ChatSession::open(&path, Arc::new(|| {})).is_err());
    assert_eq!(count(&path, "conversations"), 0);
    assert_eq!(count(&path, "chat_meta"), 0);
    assert_eq!(fs::read(&legacy).unwrap(), invalid_bytes);
    let bytes = serde_json::to_vec(&legacy_history()).unwrap();
    fs::write(&legacy, &bytes).unwrap();
    block_writes(&path);
    assert!(ChatSession::open(&path, Arc::new(|| {})).is_err());
    assert_eq!(count(&path, "conversations"), 0);
    assert_eq!(count(&path, "turns"), 0);
    assert_eq!(count(&path, "chat_meta"), 0);
    assert_eq!(fs::read(&legacy).unwrap(), bytes);
    restore_writes(&path);
    assert_eq!(open(&path).history().conversations.len(), 2);
}

#[test]
fn corrupt_legacy_and_future_database_are_not_silently_replaced() {
    let root = Directory::new();
    let path = root.path();
    let legacy = path.with_file_name("chat-history.json");
    fs::write(&legacy, b"{broken").unwrap();
    assert!(ChatSession::open(&path, Arc::new(|| {})).is_err());
    assert_eq!(count(&path, "chat_meta"), 0);
    let database = Connection::open(&path).unwrap();
    database.execute_batch("PRAGMA user_version=3; CREATE TABLE future_data(value TEXT); INSERT INTO future_data VALUES('保留');").unwrap();
    assert!(ChatSession::open(&path, Arc::new(|| {})).is_err());
    assert_eq!(database.query_row("SELECT value FROM future_data", [], |row| row.get::<_, String>(0)).unwrap(), "保留");
    assert_eq!(database.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), 3);
}

#[test]
fn usage_round_snapshots_survive_database_round_trip() {
    let root = Directory::new();
    let path = root.path();
    let mut history = legacy_history();
    let rounds = json!([
        {"usage":{"input_tokens":123,"output_tokens":45,"cache_read_tokens":67,"cache_write_tokens":8},
         "pricing":{"currency":"USD","tiers":[{"min_input_tokens":0,"input":1.0,"output":2.0,"cache_read":0.25,"cache_write":1.25}]},"cost":0.00017775},
        {"usage":null,"pricing":null,"cost":null}
    ]);
    history["value"]["conversations"][0]["turns"][0]["attempts"][0]["rounds"] = rounds.clone();
    fs::write(path.with_file_name("chat-history.json"), serde_json::to_vec(&history).unwrap()).unwrap();
    drop(open(&path));
    let session = open(&path);
    assert_eq!(serde_json::to_value(&session.selected().unwrap().turns[0].attempts[0].rounds).unwrap(), rounds);
}
#[test]
fn failed_attempt_preserves_prior_rounds_and_reopens_as_failed() {
    let root = Directory::new();
    let path = root.path();
    let mut history = legacy_history();
    history["value"]["conversations"][0]["turns"][0]["attempts"][0]["status"] = json!("running");
    history["value"]["conversations"][0]["turns"][0]["attempts"][0]["rounds"] = json!([{"usage":null,"pricing":null,"cost":null}]);
    fs::write(path.with_file_name("chat-history.json"), serde_json::to_vec(&history).unwrap()).unwrap();
    let session = open(&path);
    let attempt = &session.selected().unwrap().turns[0].attempts[0];
    assert_eq!(attempt.status, TurnStatus::Failed);
    assert_eq!(attempt.rounds.len(), 1);
    assert!(attempt.error.as_deref().unwrap().contains("中断"));
}

#[test]
fn invalid_import_identifiers_and_model_sources_never_commit() {
    let changes = [
        ("/value/conversations/0/id", json!("  ")),
        ("/value/conversations/0/turns/0/id", json!("")),
        ("/value/conversations/0/turns/1/id", json!("t2")),
        ("/value/conversations/0/turns/0/attempts/0/id", json!("\n")),
        ("/value/conversations/0/turns/0/attempts/1/id", json!("a2")),
        ("/value/conversations/0/turns/0/attempts/0/model/provider", json!("")),
        ("/value/conversations/0/turns/0/attempts/0/model/endpoint", json!(" ")),
        ("/value/model/model", json!("")),
    ];
    for (pointer, value) in changes {
        let root = Directory::new();
        let path = root.path();
        let legacy = path.with_file_name("chat-history.json");
        let mut history = legacy_history();
        *history.pointer_mut(pointer).unwrap() = value;
        let bytes = serde_json::to_vec(&history).unwrap();
        fs::write(&legacy, &bytes).unwrap();
        assert!(ChatSession::open(&path, Arc::new(|| {})).is_err(), "{pointer}");
        assert_eq!(count(&path, "chat_meta"), 0, "{pointer}");
        assert_eq!(count(&path, "conversations"), 0, "{pointer}");
        assert_eq!(count(&path, "turns"), 0, "{pointer}");
        assert_eq!(count(&path, "attempts"), 0, "{pointer}");
        assert_eq!(fs::read(&legacy).unwrap(), bytes);
    }
}

#[test]
fn legacy_stale_selection_is_repaired_without_discarding_history() {
    let root = Directory::new();
    let path = root.path();
    let mut history = legacy_history();
    history["value"]["selected"] = json!("removed");
    fs::write(path.with_file_name("chat-history.json"), serde_json::to_vec(&history).unwrap()).unwrap();
    let session = open(&path);
    assert_eq!(session.history().conversations.len(), 2);
    assert_eq!(session.selected().unwrap().id, "c2");
}

#[test]
fn streamed_usage_and_original_price_survive_request_and_reopen() {
    use lumakit_ai::model_config::{ModelPricing, PriceTier};
    let root = Directory::new();
    let path = root.path();
    let mut session = setup(&path);
    let usage = json!({"choices":[],"usage":{"prompt_tokens":123,"completion_tokens":45,"prompt_tokens_details":{"cached_tokens":67}}});
    let body = format!("{}data: {usage}\n\ndata: [DONE]\n\n", delta("含用量回答"));
    let (url, requests, worker) = server(vec![(body, false)]);
    let mut connection = connection(&url, "source");
    let pricing = ModelPricing { currency: "CNY".into(), tiers: vec![PriceTier {
        min_input_tokens: 0, input: 1.0, output: 2.0, cache_read: Some(0.25), cache_write: Some(1.25),
    }] };
    connection.settings.pricing = Some(pricing.clone());
    session.set_draft("用量问题".into());
    session.send(connection);
    complete(&mut session);
    requests.recv().unwrap();
    worker.join().unwrap();
    let attempt = &session.selected().unwrap().turns[0].attempts[0];
    assert_eq!(attempt.status, TurnStatus::Completed);
    assert_eq!(attempt.rounds.len(), 1);
    let round = &attempt.rounds[0];
    let usage = round.usage.as_ref().unwrap();
    assert_eq!((usage.input_tokens, usage.output_tokens, usage.cache_read_tokens, usage.cache_write_tokens), (123, 45, 67, 0));
    assert_eq!(round.pricing.as_ref(), Some(&pricing));
    assert_eq!(round.cost, pricing.cost(usage));
    let saved = serde_json::to_value(&attempt.rounds).unwrap();
    assert!(attempt.reasoning.is_empty());
    drop(session);
    let reopened = open(&path);
    assert_eq!(serde_json::to_value(&reopened.selected().unwrap().turns[0].attempts[0].rounds).unwrap(), saved);
}
#[test]
fn selected_attempt_and_candidate_mutations_survive_restart() {
    let root = Directory::new();
    let path = root.path();
    let mut session = setup(&path);
    let (url, _requests, worker) = server(vec![(response("首答"), false)]);
    session.set_draft("问题".into());
    session.send(connection(&url, "source"));
    complete(&mut session);
    let _ = worker.join();
    let conversation = session.selected().unwrap().id.clone();
    let turn = session.selected().unwrap().turns[0].id.clone();
    let first = session.selected().unwrap().turns[0].attempts[0].id.clone();
    assert!(session.select_attempt(&conversation, &turn, &first).is_ok());
    assert!(session.edit_attempt(&conversation, &turn, &first, "首答", "编辑后").is_ok());
    assert!(session.edit_attempt(&conversation, &turn, &first, "首答", "冲突").is_err());
    drop(session);
    let reopened = open(&path);
    assert_eq!(reopened.selected().unwrap().turns[0].selected_attempt.as_deref(), Some(first.as_str()));
    assert_eq!(reopened.selected().unwrap().turns[0].attempts[0].text, "编辑后");
}
#[test]
fn search_sources_and_terminal_states_survive_restart() {
    let root = Directory::new();
    let path = root.path();
    let mut session = setup(&path);
    let (url, _requests, worker) = server(vec![(response("回答"), false)]);
    session.set_draft("问题".into());
    session.send(connection(&url, "source"));
    complete(&mut session);
    worker.join().unwrap();
    let conversation = session.selected().unwrap().id.clone();
    let turn = session.selected().unwrap().turns[0].id.clone();
    let attempt = session.selected().unwrap().turns[0].attempts[0].id.clone();
    session.search_start(&conversation, &turn, &attempt, "查询", "provider").unwrap();
    session.search_result(&conversation, &turn, &attempt, lumakit_ai::search::SearchResult {
        query: "查询".into(), provider: "provider".into(), hits: vec![lumakit_ai::search::SearchHit {
            title: "来源".into(), url: "https://example.com/a".into(), snippet: "摘要".into(),
        }],
    }).unwrap();
    drop(session);
    let reopened = open(&path);
    let saved = &reopened.selected().unwrap().turns[0].attempts[0];
    assert_eq!(saved.text, "回答");
    assert_eq!(saved.search.status, lumakit_ai::chat::SearchStatus::Complete);
    assert_eq!(saved.search.hits[0].url, "https://example.com/a");
}
#[test]
fn tool_events_and_errors_survive_restart() {
    let root = Directory::new();
    let path = root.path();
    let mut session = setup(&path);
    let (url, _requests, worker) = server(vec![(response("回答"), false)]);
    session.set_draft("问题".into());
    session.send(connection(&url, "source"));
    complete(&mut session);
    worker.join().unwrap();
    let conversation = session.selected().unwrap().id.clone();
    let turn = session.selected().unwrap().turns[0].id.clone();
    let attempt = session.selected().unwrap().turns[0].attempts[0].id.clone();
    session.tool_start(&conversation, &turn, &attempt, lumakit_ai::ToolCall { id: "call-1".into(), name: "lookup".into(), arguments: json!({"q":"x"}) }).unwrap();
    session.tool_approval(&conversation, &turn, &attempt, "call-1", lumakit_ai::Approval::Allow).unwrap();
    session.tool_error(&conversation, &turn, &attempt, "call-1", "上游失败").unwrap();
    drop(session);
    let reopened = open(&path);
    let event = &reopened.selected().unwrap().turns[0].attempts[0].tool_events[0];
    assert_eq!(event.approval, Some(lumakit_ai::Approval::Allow));
    assert_eq!(event.error.as_deref(), Some("上游失败"));
    assert!(event.is_error);
}
#[test]
fn tool_result_state_is_not_answer_text() {
    let root = Directory::new();
    let path = root.path();
    let mut session = setup(&path);
    let (url, _requests, worker) = server(vec![(response("回答"), false)]);
    session.set_draft("问题".into());
    session.send(connection(&url, "source"));
    complete(&mut session);
    worker.join().unwrap();
    let conversation = session.selected().unwrap().id.clone();
    let turn = session.selected().unwrap().turns[0].id.clone();
    let attempt = session.selected().unwrap().turns[0].attempts[0].id.clone();
    session.tool_start(&conversation, &turn, &attempt, lumakit_ai::ToolCall { id: "call-2".into(), name: "lookup".into(), arguments: json!({}) }).unwrap();
    session.tool_approval(&conversation, &turn, &attempt, "call-2", lumakit_ai::Approval::Allow).unwrap();
    session.tool_result(&conversation, &turn, &attempt, "call-2", lumakit_ai::ToolOutput { text: "结果".into(), is_error: false }).unwrap();
    let saved = &session.selected().unwrap().turns[0].attempts[0];
    assert_eq!(saved.text, "回答");
    assert_eq!(saved.tool_events[0].result.as_deref(), Some("结果"));
    assert!(!saved.tool_events[0].is_error);
}

#[test]
fn painting_two_image_cap_and_later_text_failure_keep_persisted_media() {
    use lumakit_ai::chat::ComposerSettings;
    let root = Directory::new();
    let path = root.path();
    let mut session = setup(&path);
    session.set_composer(ComposerSettings { image_mode: true, image_model: Some(model("image")), ..Default::default() });
    session.set_draft("两张图".into());
    let calls = tool_calls(&[("one", json!({"prompt":"一"})), ("two", json!({"prompt":"二","size":"1024x1536"})), ("three", json!({"prompt":"三"}))]);
    let (url, requests, worker) = server_steps(vec![
        ("/v1/chat/completions", 200, calls, false),
        ("/v1/images/generations", 200, image_response(), false),
        ("/v1/images/generations", 200, image_response(), false),
        ("/v1/chat/completions", 503, "最终文字失败".into(), false),
    ]);
    session.send_with_painting(connection(&url, "source"), painting_connection(&url, "image"));
    complete(&mut session);
    let sent: Vec<_> = (0..4).map(|_| requests.recv().unwrap()).collect();
    assert_eq!(sent[2]["size"], "1024x1536");
    let attempt = &session.selected().unwrap().turns[0].attempts[0];
    assert_eq!(attempt.status, TurnStatus::Failed);
    assert_eq!(attempt.generated.len(), 2);
    assert!(!attempt.tool_events[0].is_error);
    assert!(!attempt.tool_events[1].is_error);
    assert!(attempt.tool_events[2].is_error);
    assert!(attempt.error.as_ref().unwrap().contains("503"));
    let ids: Vec<_> = attempt.generated.iter().map(|image| image.id.clone()).collect();
    worker.join().unwrap();
    drop(session);
    let reopened = open(&path);
    let saved = &reopened.selected().unwrap().turns[0].attempts[0];
    assert_eq!(saved.generated.len(), 2);
    assert_eq!(saved.tool_events.len(), 3);
    for id in ids { assert!(reopened.media_path(&id).unwrap().is_file()); }
}

#[test]
fn painting_cancellation_during_image_or_continuation_preserves_completed_work() {
    use lumakit_ai::chat::ComposerSettings;
    for during_image in [true, false] {
        let root = Directory::new();
        let mut session = setup(&root.path());
        session.set_composer(ComposerSettings { image_mode: true, image_model: Some(model("image")), ..Default::default() });
        session.set_draft("可取消绘图".into());
        let mut steps = vec![("/v1/chat/completions", 200, tool_calls(&[("paint", json!({"prompt":"一张图"}))]), false)];
        if during_image { steps.push(("/v1/images/generations", 200, "{".into(), true)); }
        else {
            steps.push(("/v1/images/generations", 200, image_response(), false));
            steps.push(("/v1/chat/completions", 200, delta("生成后的部分说明"), true));
        }
        let (url, requests, worker) = server_steps(steps);
        session.send_with_painting(connection(&url, "source"), painting_connection(&url, "image"));
        requests.recv_timeout(Duration::from_secs(10)).unwrap();
        requests.recv_timeout(Duration::from_secs(10)).unwrap();
        if !during_image { wait_until(&mut session, |session| !session.selected().unwrap().turns[0].attempts[0].text.is_empty()); }
        session.cancel();
        complete(&mut session);
        let attempt = &session.selected().unwrap().turns[0].attempts[0];
        assert_eq!(attempt.status, TurnStatus::Cancelled);
        assert_eq!(attempt.generated.len(), usize::from(!during_image));
        assert_eq!(attempt.tool_events.len(), 1);
        assert_eq!(session.selected().unwrap().draft, "可取消绘图");
        worker.join().unwrap();
    }
}

#[test]
fn painting_insecure_result_is_error_tool_payload_without_retry() {
    use lumakit_ai::chat::ComposerSettings;
    let root = Directory::new();
    let mut session = setup(&root.path());
    session.set_composer(ComposerSettings { image_mode: true, image_model: Some(model("image")), ..Default::default() });
    session.set_draft("不安全图片".into());
    let (url, requests, worker) = server_steps(vec![
        ("/v1/chat/completions", 200, tool_calls(&[("paint", json!({"prompt":"画面"}))]), false),
        ("/v1/images/generations", 200, json!({"data":[{"url":"http://127.0.0.1:1/private"}]}).to_string(), false),
        ("/v1/chat/completions", 200, response("图片地址不安全，生成失败"), false),
    ]);
    session.send_with_painting(connection(&url, "source"), painting_connection(&url, "image"));
    complete(&mut session);
    for _ in 0..3 { requests.recv().unwrap(); }
    let attempt = &session.selected().unwrap().turns[0].attempts[0];
    assert!(attempt.generated.is_empty());
    assert!(attempt.tool_events[0].is_error);
    assert!(attempt.tool_events[0].error.as_ref().unwrap().contains("HTTPS"));
    worker.join().unwrap();
}

#[test]
fn legacy_failed_direct_image_retry_explicitly_migrates_chat_source() {
    let root = Directory::new();
    let path = root.path();
    let legacy = json!({"schemaVersion":1,"value":{"selected":"legacy-c","model":model("source"),"conversations":[{"id":"legacy-c","title":"旧绘图","draft":"","turns":[{"id":"legacy-t","input":"原问题不能丢失","attempts":[{"id":"legacy-a","model":model("old-image"),"text":"","status":"failed","error":"原图片接口失败"}]}]}]}});
    fs::write(path.with_file_name("chat-history.json"), serde_json::to_vec(&legacy).unwrap()).unwrap();
    drop(open(&path));
    let composer = json!({"drafts":{},"turns":{"legacy-t":{"settings":{"image_mode":true,"image_model":null,"web_search":false,"reasoning":null},"attachments":[]}}});
    Connection::open(&path).unwrap().execute("INSERT OR REPLACE INTO chat_meta(key,value) VALUES('composer',?1)", [composer.to_string()]).unwrap();
    let mut session = open(&path);
    assert_eq!(session.request_model(true).unwrap().model, "old-image");
    assert_eq!(session.request_painting_model(true).unwrap().unwrap().model, "old-image");
    let (url, requests, worker) = server_steps(vec![
        ("/v1/chat/completions", 200, tool_calls(&[("migrated", json!({"prompt":"原问题对应的画面"}))]), false),
        ("/v1/images/generations", 200, image_response(), false),
        ("/v1/chat/completions", 200, response("迁移后生成成功"), false),
    ]);
    session.retry_with_model_and_painting(connection(&url, "source"), painting_connection(&url, "old-image"));
    complete(&mut session);
    assert_eq!(requests.recv().unwrap()["messages"][0]["content"], "原问题不能丢失");
    assert_eq!(requests.recv().unwrap()["model"], "old-image");
    requests.recv().unwrap();
    let attempts = &session.selected().unwrap().turns[0].attempts;
    assert_eq!(attempts[0].model.model, "old-image");
    assert_eq!(attempts[1].model.model, "source");
    assert_eq!(attempts[1].status, TurnStatus::Completed);
    worker.join().unwrap();
    drop(session);
    let session = open(&path);
    assert_eq!(session.request_model(true).unwrap().model, "source");
    assert_eq!(session.request_painting_model(true).unwrap().unwrap().model, "old-image");
}

#[test]
fn painting_declared_tool_incapability_rejects_before_network() {
    use lumakit_ai::chat::ComposerSettings;
    let root = Directory::new();
    let mut session = setup(&root.path());
    session.set_composer(ComposerSettings { image_mode: true, image_model: Some(model("image")), ..Default::default() });
    session.set_draft("不支持工具".into());
    let mut chat = connection("http://127.0.0.1:1/v1", "source");
    chat.settings.supports_tools = Some(false);
    session.send_with_painting(chat, painting_connection("http://127.0.0.1:1/v1", "image"));
    assert!(!session.busy());
    assert!(session.notice().contains("不支持工具"));
    assert!(session.selected().unwrap().turns.is_empty());
    assert_eq!(session.selected().unwrap().draft, "不支持工具");
}

#[test]
fn painting_responses_continuation_keeps_vision_web_and_reasoning() {
    use lumakit_ai::{chat::ComposerSettings, model_config::{ReasoningConfig, ReasoningDialect, ReasoningEffort}};
    let root = Directory::new();
    let mut session = setup(&root.path());
    let image = root.0.join("input.png");
    fs::write(&image, b"\x89PNG\r\n\x1a\n").unwrap();
    session.add_attachment(&image).unwrap();
    session.select_model(ModelRef { protocol: Protocol::OpenAiResponses, ..model("source") });
    session.set_composer(ComposerSettings { web_search:true, reasoning:Some(ReasoningEffort::High), image_mode:true, image_model:Some(model("image")) });
    session.set_draft("参考图片后绘图".into());
    let reasoning = json!({"type":"reasoning","id":"rs-original","summary":[{"type":"summary_text","text":"独立推理"}],"encrypted_content":"opaque"});
    let first = json!({"status":"completed","output":[reasoning.clone(),{"type":"function_call","id":"item","call_id":"paint","name":"generate_image","arguments":"{\"prompt\":\"完整构图\"}"}]}).to_string();
    let final_response = json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"完成绘图"}]}]}).to_string();
    let (url, requests, worker) = server_steps(vec![
        ("/v1/responses", 200, first, false),
        ("/v1/images/generations", 200, image_response(), false),
        ("/v1/responses", 200, final_response, false),
    ]);
    let mut chat = connection(&url, "source");
    chat.protocol = Protocol::OpenAiResponses;
    chat.settings.supports_streaming = false;
    chat.settings.supports_vision = true;
    chat.settings.reasoning = Some(ReasoningConfig { dialect:ReasoningDialect::OpenAi, efforts:vec![ReasoningEffort::High], budget_tokens:None });
    session.send_with_painting(chat, painting_connection(&url, "image"));
    complete(&mut session);
    let initial = requests.recv().unwrap();
    requests.recv().unwrap();
    let continuation = requests.recv().unwrap();
    assert_eq!(initial["input"][0]["content"][1]["type"], "input_image");
    for body in [&initial, &continuation] {
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["tools"][0]["name"], "generate_image");
        assert_eq!(body["tools"][1]["type"], "web_search");
    }
    assert_eq!(continuation["input"][0], initial["input"][0]);
    assert_eq!(continuation["input"][1], reasoning);
    assert_eq!(continuation["input"][3]["type"], "function_call_output");
    let attempt = &session.selected().unwrap().turns[0].attempts[0];
    assert_eq!(attempt.status, TurnStatus::Completed);
    assert_eq!(attempt.reasoning, vec![reasoning]);
    assert_eq!(attempt.generated.len(), 1);
    worker.join().unwrap();
}
