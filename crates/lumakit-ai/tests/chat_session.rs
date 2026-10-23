use lumakit_ai::{
    AiConnection, ModelRef, Protocol,
    chat::{ChatSession, History},
    history::TurnStatus,
};
use lumakit_support::{load_versioned, new_id};
use serde_json::{Value, json};
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
    fn path(&self) -> PathBuf { self.0.join("chat-history.json") }
}
impl Drop for Directory {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}
fn open(path: &Path) -> ChatSession { ChatSession::open(path, Arc::new(|| {})).unwrap() }
fn model(name: &str) -> ModelRef {
    ModelRef { provider: "provider-id".into(), endpoint: "endpoint-id-not-url".into(), model: name.into(), protocol: Protocol::OpenAiChat }
}
fn connection(url: &str, name: &str) -> AiConnection {
    AiConnection { base_url: url.into(), model: name.into(), protocol: Protocol::OpenAiChat, api_key: None }
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

// 真 HTTP 回环：读取宿主实际发送的正文，保持响应可覆盖停止和缺失结束帧。
fn server(responses: Vec<(String, bool)>) -> (String, mpsc::Receiver<Value>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        for (body, hold) in responses {
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
                    assert!(headers.starts_with("POST /v1/chat/completions HTTP/1.1"));
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
            let header = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len() + if hold { 100 } else { 0 });
            if socket.write_all(header.as_bytes()).is_ok() && socket.write_all(body.as_bytes()).is_ok() && hold {
                let mut byte = [0];
                let _ = socket.read(&mut byte);
            }
        }
    });
    (format!("http://{address}/v1"), rx, worker)
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
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
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
}

#[test]
fn unknown_versions_leave_original_bytes_untouched() {
    let root = Directory::new();
    let path = root.path();
    for version in [0, 2] {
        let bytes = serde_json::to_vec(&json!({"schemaVersion":version,"value":{"conversations":[],"selected":null,"model":null}})).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert!(ChatSession::open(&path, Arc::new(|| {})).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn preflight_save_failure_rolls_back_and_never_sends_or_duplicates() {
    let root = Directory::new();
    let path = root.path();
    let backup = root.0.join("original.json");
    let mut session = setup(&path);
    session.set_draft("  原始问题\n ".into());
    fs::rename(&path, &backup).unwrap();
    let original = fs::read(&backup).unwrap();
    fs::create_dir(&path).unwrap();
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
    assert_eq!(fs::read(&backup).unwrap(), original);
    fs::remove_dir(&path).unwrap();
    fs::rename(&backup, &path).unwrap();
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
    let backup = root.0.join("accepted.json");
    let mut session = setup(&path);
    let (url, requests, worker) = server(vec![(response("完整终态内容"), false)]);
    session.set_draft("问题".into());
    session.send(connection(&url, "source"));
    requests.recv_timeout(Duration::from_secs(10)).unwrap();
    fs::rename(&path, &backup).unwrap();
    fs::create_dir(&path).unwrap();
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
    fs::remove_dir(&path).unwrap();
    fs::rename(&backup, &path).unwrap();
    session.retry_save();
    assert!(!session.save_failed());
    let (_, saved) = load_versioned::<History>(&path, 1).unwrap().unwrap();
    assert_eq!(saved.conversations[0].turns[0].attempts[0].text, "完整终态内容");
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
    let backup = root.0.join("before-retry.json");
    fs::rename(&path, &backup).unwrap();
    fs::create_dir(&path).unwrap();
    session.retry(connection(&url, "source"));
    assert!(session.save_failed());
    assert!(!session.busy());
    assert_eq!(serde_json::to_value(session.history()).unwrap(), snapshot);
    fs::remove_dir(&path).unwrap();
    fs::rename(&backup, &path).unwrap();
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
