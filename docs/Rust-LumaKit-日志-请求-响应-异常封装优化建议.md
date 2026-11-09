 # Rust / LumaKit 日志、请求、响应与异常封装概要设计

状态：概要设计，作为后续大模型实施契约。适用于 `lumakit-observability`、LumaDesk、ThinkRail Native 和其他 Rust desktop host。

本文不是 MES .NET 的实现说明；MES 单独使用 `MES.Backend/docs/日志-请求-响应-异常封装优化建议.md`。
>
> 本文只讨论 Rust/LumaKit，不包含 MES .NET 的具体实现。

## 1. 设计目标

Rust 项目需要同时解决四件事：

```text
请求上下文
  → 业务操作
    → 外部资源/存储/网络/进程
      → typed error
        → 产品边界响应
```

日志必须能通过一个 `trace_id` 找到完整链路：

```text
UI action / command / host tool / AI turn
  → service / use case
  → repository / database / file / git
  → external HTTP / Agent process
  → completion / error / cleanup
```

目标不是把所有函数都打印日志，而是给真实边界提供：

- 方法入口；
- 方法出口；
- 耗时；
- 错误码；
- 取消/超时；
- TraceId；
- 父子 span 关系。

## 2. LumaKit 公共边界

公共实现放在：

```text
LumaKit/crates/lumakit-observability
```

它只依赖通用库，不依赖 UI、AI、数据库或产品。

### 2.1 公共能力

```text
LogLevel
LogHandle
TraceContext
OperationGuard
redaction helpers
file writer / rotation / tail
stable tracing field conventions
```

### 2.2 不放入 LumaKit

```text
AppPrefs
SQLite schema
ChatSession
ChatId / WorkspaceId
ReviewRunner
Git 归属
产品错误文案
GPUI/egui 日志设置页
```

产品只通过结构化字段追加自己的业务上下文。

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
ERROR：未处理异常、系统错误、外部进程失败
WARN ：取消、超时、业务拒绝、可恢复错误
INFO ：启动、正常完成、关键生命周期
DEBUG：方法入口、出口、耗时、关键摘要
TRACE：协议帧、底层通信和细节
```

默认 `Info`。

产品层负责：

- 将级别持久化到 AppPrefs 或产品配置；
- 启动后恢复；
- 设置页动态调用 `LogHandle::set_level`；
- 环境变量优先覆盖。

## 4. TraceContext

推荐公共 API：

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

要求：

- `trace_id` 使用稳定随机标识；
- context 可以 clone；
- 不使用全局可变 current trace；
- 跨 thread、async、channel、worker 显式传递；
- `chat_id`、`conversation_id`、`request_id` 不能替代 trace_id；
- 子操作继承 trace_id，使用子 span 表达调用关系。

## 5. OperationGuard

跨边界函数使用统一 operation 生命周期：

```rust
let operation = lumakit_observability::operation(
    &trace,
    "ai.http.request",
    "external",
);

let result = perform_request().await;

match result {
    Ok(value) => operation.finish("ok"),
    Err(error) => operation.fail(error.code()),
}
```

日志事件：

```text
ENTRY
EXIT + duration_ms
ERROR + duration_ms + error_code
CANCEL + duration_ms + reason
TIMEOUT + duration_ms + timeout_ms
```

`Drop` 时如果没有显式结束，应记录 `unfinished`，防止异步任务静默消失。

## 6. 请求封装

Rust 业务请求使用明确的输入结构：

```rust
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: SecretString,
}
```

不要把请求上下文塞入业务 DTO。上下文单独传递：

```rust
service.login(request, request_context).await
```

建议：

```rust
pub struct RequestContext {
    pub trace: TraceContext,
    pub request_id: String,
    pub actor: Option<ActorId>,
}
```

请求边界负责：

- 反序列化；
- 字段校验；
- 请求大小限制；
- 敏感字段隔离；
- 创建或继承 TraceContext。

## 7. 响应封装

业务层不返回 HTTP/MCP/UI response 类型。

业务层：

```rust
pub type AppResult<T> = Result<T, AppError>;
```

成功：

```rust
Ok(value)
```

失败：

```rust
Err(AppError::unauthorized(
    "AUTH_INVALID_CREDENTIALS",
    "invalid credentials",
))
```

外部边界再转换：

```rust
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
    pub trace_id: String,
}
```

HTTP、MCP、GUI、CLI 可以有不同响应格式，但都应保留：

```text
stable error code
user-safe message
trace_id
```

不要让 Service 或 Repository 直接依赖 GPUI、MCP 或 HTTP response 类型。

## 8. 错误封装

推荐用 `thiserror` 定义稳定错误分类：

```rust
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("invalid request: {message}")]
    InvalidRequest {
        code: &'static str,
        message: String,
    },

    #[error("unauthorized")]
    Unauthorized {
        code: &'static str,
        message: String,
    },

    #[error("not found: {resource}")]
    NotFound {
        code: &'static str,
        resource: &'static str,
    },

    #[error("conflict: {message}")]
    Conflict {
        code: &'static str,
        message: String,
    },

    #[error("database error")]
    Database {
        source: anyhow::Error,
    },

    #[error("external service error")]
    External {
        code: &'static str,
        source: anyhow::Error,
    },
}
```

每个领域可有自己的错误 enum，但必须提供稳定 code：

```text
AUTH_INVALID_CREDENTIALS
MODEL_UNAVAILABLE
ENGINE_NOT_INSTALLED
DATABASE_TIMEOUT
WORKSPACE_NOT_FOUND
REVIEW_FAILED
```

错误 code 用于：

- 日志搜索；
- 自动化判断；
- UI 显示映射；
- 测试断言；
- 诊断报告。

不要用错误 message 做程序判断。

## 9. 层级边界

### UI / command

负责：

- 创建 root TraceContext；
- 调用 service；
- 把结果转换为 UI 状态；
- 把 error 转换为用户提示。

### Service / use case

负责：

- 业务流程；
- 事务边界；
- 调用多个 repository/external service；
- 添加 service operation span。

### Repository / storage

负责：

- SQLite、文件、Git 或网络存储；
- 将底层错误转换成 typed error；
- 记录 operation、耗时和错误码；
- 不构造 UI 文案。

### External boundary

负责：

- 记录目标类别、状态码、耗时；
- 记录允许保留的 request id；
- 脱敏 URL、header、body；
- 处理取消和超时。

## 10. 日志输出格式

默认文本输出保持 grep-friendly：

```text
TraceId:<id>|Layer:Service|ME:Auth.Login|EV:ENTRY
TraceId:<id>|Layer:Repository|ME:User.find|EV:EXIT|RT:8|Result:SUCCESS
TraceId:<id>|Layer:Repository|ME:User.find|EV:ERROR|RT:9|Error:DATABASE_TIMEOUT
```

内部仍保留 tracing 字段，未来可以增加 JSON sink，不改变字段契约。

推荐字段：

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

## 11. 脱敏

默认禁止日志记录：

- API key；
- token；
- Cookie；
- password；
- 完整 prompt；
- 文件内容；
- Base64 附件；
- 带 query secret 的 URL；
- 未限制长度的外部响应正文。

只记录必要摘要：

```text
body_bytes
prompt_chars
file_path
file_size
content_hash
status_code
request_id
```

## 12. LumaDesk 迁移顺序

1. `lumakit-observability` 增加 `TraceContext`、`OperationGuard` 和 `Trace` 级别；
2. `lumakit-ai::chat_request` 为每次 AI 请求创建 root trace；
3. HTTP/provider 请求创建 child operation；
4. stream、usage、tool、MCP、search 传播 context；
5. 取消、超时和失败使用稳定 error code；
6. LumaDesk 设置页使用现有 `LogHandle::set_level`；
7. LumaDesk 日志查看器按 TraceId 过滤。

## 13. ThinkRail Native 迁移顺序

1. Native 复用 `lumakit-observability`；
2. `state::run()` 传播 TraceContext；
3. HostTool bridge/MCP/OMP worker；
4. Agent process/session；
5. ReviewRunner；
6. Router、Workspace、PlanReview、SettingsStore；
7. Native 日志页增加 TraceId 查询。

## 14. 不要照搬 .NET 的部分

Rust 不建议复制：

- DispatchProxy 式全量反射拦截；
- Service 层直接返回 API `R<T>`；
- Controller 基类重复 try/catch；
- 用字符串 message 判断错误类型；
- 把完整异常正文直接返回给用户；
- 把产品业务字段放进 LumaKit。

Rust 应使用：

```text
thiserror typed error
Result<T, E>
tracing span
explicit TraceContext propagation
boundary response conversion
```

## 15. 验收

LumaKit：

- 五级日志过滤正确；
- level reload 正确；
- TraceContext root/child 正确；
- 跨线程/async/channel 不丢 trace；
- operation 成功/失败/取消/超时均有终态；
- 脱敏测试通过；
- 不依赖产品 crate。

LumaDesk：

- 一次 AI 请求可按 TraceId 找到 request、HTTP、stream、usage 和终态；
- API key、Cookie、prompt 正文不出日志；
- 取消不误报 ERROR；
- 日志级别重启后恢复。

ThinkRail Native：

- 一次 HostTool 能串起 bridge、handler、workspace/plan 和返回；
- 一次 Agent turn 能串起 process、protocol、session 和 cleanup；
- 一次 Reviewer 能串起 worker chat、reviewer chat、verdict 和 cleanup；
- 并发 Chat 不串 TraceId；
- 日志页可以 grep/过滤 TraceId。
