# Rust / LumaKit 日志、请求、响应与异常封装概要设计

状态：概要设计，供后续大模型按契约实施。

适用范围：`LumaKit`、LumaDesk、ThinkRail Native 和其他 Rust 桌面宿主。不包含 MES .NET；MES 使用独立文档。

## 1. 目标

建立一套可复用的 Rust 边界封装：

```text
UI action / command / host tool / AI turn
  → RequestContext + TraceContext
  → service / use case
  → repository / database / file / git
  → external HTTP / Agent process
  → typed error
  → product boundary response
```

必须满足：

- 一次顶层操作生成一个稳定 `trace_id`；
- 所有子操作保留同一个 `trace_id`，通过 child span 表达父子关系；
- 方法入口、出口、耗时、错误、取消和超时可按 TraceId 检索；
- 请求上下文不混入业务 DTO；
- 业务层返回 typed `Result`，不返回 UI/HTTP/MCP response；
- 边界层统一把错误转换成用户或协议响应；
- 日志默认 `Info`，诊断时可切到 `Debug/Trace`；
- API key、token、password、完整 prompt、文件正文不进入日志。

## 2. 模块边界

### 2.1 `lumakit-observability`

位置：

```text
LumaKit/crates/lumakit-observability
```

只提供通用能力：

- `LogLevel`；
- `LogHandle`；
- `TraceContext`；
- `OperationGuard`；
- redaction/bounded-summary helpers；
- 文件写入、轮转、tail；
- tracing 字段约定。

不得依赖：UI、AI、数据库、LumaDesk、ThinkRail Native。

### 2.2 产品宿主

LumaDesk、ThinkRail Native 各自负责：

- AppPrefs / 配置持久化；
- 语言和用户文案；
- HTTP/MCP/GUI 边界响应；
- 领域错误；
- 日志设置页和日志查看页；
- 产品业务字段。

## 3. 日志级别

```rust
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}
```

语义：

```text
Error：系统错误、未处理异常、外部进程失败
Warn ：取消、超时、业务拒绝、可恢复异常
Info ：启动、正常完成、关键生命周期
Debug：方法入口、出口、耗时、关键摘要
Trace：协议帧、底层通信和细节
```

默认 `Info`。选择的级别包含该级别及更严重日志。

`LogHandle` 必须支持：

```text
init
set_level
status
 tail
```

产品负责将级别保存到自己的配置，并在下次启动时恢复。环境变量可覆盖保存值，适用于启动故障和临时诊断。环境覆盖不删除数据库里的用户设置。

## 4. TraceContext

建议 API：

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceContext {
    pub trace_id: String,
}

impl TraceContext {
    pub fn root(operation: &'static str) -> (Self, tracing::Span);
    pub fn child(&self, operation: &'static str) -> tracing::Span;
}
```

契约：

- `trace_id` 使用稳定随机值，32 个小写十六进制字符；
- 不使用全局 mutable current trace；
- Context 可 clone，调用方显式传给 worker；
- `chat_id`、`conversation_id`、`request_id` 不替代 TraceId；
- 不同请求绝不共享 TraceId；
- span 通过 `tracing` 记录 `span_id/parent_span_id` 或等价关系。

## 5. OperationGuard

跨边界操作必须使用统一生命周期：

```rust
let op = lumakit_observability::operation(&trace, "ai.http.request", "external");
let result = perform_request().await;
match result {
    Ok(_) => op.finish("ok"),
    Err(error) => op.fail(error.code()),
}
```

公共事件：

```text
ENTRY
EXIT + duration_ms
ERROR + duration_ms + error_code
CANCEL + duration_ms + reason
TIMEOUT + duration_ms + timeout_ms
```

`Drop` 时若没有终态，记录 `event=unfinished`。不要只依赖 span 自动关闭来表示业务成功或失败。

## 6. 统一日志字段

默认文本格式保持 grep-friendly：

```text
TraceId:<id>|Layer:Service|ME:Auth.Login|EV:ENTRY
TraceId:<id>|Layer:Repository|ME:User.find|EV:EXIT|RT:8|Result:SUCCESS
TraceId:<id>|Layer:Repository|ME:User.find|EV:ERROR|RT:9|Error:DATABASE_TIMEOUT
```

字段契约：

```text
TraceId
SpanId
ParentSpanId
Layer
Component
ME
EV
RT
Result
Error
```

内部必须保留结构化 tracing fields，未来可以增加 JSON/OTLP sink，不改变业务字段语义。

产品可以追加：

```text
workspace_id
chat_id
conversation_id
turn_id
attempt_id
backend
provider
model
item_id
```

## 7. 请求封装

请求参数使用明确 DTO：

```rust
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: SecretString,
}
```

请求上下文单独传递：

```rust
pub struct RequestContext {
    pub trace: TraceContext,
    pub request_id: String,
    pub actor: Option<ActorId>,
}

service.login(request, context).await
```

约束：

- DTO 不混入 TraceId、SpanId、数据库连接或 UI 状态；
- 请求边界负责反序列化、校验、大小限制和敏感字段隔离；
- Service 不读取隐式全局请求状态；
- 异步任务创建前 clone `RequestContext.trace`；
- 请求 id 用于一次具体外部请求，TraceId 用于完整业务链路，两者不能混用。

## 8. Typed Error

业务层使用：

```rust
pub type AppResult<T> = Result<T, AppError>;
```

建议基础错误分类：

```rust
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("invalid request: {message}")]
    InvalidRequest { code: &'static str, message: String },
    #[error("unauthorized")]
    Unauthorized { code: &'static str, message: String },
    #[error("not found: {resource}")]
    NotFound { code: &'static str, resource: &'static str },
    #[error("conflict: {message}")]
    Conflict { code: &'static str, message: String },
    #[error("database error")]
    Database { source: anyhow::Error },
    #[error("external service error")]
    External { code: &'static str, source: anyhow::Error },
}
```

错误码稳定、可搜索、不可依赖 message：

```text
AUTH_INVALID_CREDENTIALS
MODEL_UNAVAILABLE
ENGINE_NOT_INSTALLED
DATABASE_TIMEOUT
WORKSPACE_NOT_FOUND
REVIEW_FAILED
```

业务层不得直接返回 HTTP/MCP/GPUI response，也不得用字符串判断错误类型。

## 9. 边界响应

HTTP、MCP、GUI、CLI 各自转换 `AppError`，统一保留：

```rust
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
    pub trace_id: String,
}
```

边界转换原则：

```text
AppResult<T>::Ok(value) → success response
AppResult<T>::Err(error) → safe error response + trace_id
```

用户响应不得包含：

- 内部堆栈；
- SQL；
- API key；
- 文件路径中的敏感信息；
- provider 原始密钥错误；
- 完整 prompt 或文件正文。

日志保留脱敏异常摘要、error code、TraceId 和 duration。

## 10. 分层责任

### UI / command

- 创建 root TraceContext；
- 调用 service；
- 转换结果为 UI 状态；
- 把错误转换为用户提示。

### Service / use case

- 执行业务流程；
- 建立事务和子操作边界；
- 组合多个 repository/external call；
- 记录 service entry/exit/error。

### Repository / storage

- 访问 SQLite、文件、Git 或缓存；
- 转换底层错误为 typed error；
- 记录操作耗时和稳定 error code；
- 不创建 UI 文案、不返回 API response。

### External

- 记录目标类别、状态码/退出码、耗时、取消、超时；
- 只记录白名单 request id；
- 脱敏 URL/header/body。

## 11. 线程、异步和 channel 传播

必须显式传播 TraceContext 的边界：

```text
std::thread::spawn
background executor
async task
async_channel
worker queue
engine reader
callback
ReviewEvent
```

示例：

```rust
let worker_trace = trace.clone();
std::thread::spawn(move || {
    let _span = worker_trace.child("repository.query").entered();
    // 日志保留原 trace_id
});
```

禁止用线程本地变量或日志文本推断父级关系。

## 12. 脱敏

公共 API：

```rust
pub fn redact_secret(value: &str) -> String;
pub fn redact_url(url: &str) -> String;
pub fn bounded_text(value: &str, max_chars: usize) -> String;
pub fn diagnostic_hash(value: &str) -> String;
```

禁止日志记录：

- API key、token、password、Cookie、Authorization；
- 完整 prompt；
- 文件正文和附件；
- Base64；
- 未限制长度的 response body；
- 带敏感 query 的 URL。

允许记录摘要：

```text
status_code
request_id
body_bytes
prompt_chars
file_path
file_size
content_hash
```

## 13. 迁移顺序

### Phase 1：LumaKit

- 增加 `LogLevel::Trace`；
- 增加 `TraceContext`；
- 增加 `OperationGuard`；
- 增加脱敏 helper；
- 增加 root/child/thread/async/cancel/failure 测试；
- 保持 `LogHandle` 现有消费者兼容。

### Phase 2：LumaDesk

- AI request root trace；
- HTTP/provider child span；
- stream、usage、MCP、search、tool 传播；
- cancellation/timeout/failure error code；
- 设置页复用 `LogHandle::set_level`；
- 日志页支持 TraceId 过滤。

### Phase 3：ThinkRail Native

- 复用 `lumakit-observability`；
- 迁移 `state::run`；
- 迁移 HostTool/MCP/OMP worker；
- 迁移 Agent process/session；
- 迁移 ReviewRunner；
- 迁移 Router/Workspace/PlanReview/SettingsStore；
- 增加 Native trace 搜索。

## 14. 验收标准

LumaKit：

- 五级过滤正确；
- runtime reload 正确；
- root/child TraceContext 正确；
- thread/async/channel 不丢 TraceId；
- operation success/error/cancel/timeout/unfinished 均有终态；
- 脱敏测试通过；
- 不依赖 UI/AI/产品 crate。

消费者：

- 一次请求可以按 TraceId 找到完整链路；
- 方法入口、出口、耗时和错误码可检索；
- 并发请求不串 TraceId；
- 取消/超时不误报为系统 ERROR；
- 用户响应包含安全错误码和 TraceId；
- 日志不含密钥、完整 prompt 或敏感正文；
- LumaDesk 和 ThinkRail Native 各自通过真实操作验证。
