# Rust / LumaKit 日志、请求、响应与异常封装详细设计

状态：详细设计，供后续大模型按步骤实施。

关联文档：

- [概要设计](Rust-LumaKit-日志-请求-响应-异常封装概要设计.md)
- [优化建议](Rust-LumaKit-日志-请求-响应-异常封装优化建议.md)
- [Observability 设计](observability-trace-design.md)

适用项目：LumaKit、LumaDesk、ThinkRail Native 和其他 Rust desktop host。

## 1. 实施边界

### 1.1 第一批文件

只修改以下公共 crate：

```text
LumaKit/crates/lumakit-observability/Cargo.toml
LumaKit/crates/lumakit-observability/src/lib.rs
LumaKit/crates/lumakit-observability/src/trace.rs
LumaKit/crates/lumakit-observability/src/operation.rs
LumaKit/crates/lumakit-observability/src/redact.rs
```

第一批不修改：

```text
lumakit-ai 的业务状态机
LumaDesk 的设置 UI
ThinkRail Native 的 AppPrefs
任何产品数据库 schema
```

第二批消费者迁移：

```text
LumaKit/crates/lumakit-ai/src/chat_request.rs
LumaDesk/src/main.rs
LumaDesk/src/model_settings/*
LumaDesk/src/search_settings/*
ThinkRail Native state::run
ThinkRail Native HostTool / ReviewRunner / Workspace
```

### 1.2 依赖约束

`lumakit-observability` 可以继续使用：

```toml
lumakit-support
serde
serde_json
thiserror
tracing
tracing-subscriber
```

如需生成随机 trace id，优先复用已有 `lumakit-support::new_id`；若该 API 不满足 32 位小写十六进制格式，使用已存在的 `uuid`/随机依赖，不新建重复依赖。

禁止引入：

- GPUI/egui；
- lumakit-ai；
- 产品 crate；
- HTTP client；
- OpenTelemetry SDK；
- 额外全局 singleton。

## 2. 日志级别详细契约

### 2.1 枚举

将现有：

```rust
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}
```

扩展为：

```rust
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}
```

保持现有 `Serialize/Deserialize/Clone/Copy/PartialEq/Eq` 行为和 `level.json` 兼容。

### 2.2 filter

```rust
impl LogLevel {
    fn as_filter(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}
```

环境过滤器的产品模块限制由宿主通过环境变量完成，不在公共 crate 内硬编码 `lumakit_*` 或产品名称。

### 2.3 持久化语义

`LogHandle::init(dir, default, max_bytes)` 保持签名兼容：

- 读取 `level.json`；
- 解析失败使用 `default`；
- 不因未知未来级别导致进程 panic；
- `set_level` 先 reload，再写文件；
- 写文件失败时返回错误，并记录当前内存级别已改变的事实；
- `LogStatus.level` 返回当前内存级别。

产品设置页负责决定是否需要重启；公共 `LogHandle` 支持立即生效。

## 3. TraceContext API

新增文件 `src/trace.rs`：

```rust
use tracing::Span;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceContext {
    trace_id: String,
}

impl TraceContext {
    pub fn root(operation: &'static str) -> (Self, Span);
    pub fn child(&self, operation: &'static str) -> Span;
    pub fn trace_id(&self) -> &str;
}
```

字段规则：

- `trace_id` 对外只读；不暴露任意修改 setter；
- root 使用随机 16-byte 值，lowercase hex 编码；
- root span 创建字段：`trace_id`, `operation`, `event="entry"`; 
- child span 创建字段：`trace_id`, `operation`, `event="entry"`; 
- `Span::enter()` 由调用方在 worker 内使用；
- `TraceContext` 不持有锁和文件句柄。

日志输出层默认格式可以继续使用 tracing subscriber 的文本格式；产品如需 `TraceId:...|ME:...` 的 grep 格式，应通过 formatter 层统一实现，不在业务消息中手写 TraceId。

### 3.1 Root 创建点

每个产品只在真实顶层入口创建 root：

```text
LumaDesk ChatSession::start → ai.request
LumaDesk settings network job → settings.<operation>
Native state::run → ui.background
Native host tool → host_tool.call
Native ReviewRunner → review.run
```

Service/repository 不创建新的 root，只从参数接收 context 并创建 child span。

## 4. OperationGuard API

新增文件 `src/operation.rs`：

```rust
use std::time::Instant;
use tracing::Span;
use crate::TraceContext;

pub struct OperationGuard {
    started: Instant,
    span: Span,
    finished: bool,
}

pub fn operation(
    trace: &TraceContext,
    operation: &'static str,
    layer: &'static str,
) -> OperationGuard;

impl OperationGuard {
    pub fn finish(mut self, result: &'static str);
    pub fn fail(mut self, error_code: &str);
    pub fn cancel(mut self, reason: &str);
    pub fn timeout(mut self, timeout_ms: u64);
}
```

### 4.1 事件实现

`operation()`：

```rust
tracing::debug!(
    parent: &span,
    trace_id = %trace.trace_id(),
    operation,
    layer,
    event = "entry",
    "operation entry"
);
```

`finish()`：

```rust
tracing::debug!(
    parent: &self.span,
    event = "exit",
    result,
    duration_ms = self.started.elapsed().as_millis() as u64,
    "operation exit"
);
```

`fail()`：

```rust
tracing::error!(
    parent: &self.span,
    event = "error",
    error_code,
    duration_ms = ...,
    "operation failed"
);
```

`cancel()` 使用 `info` 或 `warn`，不得默认 error；`timeout()` 使用 warn。

`Drop`：

- 未完成时记录 `event="unfinished"`；
- 不 panic；
- 不覆盖已经完成的状态；
- 必须保证 guard 在跨线程移动时安全。

## 5. 脱敏 API

新增文件 `src/redact.rs`：

```rust
pub fn redact_secret(value: &str) -> String;
pub fn redact_url(value: &str) -> String;
pub fn bounded_text(value: &str, max_chars: usize) -> String;
pub fn diagnostic_hash(value: &str) -> String;
```

规则：

- `redact_secret` 只返回固定 mask 或首尾极短摘要；
- `redact_url` 删除 userinfo、query、fragment；
- `bounded_text` 按 Unicode scalar 截断，不按 byte 中间截断；
- `diagnostic_hash` 不可逆，不把原文写入日志；
- helper 自身不记录日志；
- 测试覆盖 token、URL password、长文本、Unicode。

## 6. RequestContext

`lumakit-observability` 只提供 trace 部分，不提供产品 actor/id 类型。消费者可以定义：

```rust
#[derive(Clone)]
pub struct RequestContext {
    pub trace: lumakit_observability::TraceContext,
    pub request_id: String,
}
```

规则：

- 业务 DTO 不包含 `RequestContext` 字段；
- 请求函数将 context 作为单独参数传递；
- `request_id` 是一次 HTTP/provider/数据库请求；
- `trace_id` 是完整业务操作；
- 外部响应的 request id 必须作为有限字段记录；
- 取消 token 不放入 RequestContext，使用独立 cancellation 参数。

## 7. Typed Error 详细约定

公共 LumaKit 不定义所有产品的 `AppError`，但各产品必须遵循：

```rust
pub type AppResult<T> = Result<T, AppError>;
```

错误 enum 要求：

- `thiserror::Error`；
- 每个可处理变体有稳定 `code()`；
- 保留 `source`；
- 不把用户文案作为错误身份；
- 可在边界转换成安全 response；
- 可在日志中输出 `error_code` 和 `trace_id`。

LumaKit 自身的基础错误继续使用现有 `SupportError` / `LogError`，不把产品领域错误塞进 support crate。

## 8. LumaDesk 详细迁移

### 8.1 `lumakit-ai::chat_request`
现有代码已经创建：

```rust
tracing::info_span!(
    "ai.request",
    conversation_id,
    turn_id,
    attempt_id,
    model,
    protocol,
    image_mode
)
```

迁移为：

1. 创建 `TraceContext::root("ai.request")`；
2. root span 追加 `conversation_id/turn_id/attempt_id/model/protocol`；
3. 将 `TraceContext` move 到 `lumakit-chat` thread；
4. HTTP 请求创建 `trace.child("ai.http.request")`；
5. 工具调用创建 `trace.child("ai.tool.call")`；
6. stream 完成、取消、错误使用 `OperationGuard`；
7. `AiError::Http` 记录 status/request_id/error_code，不记录完整 body；
8. `Event::End` 不携带 trace 正文，必要时携带 `TraceContext` 或稳定 request id。

### 8.2 Settings/network

每个可取消网络 job：

```text
root settings.catalog.refresh
  → child settings.http.request
  → child settings.catalog.persist
```

取消：

```text
EV=CANCEL | reason=user_cancelled | RT=<ms>
```

超时：

```text
EV=TIMEOUT | timeout_ms=<...> | RT=<...>
```

### 8.3 产品错误响应

LumaDesk UI 边界使用：

```rust
struct Notice {
    message: String,
    trace_id: Option<String>,
}
```

普通用户提示可以显示稳定诊断编号，但不显示内部 stack/source。

## 9. Native 详细迁移

### 9.1 `state::run`

当前 `op()` 在 UI background job 周围建立 span。迁移：

```text
TraceContext::root("ui.background").0
  → background executor
  → job span
  → completion callback
```

job 和 completion 都要显式 enter 同一个 context。

### 9.2 HostTool

入口创建：

```text
host_tool.call
fields: tool, chat_id, workspace_id
```

MCP/bridge/OMP worker 传递 context；不要在每个 transport 重新创建 root。

### 9.3 Agent

Session start/turn/protocol/process exit 使用 child spans：

```text
agent.session.start
agent.turn
agent.protocol.request
agent.protocol.response
agent.session.dispose
```

payload 只记录 method/type/id/size/status，不记录完整 prompt 或密钥。

### 9.4 ReviewRunner

```text
review.run
  → review.route
  → review.reviewer_session
  → review.verdict.parse
  → review.verdict.apply
  → review.cleanup
```

`ReviewEvent` 如需 UI 搜索，携带 `TraceContext` 的 trace id；不携带敏感 reviewer prompt。

## 10. 测试设计

### 10.1 observability 单元测试

- `LogLevel` filter mapping；
- Error/Warn/Info/Debug/Trace 序列化；
- `TraceContext::root` id 格式；
- child 保留 trace id；
- operation success/error/cancel/timeout/unfinished；
- redact_secret/redact_url/bounded_text/hash；
- `LogHandle::set_level` 后新日志过滤；
- level file 重启读取；
- writer rotation 保留旧文件。

### 10.2 传播测试

提供测试 subscriber 或 memory layer，验证：

- thread spawn 后 trace_id 相同；
- async task 后 trace_id 相同；
- channel event 携带正确 trace；
- 两个并发 root 互不串线；
- worker 结束后 operation 有终态；
- panic/early return 不造成未观察的生命周期。

### 10.3 LumaDesk 集成测试

- 一次 AI request 可看到 root、HTTP child、completion；
- provider HTTP 失败保留 status/request_id；
- cancel 不记录 ERROR；
- prompt/key/body 不出日志；
- settings job 的 trace 跨 thread。

## 11. 兼容与迁移策略

### 11.1 API

- 保持现有 `LogHandle::init/set_level/status/tail` 签名，除非迁移完成后有明确破坏性版本；
- 现有 `op()` 先保留为兼容 wrapper，内部改用 `TraceContext`；
- 新代码只使用 `TraceContext/operation`；
- 不添加产品字段到 LumaKit 公共 struct；
- 不为单一消费者添加 trait、factory 或全局 registry。

### 11.2 日志输出

第一阶段保持现有文件路径、轮转和 tail 行为；新增字段不删除旧字段。文本格式先稳定，再考虑 JSON sink。

### 11.3 依赖

不得把 `anyhow`、GPUI、AI、HTTP 或数据库引入 `lumakit-observability`，除非当前公共 crate 已经有明确依赖边界变更并更新 ADR。

## 12. 实施任务顺序（迁移顺序）

1. 扩展 `LogLevel` 与 filter；
2. 实现 `TraceContext`；
3. 实现 `OperationGuard`；
4. 实现 redaction helpers；
5. 增加 observability 单元测试；
6. 改 LumaDesk `chat_request`；
7. 改 LumaDesk settings/network；
8. 改 Native `state::run`；
9. 改 Native HostTool/Agent/ReviewRunner；
10. 改两个产品日志页面的 TraceId 查询；
11. 运行两个 workspace 的 fmt/check/clippy/test；
12. 进行 LumaDesk 和 Native 实际操作 smoke。

## 13. 交付标准（验收）

大模型实施时必须先报告：

- 当前调用链；
- 所有受影响调用方；
- 新增公共 API；
- TraceContext 传播点；
- 错误码和边界 response；
- 脱敏字段；
- 测试和真实 smoke 方案。

交付必须报告：

- 修改文件；
- 公共 API 变化；
- 兼容处理；
- 实际运行的验证命令和结果；
- 未验证的真实外部引擎/网络场景；
- 受影响生产文件行数和是否超过 LumaKit 300 行评审线。
