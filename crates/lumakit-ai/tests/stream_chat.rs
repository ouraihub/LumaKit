//! 纯文本流式聊天的请求编码、响应处理及失败边界集成测试。
use lumakit_ai::{AiConnection, AiError, ChatMessage, Protocol, stream_chat};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

const KEY: &str = "test-secret-not-for-display";

fn history() -> Vec<ChatMessage> {
    vec![
        ChatMessage::User("之前的问题".into()),
        ChatMessage::Assistant("之前的回答".into()),
        ChatMessage::User("现在的问题".into()),
    ]
}

fn connection(base_url: String, protocol: Protocol) -> AiConnection {
    AiConnection {
        base_url,
        protocol,
        api_key: Some(Zeroizing::new(KEY.into())),
        model: "loopback-model".into(),
        settings: Default::default(),
    }
}

fn event(value: Value) -> String {
    format!("data: {value}\n\n")
}

fn delta(protocol: Protocol, text: &str) -> String {
    event(match protocol {
        Protocol::AnthropicMessages => json!({
            "type":"content_block_delta", "index":0,
            "delta":{"type":"text_delta", "text":text}
        }),
        Protocol::OpenAiChat => json!({"choices":[{"delta":{"content":text}}]}),
        Protocol::OpenAiResponses => json!({"type":"response.output_text.delta","delta":text}),
    })
}

fn done(protocol: Protocol) -> String {
    match protocol {
        Protocol::AnthropicMessages => event(json!({"type":"message_stop"})),
        Protocol::OpenAiChat => "data: [DONE]\n\n".into(),
        Protocol::OpenAiResponses => event(json!({"type":"response.completed"})),
    }
}

// 真实 TCP/HTTP；分段写入刻意拆开 UTF-8 字节，覆盖流式解析边界。
fn server(
    protocol: Protocol,
    status: u16,
    body: String,
    hold: bool,
    ready: Option<tokio::sync::oneshot::Sender<()>>,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        socket.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut request = Vec::new();
        let (header_end, length) = loop {
            let mut chunk = [0; 4096];
            let count = socket.read(&mut chunk).unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&chunk[..count]);
            if let Some(offset) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..offset]);
                let length = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                }).unwrap();
                let path = match protocol {
                    Protocol::AnthropicMessages => "messages",
                    Protocol::OpenAiChat => "chat/completions",
                    Protocol::OpenAiResponses => "responses",
                };
                assert!(headers.starts_with(&format!("POST /v1/{path} HTTP/1.1")));
                assert!(headers.contains(KEY));
                if protocol == Protocol::AnthropicMessages {
                    assert!(headers.to_ascii_lowercase().contains("anthropic-version: 2023-06-01"));
                }
                break (offset + 4, length);
            }
        };
        while request.len() < header_end + length {
            let mut chunk = [0; 4096];
            let count = socket.read(&mut chunk).unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&chunk[..count]);
        }
        let payload: Value = serde_json::from_slice(&request[header_end..header_end + length]).unwrap();
        assert_eq!(payload["model"], "loopback-model");
        let json_response = body.starts_with('{');
        assert_eq!(payload["stream"], !json_response);
        if json_response {
            let limit = if protocol == Protocol::OpenAiResponses { "max_output_tokens" } else { "max_tokens" };
            assert_eq!(payload[limit], 8192);
            assert!(payload.get("stream_options").is_none());
            match protocol {
                Protocol::OpenAiChat => {
                    assert_eq!(payload["reasoning_effort"], "high");
                    assert!(payload.get("tools").is_none());
                }
                Protocol::OpenAiResponses => {
                    assert_eq!(payload["reasoning"]["effort"], "high");
                    assert_eq!(payload["tools"][0]["type"], "web_search");
                }
                Protocol::AnthropicMessages => {
                    assert_eq!(payload["thinking"], json!({"type":"enabled","budget_tokens":1638}));
                    assert_eq!(payload["tools"][0]["type"], "web_search_20250305");
                }
            }
        } else { assert!(payload.get("tools").is_none()); }
        let messages = if protocol == Protocol::OpenAiResponses { &payload["input"] } else { &payload["messages"] };
        assert_eq!(messages.as_array().unwrap().len(), 3);
        assert_eq!(messages[0], json!({"role":"user","content":"之前的问题"}));
        assert_eq!(messages[2], json!({"role":"user","content":"现在的问题"}));
        assert_eq!(messages[1]["role"], "assistant");
        if protocol == Protocol::AnthropicMessages {
            assert_eq!(messages[1]["content"], json!([{"type":"text","text":"之前的回答"}]));
        } else {
            assert_eq!(messages[1]["content"], "之前的回答");
        }
        // status=0：已连接，但上游尚未返回响应头。
        if status != 0 {
            let content_type = if json_response { "application/json" } else { "text/event-stream" };
            write!(socket, "HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len() + usize::from(hold) * 100).unwrap();
            let chunk_size = if body.len() > 8192 { 4096 } else { 1 };
            for chunk in body.as_bytes().chunks(chunk_size) {
                if socket.write_all(chunk).is_err() {
                    break;
                }
            }
        }
        if let Some(ready) = ready {
            let _ = ready.send(());
        }
        if hold {
            let _ = socket.read_to_end(&mut Vec::new());
        }
    });
    (format!("http://{address}/v1"), worker)
}

async fn exercise(protocol: Protocol) {
    let body = delta(protocol, "你好") + &delta(protocol, "，世界🌏") + &done(protocol);
    let (url, worker) = server(protocol, 200, body, false, None);
    let mut deltas = Vec::new();
    let text = stream_chat(connection(url, protocol), history(), CancellationToken::new(), |delta| deltas.push(delta)).await.unwrap();
    assert_eq!(text, "你好，世界🌏");
    assert_eq!(deltas, ["你好", "，世界🌏"]);
    worker.join().unwrap();

    let (url, worker) = server(protocol, 200, delta(protocol, "未完成"), false, None);
    let error = stream_chat(connection(url, protocol), history(), CancellationToken::new(), |_| {}).await.unwrap_err();
    assert!(error.to_string().contains("stream ended before completion"));
    worker.join().unwrap();

    let tools = match protocol {
        Protocol::AnthropicMessages => event(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call","name":"unexpected"}})),
        Protocol::OpenAiChat => event(json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call","function":{"name":"unexpected","arguments":"{}"}}]}}]})),
        Protocol::OpenAiResponses => event(json!({"type":"response.output_item.added","item":{"type":"function_call","id":"item","call_id":"call","name":"unexpected"}})),
    };
    let (url, worker) = server(protocol, 200, tools + &done(protocol), false, None);
    let error = stream_chat(connection(url, protocol), history(), CancellationToken::new(), |_| {}).await.unwrap_err();
    assert!(error.to_string().contains("unexpected tool calls"));
    worker.join().unwrap();

    // 连接等待响应头、读取 SSE、读取错误正文，都必须可取消。
    for (status, body) in [(0, String::new()), (200, delta(protocol, "部分")), (401, String::new())] {
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (url, worker) = server(protocol, status, body, true, Some(ready_tx));
        let cancel = CancellationToken::new();
        let cancelling = cancel.clone();
        let cancel_worker = tokio::spawn(async move {
            ready_rx.await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancelling.cancel();
        });
        let error = tokio::time::timeout(Duration::from_secs(2), stream_chat(connection(url, protocol), history(), cancel, |_| {})).await.unwrap().unwrap_err();
        assert!(matches!(error, AiError::Cancelled));
        cancel_worker.await.unwrap();
        worker.join().unwrap();
    }

    let (url, worker) = server(protocol, 401, format!("rejected {KEY}"), false, None);
    let error = stream_chat(connection(url, protocol), history(), CancellationToken::new(), |_| {}).await.unwrap_err().to_string();
    assert!(error.contains("401") && error.contains("***"));
    assert!(!error.contains(KEY));
    worker.join().unwrap();

    let upstream_error = match protocol {
        Protocol::AnthropicMessages => json!({"type":"error","error":{"message":KEY}}),
        Protocol::OpenAiChat => json!({"error":{"message":KEY}}),
        Protocol::OpenAiResponses => json!({"type":"response.failed","response":{"error":{"message":KEY}}}),
    };
    let (url, worker) = server(protocol, 200, event(upstream_error), false, None);
    let error = stream_chat(connection(url, protocol), history(), CancellationToken::new(), |_| {}).await.unwrap_err().to_string();
    assert!(!error.contains(KEY));
    assert!(error.contains("***"));
    worker.join().unwrap();

    let (url, worker) = server(protocol, 401, "x".repeat(16384), false, None);
    let error = stream_chat(connection(url, protocol), history(), CancellationToken::new(), |_| {}).await.unwrap_err().to_string();
    assert!(error.contains("exceeds 8192 bytes"));
    assert!(error.len() < 200);
    worker.join().unwrap();

    let (url, worker) = server(protocol, 200, "x".repeat(4 * 1024 * 1024 + 1), false, None);
    let error = stream_chat(connection(url, protocol), history(), CancellationToken::new(), |_| {}).await.unwrap_err().to_string();
    assert!(error.contains("stream exceeds 4 MiB"));
    worker.join().unwrap();

    // 校验发生在联网之前，不能将不完整角色序列或超限内容发送出去。
    let invalid = [
        Vec::new(),
        vec![ChatMessage::Assistant("助手先发言".into())],
        vec![ChatMessage::User("问题".into()), ChatMessage::Assistant("回答".into())],
        vec![ChatMessage::User("甲".into()), ChatMessage::User("乙".into()), ChatMessage::User("丙".into())],
        vec![ChatMessage::User("  \n".into())],
        vec![ChatMessage::User("a".repeat(1024 * 1024 + 1))],
        vec![ChatMessage::User("a".repeat(1024 * 1024)), ChatMessage::Assistant("答".into()), ChatMessage::User("问".into())],
    ];
    for messages in invalid {
        let error = stream_chat(connection("http://127.0.0.1:1".into(), protocol), messages, CancellationToken::new(), |_| {}).await.unwrap_err();
        assert!(!error.to_string().contains("network request"));
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(stream_chat(connection("http://127.0.0.1:1".into(), protocol), history(), cancel, |_| {}).await, Err(AiError::Cancelled)));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let error = stream_chat(connection(format!("http://{address}/{KEY}"), protocol), history(), CancellationToken::new(), |_| {}).await.unwrap_err().to_string();
    assert!(!error.contains(KEY) && !error.contains(&address.to_string()) && !error.contains("http://"));
}

#[tokio::test]
async fn anthropic_text_chat_loopback() {
    exercise(Protocol::AnthropicMessages).await;
}

#[tokio::test]
async fn openai_chat_text_chat_loopback() {
    exercise(Protocol::OpenAiChat).await;
}

#[tokio::test]
async fn openai_responses_text_chat_loopback() {
    exercise(Protocol::OpenAiResponses).await;
    let protocol = Protocol::OpenAiResponses;
    let body = delta(protocol, "被截断的回复")
        + &event(json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}));
    let (url, worker) = server(protocol, 200, body, false, None);
    let mut partial = String::new();
    let error = stream_chat(connection(url, protocol), history(), CancellationToken::new(), |text| partial.push_str(&text)).await.unwrap_err();
    assert_eq!(partial, "被截断的回复");
    assert!(error.to_string().contains("provider response incomplete"));
    worker.join().unwrap();
}

#[tokio::test]
async fn configured_json_roundtrip_snapshots_real_usage_and_prices() {
    use lumakit_ai::{model_config::*, stream_chat_with_usage};
    for protocol in [Protocol::OpenAiChat, Protocol::OpenAiResponses, Protocol::AnthropicMessages] {
        // 固定协议响应不是请求回显：真实 HTTP 请求必须先通过 fixture 的字段断言。
        let body = match protocol {
            Protocol::OpenAiChat => json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"完成"}}],"usage":{"prompt_tokens":200,"completion_tokens":20,"prompt_tokens_details":{"cached_tokens":50}}}),
            Protocol::OpenAiResponses => json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"完成"}]}],"usage":{"input_tokens":200,"output_tokens":20,"input_tokens_details":{"cached_tokens":50}}}),
            Protocol::AnthropicMessages => json!({"stop_reason":"end_turn","content":[{"type":"text","text":"完成"}],"usage":{"input_tokens":150,"output_tokens":20,"cache_read_input_tokens":50}}),
        }.to_string();
        let (url, worker) = server(protocol,200,body,false,None);
        let pricing = ModelPricing { currency:"CNY".into(), tiers: vec![
            PriceTier { min_input_tokens:0,input:1.0,output:2.0,cache_read:None,cache_write:None },
            PriceTier { min_input_tokens:200,input:2.0,output:4.0,cache_read:Some(0.5),cache_write:Some(3.0) },
        ] };
        let mut c = connection(url,protocol);
        c.settings = ModelSettings {
            context_window:Some(32768),max_output:Some(8192),supports_streaming:false,
            native_web_search:protocol != Protocol::OpenAiChat,
            reasoning:Some(ReasoningConfig { dialect:if protocol == Protocol::AnthropicMessages { ReasoningDialect::AnthropicBudget } else { ReasoningDialect::OpenAi }, efforts:vec![ReasoningEffort::High],budget_tokens:if protocol == Protocol::AnthropicMessages { Some(2048) } else { None } }),
            pricing:Some(pricing.clone()),..Default::default()
        };
        let mut deltas = Vec::new();
        let run = stream_chat_with_usage(c,history(),CancellationToken::new(),|text| deltas.push(text)).await.unwrap();
        worker.join().unwrap();
        assert_eq!(run.text,"完成");
        assert_eq!(deltas,["完成"]);
        assert_eq!(run.rounds.len(),1);
        let round = &run.rounds[0];
        assert_eq!(round.usage,Some(TokenUsage {input_tokens:200,output_tokens:20,cache_read_tokens:50,cache_write_tokens:0}));
        assert_eq!(round.pricing,Some(pricing));
        assert!((round.cost.unwrap()-0.000405).abs()<1e-12);
        assert!(run.tool_calls.is_empty());
    }
}
