//! Browser-free AI contracts and the smallest real streaming runtime.
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin, sync::Arc};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

pub mod chat;
pub mod catalog;
pub mod history;
pub mod images;
pub mod mcp;
pub mod mcp_server;
mod protocol;
pub mod providers;
pub mod search;
pub mod search_adapters;
pub mod skills;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Protocol {
    AnthropicMessages,
    OpenAiChat,
    OpenAiResponses,
}

#[derive(Clone)]
pub struct AiConnection {
    pub base_url: String,
    pub protocol: Protocol,
    pub api_key: Option<Zeroizing<String>>,
    pub model: String,
}
impl std::fmt::Debug for AiConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiConnection")
            .field("base_url", &self.base_url)
            .field("protocol", &self.protocol)
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .field("model", &self.model)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub protocol: Protocol,
}
/// 纯文本聊天消息；历史必须按用户、助手交替排列，并以用户消息结束。
#[derive(Clone)]
pub enum ChatMessage {
    /// 用户输入。
    User(String),
    /// 已完成的助手回复。
    Assistant(String),
}

/// 流式发送纯文本会话（正文合计最多 1 MiB，单次请求最多 120 秒）。
///
/// 返回完整回复；取消、上游错误、未完成的流或意外工具调用均返回错误。
pub async fn stream_chat<F>(
    connection: AiConnection,
    messages: Vec<ChatMessage>,
    cancel: CancellationToken,
    mut on_text: F,
) -> Result<String, AiError>
where
    F: FnMut(String) + Send,
{
    if messages.is_empty() || messages.len() % 2 == 0 {
        return Err(AiError::Provider(
            "chat must start and end with a user message".into(),
        ));
    }
    let mut bytes = 0usize;
    for (index, message) in messages.iter().enumerate() {
        let text = match message {
            ChatMessage::User(text) if index % 2 == 0 => text,
            ChatMessage::Assistant(text) if index % 2 == 1 => text,
            _ => {
                return Err(AiError::Provider(
                    "chat roles must alternate user and assistant".into(),
                ));
            }
        };
        bytes = bytes.saturating_add(text.len());
        if text.trim().is_empty() || bytes > 1024 * 1024 {
            return Err(AiError::Provider(
                "chat messages must be nonempty and total at most 1 MiB".into(),
            ));
        }
    }
    if connection.model.trim().is_empty() {
        return Err(AiError::ModelNotConfigured);
    }
    if cancel.is_cancelled() {
        return Err(AiError::Cancelled);
    }
    let messages: Vec<_> = messages
        .into_iter()
        .map(|message| match message {
            ChatMessage::User(text) => Message::User(text),
            ChatMessage::Assistant(text) => Message::Assistant {
                text,
                tool_calls: Vec::new(),
            },
        })
        .collect();
    let result = round(
        &http_client()?,
        &connection,
        None,
        &messages,
        &[],
        &cancel,
        &mut on_text,
    )
    .await?;
    if !result.tool_calls.is_empty() {
        return Err(AiError::Provider("unexpected tool calls in text chat".into()));
    }
    Ok(result.text)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRequest {
    pub input: String,
    pub model: Option<ModelRef>,
    pub purpose: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub read_only: bool,
    pub requires_approval: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}
#[derive(Debug, Clone)]
pub struct AiRun {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Allow,
    Deny,
    Ask,
}
#[derive(Debug, Error)]
pub enum AiError {
    #[error("model is not configured")]
    ModelNotConfigured,
    #[error("tool is not allowed")]
    ToolDenied,
    #[error("AI provider error: {0}")]
    Provider(String),
    #[error("request cancelled")]
    Cancelled,
    #[error("invalid endpoint: {0}")]
    InvalidEndpoint(String),
}
pub type ApprovalFuture = Pin<Box<dyn Future<Output = Approval> + Send>>;
pub type ToolFuture = Pin<Box<dyn Future<Output = Result<ToolOutput, AiError>> + Send>>;

pub trait AiHost: Send + Sync {
    fn select_model(&self, request: &RunRequest) -> Result<ModelRef, AiError>;
    fn system_prompt(&self, request: &RunRequest) -> Result<Option<String>, AiError>;
    fn tools(&self, request: &RunRequest) -> Vec<ToolSpec>;
    fn authorize(&self, call: &ToolCall, default: Approval) -> Approval;
    fn approve(&self, _: ToolCall, _: CancellationToken) -> ApprovalFuture {
        Box::pin(async { Approval::Deny })
    }
    fn call_tool(&self, call: ToolCall, cancel: CancellationToken) -> ToolFuture;
    fn connection(&self, model: &ModelRef) -> Result<AiConnection, AiError>;
}

pub struct AiRuntime<H: AiHost> {
    pub host: Arc<H>,
    http: reqwest::Client,
}
impl<H: AiHost> AiRuntime<H> {
    pub fn new(host: Arc<H>) -> Result<Self, AiError> {
        let http = http_client()?;
        Ok(Self { host, http })
    }
    pub fn available_tools(&self, request: &RunRequest) -> Vec<ToolSpec> {
        self.host.tools(request)
    }
    pub async fn run<F>(
        &self,
        request: RunRequest,
        cancel: CancellationToken,
        mut on_text: F,
    ) -> Result<AiRun, AiError>
    where
        F: FnMut(String) + Send,
    {
        let model = self.host.select_model(&request)?;
        let connection = self.host.connection(&model)?;
        let system = self.host.system_prompt(&request)?;
        let tools = self.host.tools(&request);
        let mut messages = vec![Message::User(request.input)];
        let mut text = String::new();
        for _ in 0..5 {
            let round = round(
                &self.http,
                &connection,
                system.as_deref(),
                &messages,
                &tools,
                &cancel,
                &mut on_text,
            )
            .await?;
            text.push_str(&round.text);
            if round.tool_calls.is_empty() {
                return Ok(AiRun {
                    text,
                    tool_calls: Vec::new(),
                });
            }
            messages.push(Message::Assistant {
                text: round.text,
                tool_calls: round.tool_calls.clone(),
            });
            for call in round.tool_calls {
                if cancel.is_cancelled() {
                    return Err(AiError::Cancelled);
                }
                let Some(spec) = tools.iter().find(|tool| tool.name == call.name) else {
                    messages.push(Message::Tool {
                        call_id: call.id,
                        text: "error unknown_tool".into(),
                    });
                    continue;
                };
                let approval = self.host.authorize(
                    &call,
                    if spec.requires_approval {
                        Approval::Ask
                    } else {
                        Approval::Allow
                    },
                );
                let approval = if approval == Approval::Ask {
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(AiError::Cancelled),
                        approval = self.host.approve(call.clone(), cancel.clone()) => approval,
                    }
                } else {
                    approval
                };
                let output = match approval {
                    Approval::Deny | Approval::Ask => ToolOutput {
                        text: "该工具调用已被拒绝，未执行。".into(),
                        is_error: true,
                    },
                    Approval::Allow => self.host.call_tool(call.clone(), cancel.clone()).await?,
                };
                messages.push(Message::Tool {
                    call_id: call.id,
                    text: output.text,
                });
            }
        }
        Ok(AiRun {
            text,
            tool_calls: Vec::new(),
        })
    }
}

async fn round<F>(
    http: &reqwest::Client,
    connection: &AiConnection,
    system: Option<&str>,
    messages: &[Message],
    tools: &[ToolSpec],
    cancel: &CancellationToken,
    on_text: &mut F,
) -> Result<AiRun, AiError>
where
    F: FnMut(String) + Send,
{
    let url = endpoint(connection)?;
    let mut builder = http
        .post(url)
        .timeout(std::time::Duration::from_secs(120))
        .header(reqwest::header::ACCEPT, "text/event-stream");
    if connection.protocol == Protocol::AnthropicMessages {
        builder = builder.header("anthropic-version", "2023-06-01");
    }
    if let Some(key) = &connection.api_key {
        builder = builder.bearer_auth(key.as_str());
        if connection.protocol == Protocol::AnthropicMessages {
            builder = builder.header("x-api-key", key.as_str());
        }
    }
    let response = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(AiError::Cancelled),
        result = builder.json(&protocol::encode(connection, system, messages, tools)).send() => result.map_err(network)?,
    };
    if !response.status().is_success() {
        return Err(provider_response(response, connection.api_key.as_ref(), cancel).await);
    }
    let mut parser = protocol::SseParser::default();
    let mut decoder = protocol::Decoder::default();
    let mut stream = futures_util::StreamExt::boxed(response.bytes_stream());
    let mut text = String::new();
    // ponytail: 整条 SSE 限 4 MiB；需要更长输出时再做逐事件缓冲上限。
    let mut stream_bytes = 0usize;
    while let Some(chunk) = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(AiError::Cancelled),
        chunk = futures_util::StreamExt::next(&mut stream) => chunk,
    } {
        let chunk = chunk.map_err(network)?;
        stream_bytes = stream_bytes.saturating_add(chunk.len());
        if stream_bytes > 4 * 1024 * 1024 {
            return Err(AiError::Provider("stream exceeds 4 MiB".into()));
        }
        for data in parser.feed(&chunk) {
            if let Some(delta) = decoder
                .event(connection.protocol, &data)
                .map_err(|error| provider_error(error, connection.api_key.as_ref()))?
            {
                text.push_str(&delta);
                on_text(delta);
            }
        }
    }
    for data in parser.finish() {
        if let Some(delta) = decoder
            .event(connection.protocol, &data)
            .map_err(|error| provider_error(error, connection.api_key.as_ref()))?
        {
            text.push_str(&delta);
            on_text(delta);
        }
    }
    if !decoder.done {
        return Err(AiError::Provider("stream ended before completion".into()));
    }
    let tool_calls = decoder.finish().map_err(AiError::Provider)?;
    Ok(AiRun { text, tool_calls })
}

#[derive(Debug, Clone)]
enum Message {
    User(String),
    Assistant {
        text: String,
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        call_id: String,
        text: String,
    },
}
fn endpoint(connection: &AiConnection) -> Result<String, AiError> {
    let base = connection.base_url.trim_end_matches('/');
    let parsed =
        reqwest::Url::parse(base).map_err(|error| AiError::InvalidEndpoint(error.to_string()))?;
    let loopback = matches!(
        parsed.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && loopback) {
        return Err(AiError::InvalidEndpoint(
            "HTTPS required; HTTP is only allowed for loopback".into(),
        ));
    }
    Ok(format!("{base}/{}", protocol::path(connection.protocol)))
}
fn http_client() -> Result<reqwest::Client, AiError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(network)
}

fn network(error: reqwest::Error) -> AiError {
    // 不显示 reqwest 的错误链：其中可能包含带凭据的 URL 或请求头。
    AiError::Provider(if error.is_timeout() {
        "network request timed out".into()
    } else {
        "network request failed".into()
    })
}

fn provider_error(body: String, secret: Option<&Zeroizing<String>>) -> AiError {
    let body = match secret.filter(|secret| !secret.is_empty()) {
        Some(secret) => body.replace(secret.as_str(), "***"),
        None => body,
    };
    AiError::Provider(body)
}

async fn provider_response(
    mut response: reqwest::Response,
    secret: Option<&Zeroizing<String>>,
    cancel: &CancellationToken,
) -> AiError {
    let status = response.status();
    let read = async {
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(network)? {
            if chunk.len() > 8192 - body.len() {
                // 丢弃超限正文，避免截断后留下半个密钥。
                return Ok::<_, AiError>(format!("provider returned {status}: error body exceeds 8192 bytes"));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(format!("provider returned {status}: {}", String::from_utf8_lossy(&body)))
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => AiError::Cancelled,
        result = tokio::time::timeout(std::time::Duration::from_secs(10), read) => match result {
            Ok(Ok(body)) => provider_error(body, secret),
            Ok(Err(error)) => error,
            Err(_) => AiError::Provider(format!("provider returned {status}: error body read timed out")),
        },
    }
}
