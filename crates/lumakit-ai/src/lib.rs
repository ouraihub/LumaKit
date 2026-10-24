//! Browser-free AI contracts and the smallest real streaming runtime.
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin, sync::Arc};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

/// 交互式聊天界面及其会话状态。
pub mod chat;
/// 从已保存的服务商快照解析模型连接。
pub mod catalog;
/// SQLite 服务商目录持久化。
pub mod catalog_store;
/// 模型元数据、请求限制、推理方言与计费。
pub mod model_config;
pub use model_config::{RoundUsage, TokenUsage};
/// 聊天历史记录与持久化存储。
pub mod history;
/// 内嵌图片的大小、数量和编码校验。
pub mod images;
/// 受信任 MCP 服务的客户端调用。
pub mod mcp;
/// 带作用域和令牌授权的 MCP 服务端处理。
pub mod mcp_server;
mod protocol;
/// 服务商配置、模型能力与密钥存储。
pub mod providers;
/// 搜索结果策略及外部搜索接口。
pub mod search;
/// Tavily、Brave 和 Exa 的 HTTP 搜索适配器。
pub mod search_adapters;
/// 技能文档解析、确认与本地存储。
pub mod skills;

/// 模型请求与流式响应使用的协议。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Protocol {
    /// Anthropic Messages 协议。
    AnthropicMessages,
    /// OpenAI Chat Completions 协议。
    OpenAiChat,
    /// OpenAI Responses 协议。
    OpenAiResponses,
}

/// 一次模型请求所需的连接信息；调试输出隐藏密钥。
#[derive(Clone)]
pub struct AiConnection {
    /// 服务基础地址，由请求阶段解析并检查。
    pub base_url: String,
    /// 请求编码及响应解码协议。
    pub protocol: Protocol,
    /// 可选认证密钥，释放时清零。
    pub api_key: Option<Zeroizing<String>>,
    /// 发给服务商的模型标识。
    pub model: String,
    /// 已保存的请求控制；在实际请求边界再次校验。
    pub settings: model_config::ModelSettings,
}
impl AiConnection {
    /// 为此连接的请求选择已声明的推理强度，不静默替换为其他值。
    ///
    /// # Errors
    /// 未配置推理能力、所选强度不在允许列表或模型设置无效时返回错误说明。
    pub fn with_reasoning_effort(mut self, effort: model_config::ReasoningEffort) -> Result<Self, String> {
        let config = self.settings.reasoning.as_mut().ok_or("模型未配置推理能力")?;
        let index = config.efforts.iter().position(|e| *e == effort).ok_or("模型未声明该推理强度")?;
        config.efforts.swap(0, index);
        self.settings.validate(self.protocol)?;
        Ok(self)
    }
}
impl std::fmt::Debug for AiConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiConnection")
            .field("base_url", &self.base_url)
            .field("protocol", &self.protocol)
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .field("model", &self.model)
            .field("settings", &self.settings)
            .finish()
    }
}

/// 指向服务商、线路及模型的已保存选择。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRef {
    /// 服务商标识。
    pub provider: String,
    /// 服务商内的线路标识。
    pub endpoint: String,
    /// 服务商接受的模型标识。
    pub model: String,
    /// 选择模型时使用的线路协议。
    pub protocol: Protocol,
}
/// 聊天消息；用户消息可携带已验证的内嵌图片。
#[derive(Clone)]
pub enum ChatMessage {
    /// 纯文本用户输入。
    User(String),
    /// 视觉用户输入；图片数据不接受文件路径。
    UserImages {
        /// 用户文字提示。
        text: String,
        /// 已验证的内嵌图片。
        images: Vec<images::ImagePart>,
    },
    /// 已完成的助手回复。
    Assistant(String),
}

/// 流式发送纯文本会话（正文合计最多 1 MiB，单次请求最多 120 秒）。
///
/// 返回完整回复和逐请求usage/价格快照；取消、上游错误、未完成的流或意外工具调用均返回错误。
pub async fn stream_chat_with_usage<F>(
    connection: AiConnection,
    messages: Vec<ChatMessage>,
    cancel: CancellationToken,
    mut on_text: F,
) -> Result<AiRun, AiError>
where
    F: FnMut(String) + Send,
{
    if messages.is_empty() || messages.len() % 2 == 0 { return Err(AiError::Provider("chat must start and end with a user message".into())); }
    let mut bytes = 0usize;
    for (index, message) in messages.iter().enumerate() {
        let (text, images) = match message {
            ChatMessage::User(text) if index % 2 == 0 => (text.as_str(), None),
            ChatMessage::UserImages { text, images } if index % 2 == 0 => {
                if !connection.settings.supports_vision { return Err(AiError::Provider("所选模型不支持视觉输入".into())); }
                images::validate(images).map_err(|error| AiError::Provider(error.to_string()))?;
                (text.as_str(), Some(images.as_slice()))
            }
            ChatMessage::Assistant(text) if index % 2 == 1 => (text.as_str(), None),
            _ => return Err(AiError::Provider("chat roles must alternate user and assistant".into())),
        };
        bytes = bytes.saturating_add(text.len());
        if (text.trim().is_empty() && images.is_none_or(<[_]>::is_empty)) || bytes > 1024 * 1024 { return Err(AiError::Provider("chat text must be nonempty and total at most 1 MiB".into())); }
    }
    if connection.model.trim().is_empty() { return Err(AiError::ModelNotConfigured); }
    if cancel.is_cancelled() { return Err(AiError::Cancelled); }
    let messages: Vec<_> = messages.into_iter().map(|message| match message {
        ChatMessage::User(text) => Message::User { text, images: Vec::new() },
        ChatMessage::UserImages { text, images } => Message::User { text, images },
        ChatMessage::Assistant(text) => Message::Assistant { text, tool_calls: Vec::new(), reasoning_blocks: Vec::new() },
    }).collect();
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
    Ok(result)
}

/// 发送文本会话；不需要持久化用量时只返回正文。
///
/// # Errors
/// 输入、模型设置或协议无效、取消、上游报错、响应截断或返回意外工具调用时返回错误。
pub async fn stream_chat<F>(connection: AiConnection, messages: Vec<ChatMessage>, cancel: CancellationToken, on_text: F) -> Result<String, AiError>
where F: FnMut(String) + Send {
    Ok(stream_chat_with_usage(connection, messages, cancel, on_text).await?.text)
}

/// 交由宿主选择模型、提示词和工具的运行请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRequest {
    /// 首条用户消息正文。
    pub input: String,
    /// 可选模型选择，具体解析由宿主负责。
    pub model: Option<ModelRef>,
    /// 供宿主决定模型、提示词或工具的用途标识。
    pub purpose: String,
}
/// 向模型声明的工具及其授权策略。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    /// 模型调用工具时使用的名称。
    pub name: String,
    /// 供模型理解工具用途的说明。
    pub description: String,
    /// 工具输入的 JSON Schema。
    pub parameters: serde_json::Value,
    /// 工具是否声明为只读；运行时不据此强制授权。
    pub read_only: bool,
    /// 是否默认请求宿主审批，而非默认允许。
    pub requires_approval: bool,
}
/// 模型生成的工具调用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// 用于关联工具结果的调用标识。
    pub id: String,
    /// 待调用工具名称。
    pub name: String,
    /// 解码后的 JSON 参数。
    pub arguments: serde_json::Value,
}
/// 宿主工具执行结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// 回传模型的文本；运行时不单独编码错误标记。
    pub text: String,
    /// 宿主报告的失败标记。
    pub is_error: bool,
}
/// 模型运行返回的文本和工具调用。
#[derive(Debug, Clone)]
pub struct AiRun {
    /// 已收集的回复文本。
    pub text: String,
    /// 本轮工具调用；完整运行结束时此列表为空。
    pub tool_calls: Vec<ToolCall>,
    /// 每个已完成上游请求的用量及价格快照。
    pub rounds: Vec<RoundUsage>,
    /// 工具续轮需要原样回传的 Anthropic 签名思考块。
    pub reasoning_blocks: Vec<serde_json::Value>,
}
/// 工具调用、审批和结果的可持久化事件。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolEvent {
    /// 工具调用标识。
    pub id: String,
    /// 工具名称。
    pub name: String,
    /// 模型传入的 JSON 参数。
    pub arguments: serde_json::Value,
    /// 最终授权决定；工具尚未完成时可能为空。
    pub approval: Option<Approval>,
    /// 工具输出；拒绝或执行失败时也会保留可展示文本。
    pub result: Option<String>,
    /// 工具是否报告业务错误。
    pub is_error: bool,
    /// 未知工具或执行失败的错误说明。
    pub error: Option<String>,
}

/// 工具调用的授权决定。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Approval {
    /// 允许执行。
    Allow,
    /// 拒绝执行。
    Deny,
    /// 请求审批；审批后仍返回此值则不执行。
    Ask,
}
/// 模型请求、连接或工具执行失败。
#[derive(Debug, Error)]
pub enum AiError {
    /// 尚未配置可用模型。
    #[error("model is not configured")]
    ModelNotConfigured,
    /// 工具调用未获授权。
    #[error("tool is not allowed")]
    ToolDenied,
    /// 服务商、网络或响应解析错误说明。
    #[error("AI provider error: {0}")]
    Provider(String),
    /// 请求已取消。
    #[error("request cancelled")]
    Cancelled,
    /// 连接地址解析或安全策略校验失败。
    #[error("invalid endpoint: {0}")]
    InvalidEndpoint(String),
}
/// 可跨线程发送的异步工具审批结果。
pub type ApprovalFuture = Pin<Box<dyn Future<Output = Approval> + Send>>;
/// 可跨线程发送的异步工具执行结果。
pub type ToolFuture = Pin<Box<dyn Future<Output = Result<ToolOutput, AiError>> + Send>>;

/// 为运行时提供模型配置、工具实现和授权决策的宿主。
pub trait AiHost: Send + Sync {
    /// 根据请求选择模型，无法选择时返回错误。
    fn select_model(&self, request: &RunRequest) -> Result<ModelRef, AiError>;
    /// 返回可选系统提示词或构造提示词的错误。
    fn system_prompt(&self, request: &RunRequest) -> Result<Option<String>, AiError>;
    /// 返回本次请求向模型公开的工具。
    fn tools(&self, request: &RunRequest) -> Vec<ToolSpec>;
    /// 决定调用权限，可保留默认决定或要求审批。
    fn authorize(&self, call: &ToolCall, default: Approval) -> Approval;
    /// 异步审批工具调用；默认拒绝，运行时会响应取消信号。
    fn approve(&self, _: ToolCall, _: CancellationToken) -> ApprovalFuture {
        Box::pin(async { Approval::Deny })
    }
    /// 执行已获准的工具调用；宿主负责处理取消信号。
    fn call_tool(&self, call: ToolCall, cancel: CancellationToken) -> ToolFuture;
    /// 将所选模型解析为实际连接，失败时返回错误。
    fn connection(&self, model: &ModelRef) -> Result<AiConnection, AiError>;
}

/// 在宿主授权下运行最多五轮模型及工具交互的运行时。
pub struct AiRuntime<H: AiHost> {
    /// 提供模型配置和工具实现的共享宿主。
    pub host: Arc<H>,
    http: reqwest::Client,
}
impl<H: AiHost> AiRuntime<H> {
    /// 创建禁止自动重定向的 HTTP 客户端，创建失败时返回错误。
    pub fn new(host: Arc<H>) -> Result<Self, AiError> {
        let http = http_client()?;
        Ok(Self { host, http })
    }
    /// 返回宿主为该请求公开的工具，不执行调用或审批。
    pub fn available_tools(&self, request: &RunRequest) -> Vec<ToolSpec> {
        let Ok(model) = self.host.select_model(request) else { return Vec::new(); };
        let Ok(connection) = self.host.connection(&model) else { return Vec::new(); };
        if connection.settings.model_type != model_config::ModelType::Chat || connection.settings.supports_tools == Some(false) { Vec::new() } else { self.host.tools(request) }
    }
    /// 流式运行请求并依次处理工具，最多五轮后返回已收集文本。
    ///
    /// 文本增量交给回调；取消、上游或宿主错误立即返回错误。
    /// 未知或拒绝的工具会作为文本结果回传模型，而不执行。
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
        let tools = if connection.settings.supports_tools == Some(false) { Vec::new() } else { self.host.tools(&request) };
        let mut messages = vec![Message::User { text: request.input, images: Vec::new() }];
        let mut text = String::new();
        let mut rounds = Vec::new();
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
            rounds.extend(round.rounds);
            if round.tool_calls.is_empty() {
                return Ok(AiRun {
                    text,
                    tool_calls: Vec::new(),
                    rounds,
                    reasoning_blocks: Vec::new(),
                });
            }
            messages.push(Message::Assistant {
                text: round.text,
                tool_calls: round.tool_calls.clone(),
                reasoning_blocks: round.reasoning_blocks,
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
            rounds,
            reasoning_blocks: Vec::new(),
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
    let input_bytes = messages.iter().try_fold(system.map_or(0, str::len), |total, message| {
        let text = match message { Message::User { text, .. } | Message::Assistant {text,..} | Message::Tool {text,..} => text };
        total.checked_add(text.len())
    });
    if input_bytes.is_none_or(|n| n > 1024 * 1024) { return Err(AiError::Provider("chat messages must total at most 1 MiB".into())); }
    connection.settings.validate(connection.protocol).map_err(AiError::Provider)?;
    if connection.settings.model_type != model_config::ModelType::Chat {
        return Err(AiError::Provider("所选模型不是聊天类型；图片、语音、向量和重排需各自专用接口".into()));
    }
    if connection.settings.supports_tools == Some(false) && !tools.is_empty() {
        return Err(AiError::Provider("所选模型声明不支持工具".into()));
    }
    check_context(connection, system, messages, tools)?;
    let url = endpoint(connection)?;
    let mut builder = http
        .post(url)
        .timeout(std::time::Duration::from_secs(120))
        .header(reqwest::header::ACCEPT, if connection.settings.supports_streaming { "text/event-stream" } else { "application/json" });
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
    if !connection.settings.supports_streaming {
        let read = async {
            let mut response = response;
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(network)? {
                if chunk.len() > 4 * 1024 * 1024 - body.len() { return Err(AiError::Provider("response exceeds 4 MiB".into())); }
                body.extend_from_slice(&chunk);
            }
            let value = serde_json::from_slice(&body).map_err(|_| AiError::Provider("invalid response JSON".into()))?;
            let mut decoder = protocol::Decoder::default();
            let text = decoder.complete(connection.protocol, &value).map_err(|e| provider_error(e, connection.api_key.as_ref()))?;
            let usage = decoder.usage.clone();
            let pricing = connection.settings.pricing.clone();
            let cost = pricing.as_ref().zip(usage.as_ref()).and_then(|(p,u)| p.cost(u));
            let reasoning_blocks = std::mem::take(&mut decoder.reasoning_blocks);
            let tool_calls = decoder.finish().map_err(AiError::Provider)?;
            Ok(AiRun { text, tool_calls, reasoning_blocks, rounds: vec![RoundUsage { usage, pricing, cost }] })
        };
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AiError::Cancelled),
            result = tokio::time::timeout(std::time::Duration::from_secs(120), read) => result.map_err(|_| AiError::Provider("response read timed out".into()))??,
        };
        if !result.text.is_empty() { on_text(result.text.clone()); }
        return Ok(result);
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
    let usage = decoder.usage.clone();
    let pricing = connection.settings.pricing.clone();
    let cost = pricing.as_ref().zip(usage.as_ref()).and_then(|(p,u)| p.cost(u));
    let reasoning_blocks = std::mem::take(&mut decoder.reasoning_blocks);
    let tool_calls = decoder.finish().map_err(AiError::Provider)?;
    Ok(AiRun { text, tool_calls, reasoning_blocks, rounds: vec![RoundUsage { usage, pricing, cost }] })
}

#[derive(Debug, Clone)]
enum Message {
    User { text: String, images: Vec<images::ImagePart> },
    Assistant { text: String, tool_calls: Vec<ToolCall>, reasoning_blocks: Vec<serde_json::Value> },
    Tool { call_id: String, text: String },
}
// ponytail: conservative Unicode scalar estimate, not model tokenization; replace with a provider tokenizer if exact admission is needed.
fn check_context(connection: &AiConnection, system: Option<&str>, messages: &[Message], tools: &[ToolSpec]) -> Result<(), AiError> {
    let capacity = u64::from(connection.settings.context_window.unwrap_or(32000));
    let mut estimate = system.map_or(0u64, |s| s.chars().count() as u64 * 4 + 32);
    for message in messages {
        let (text, calls, images) = match message {
            Message::User { text, images } => (text, None, images.as_slice()),
            Message::Tool { text, .. } => (text, None, &[] as &[images::ImagePart]),
            Message::Assistant { text, tool_calls, reasoning_blocks } => {
                for block in reasoning_blocks { estimate = estimate.saturating_add(block.to_string().chars().count() as u64 * 4 + 32); }
                (text, Some(tool_calls), &[] as &[images::ImagePart])
            }
        };
        estimate = estimate.saturating_add(text.chars().count() as u64 * 4 + 32);
        // ponytail: 图片按每张4096 token保守预留；精确预算需接入各模型的分辨率计价规则。
        estimate = estimate.saturating_add(images.len() as u64 * 4096);
        if let Some(calls) = calls { for call in calls { estimate = estimate.saturating_add((call.name.chars().count() + call.id.chars().count() + call.arguments.to_string().chars().count()) as u64 * 4 + 32); } }
    }
    for tool in tools { estimate = estimate.saturating_add((tool.name.chars().count() + tool.description.chars().count() + tool.parameters.to_string().chars().count()) as u64 * 4 + 32); }
    if connection.settings.native_web_search { estimate = estimate.saturating_add(256); }
    if estimate.saturating_add(u64::from(connection.settings.output_limit())) > capacity { return Err(AiError::Provider("上下文预算不足：按每个Unicode字符最多4 token及消息开销保守估算（非精确token计数），并预留完整输出上限；请缩短历史或增加窗口".into())); }
    Ok(())
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

#[cfg(test)]
mod model_runtime_tests {
    use super::*;
    #[tokio::test]
    async fn context_and_type_gates_reject_before_network() {
        let mut c = AiConnection { base_url: "https://invalid.invalid".into(), protocol: Protocol::OpenAiChat, api_key: None, model: "model".into(), settings: model_config::ModelSettings { context_window: Some(50), max_output: Some(10), ..Default::default() } };
        assert!(check_context(&c,None,&[Message::User { text:"你好".into(), images:Vec::new() }],&[]).is_ok());
        assert!(check_context(&c,None,&[Message::User { text:"你好🙂".into(), images:Vec::new() }],&[]).is_err());
        c.settings.context_window = None;
        c.settings.model_type = model_config::ModelType::Image;
        let mut ignore = |_:String| {};
        let error = round(&http_client().unwrap(),&c,None,&[Message::User { text:"hello".into(), images:Vec::new() }],&[],&CancellationToken::new(),&mut ignore).await.unwrap_err();
        assert!(error.to_string().contains("不是聊天类型"));
        c.settings.model_type = model_config::ModelType::Chat;
        c.settings.supports_tools = Some(false);
        let tools = [ToolSpec { name:"x".into(),description:String::new(),parameters:serde_json::json!({}),read_only:true,requires_approval:false }];
        let error = round(&http_client().unwrap(),&c,None,&[Message::User { text:"hello".into(), images:Vec::new() }],&tools,&CancellationToken::new(),&mut ignore).await.unwrap_err();
        assert!(error.to_string().contains("不支持工具"));
        c.settings.pricing = Some(model_config::ModelPricing { currency:"USD".into(), tiers:vec![model_config::PriceTier { min_input_tokens:0,input:f64::NAN,output:0.0,cache_read:None,cache_write:None }] });
        assert!(c.settings.validate(c.protocol).is_err());
    }
}
