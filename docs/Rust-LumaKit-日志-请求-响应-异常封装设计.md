# Rust / LumaKit 日志、请求、响应与异常封装设计

状态：L1–L5 已实现（`crates/lumakit-observability`），消费者迁移按[任务列表](Rust-LumaKit-日志-请求-响应-异常封装任务列表.md)推进。

本文合并了原来的概要设计、详细设计、优化建议和 `observability-trace-design.md`，作为唯一设计来源。给其他项目大模型用的通用模板见 [traceable-logging-requirement.md](traceable-logging-requirement.md)。MES .NET 使用 `MES.Backend/docs/日志-请求-响应-异常封装优化建议.md`，日志格式与本文一致。

## 1. 目标

拿到一个 TraceId，用 `grep` 就能还原一次操作的完整链路：

```text
UI 动作 / 命令 / AI 请求 / 工具调用 / 后台任务     ← 关键入口：root，INFO 可见
  → service / use case         ← 下层：DEBUG 可见
  → repository / 数据库 / 文件 / git
  → 外部 HTTP / Agent 进程
  → 完成 / 失败 / 取消 / 超时
```

从日志中能看出：

- 哪个入口被调用，结果如何，耗时多少（默认 `Info` 级别即可见）；
- 失败发生在哪一层的哪个操作，错误码是什么（`Debug` 级别可见完整层级）；
- 父子关系（`SpanId` / `ParentSpanId`）；
- 同一链路内第三方库的日志（例如 reqwest 建连）也带同一 TraceId。

## 2. 模块边界

`lumakit-observability` 只提供通用能力，不依赖 UI、AI、数据库或任何产品：

| 能力 | 公开项 |
|---|---|
| 文件日志、轮转、级别持久化与热切换、读尾部 | `LogHandle`、`LogLevel`、`LogStatus`、`LogError` |
| TraceId 与操作生命周期 | `TraceContext`、`OperationGuard`、`operation` |
| 脱敏与摘要 | `redact_secret`、`redact_url`、`bounded_text`、`diagnostic_hash` |
| grep 文本格式 | 由 `LogHandle::init` 内部安装，无公开 API |

产品（LumaDesk、ThinkRail Native）负责：配置持久化、日志设置页与查看页、领域错误类型、用户文案、业务字段（`conversation_id`、`chat_id`、`workspace_id` 等）。

## 3. 日志级别

```rust
pub enum LogLevel { Error, Warn, Info, Debug, Trace }
```

选中某级别时记录该级别及更严重的事件。默认 `Info`。

| 级别 | 用途 | 生命周期事件 |
|---|---|---|
| `Error` | 需要人工处理的失败 | 顶层操作 `EV:ERROR` |
| `Warn` | 降级、超时、可恢复异常、生命周期缺失 | 下层操作 `EV:ERROR`、任何 `EV:TIMEOUT`、`EV:UNFINISHED` |
| `Info` | 启动、关键入口的进入与结束 | 顶层操作 `EV:ENTRY`、`EV:EXIT`、`EV:CANCEL` |
| `Debug` | 每层的进入、出口和耗时 | 下层操作 `EV:ENTRY`、`EV:EXIT`、`EV:CANCEL` |
| `Trace` | 协议帧、底层通信 | 无生命周期事件，由调用方自行记录 |

下层失败记为 `WARN`，只有顶层记一次 `ERROR`，符合《AI工程规范》§4“错误只在最终处理层记录一次”。用户取消不是错误。

`LogHandle::init(dir, default, max_bytes)` 读取 `level.json`，失败时用 `default`；`set_level` 立即重载过滤器并写回 `level.json`。产品可以用自己的配置存储覆盖它。过滤掉的日志不会补录。

## 4. 日志格式（grep 契约）

每个事件一行：

```text
<UTC RFC3339 时间> <级别> TraceId:<id>|SpanId:<n>|ParentSpanId:<n|->|Layer:<layer>|ME:<operation>|[Target:<module>]|[业务 span 字段]|EV:<event>|RT:<ms>|Result:<result>|Error:<code>|Reason:<reason>|Limit:<ms>|[事件字段]|Msg:<message>
```

实际输出（LumaDesk 实窗，回环服务返回 HTTP 429，`Debug` 级别；业务字段已省略）：

```text
2026-11-09T11:05:47.288Z INFO  TraceId:5259…f360|SpanId:5|ParentSpanId:-|Layer:service|ME:ai.request|EV:ENTRY
2026-11-09T11:05:47.289Z DEBUG TraceId:5259…f360|SpanId:2252…|ParentSpanId:5|Layer:external|ME:ai.http.request|conversation_id:…|model:loop-chat|EV:ENTRY
2026-11-09T11:05:47.292Z WARN  TraceId:5259…f360|SpanId:5|ParentSpanId:-|Layer:service|ME:ai.request|Target:lumakit_ai::http|…|endpoint:http://127.0.0.1:18765/v1/chat/completions|status:429|request_id:x-request-id=req-loop-429|Msg:上游拒绝请求；错误正文仅返回调用方，不写入日志
2026-11-09T11:05:47.292Z WARN  TraceId:5259…f360|SpanId:2252…|ParentSpanId:5|Layer:external|ME:ai.http.request|…|EV:ERROR|RT:3|Result:FAILED|Error:AI_HTTP_RATE_LIMITED
2026-11-09T11:05:47.292Z ERROR TraceId:5259…f360|SpanId:5|ParentSpanId:-|Layer:service|ME:ai.request|EV:ERROR|RT:4|Result:FAILED|Error:AI_HTTP_RATE_LIMITED
```

一次失败只有入口一条 ERROR；HTTP 层是 WARN，补充状态码和请求 ID。

规则：

- 段之间用 `|`，键值之间用 `:`。值中的 `|` 写成 `\|`，换行写成 `\n`，所以一个事件只占一行，切分不会错位。
- 生命周期段名 `EV`、`RT`、`Result`、`Error`、`Reason`、`Limit` 只出现在生命周期事件上。普通事件的字段按 tracing 字段名原样输出，即使字段名叫 `event`，也写成 `event:…`，不会冒充 `EV:`，`grep "|EV:ERROR|"` 只会命中真实终态。
- `Target:` 只出现在非生命周期事件上，用来区分来源模块。
- 不在任何操作内的事件没有 `TraceId` 段。
- 宿主在操作 span 下附加的业务 span（例如 `ai.request` 的 `conversation_id/turn_id/attempt_id/model`）会按外到内展开在 `ME` 后面。
- 时间格式由宿主决定（`GrepFormat::utc()` 为 UTC RFC 3339，`GrepFormat::new(timer)` 可换成带时区偏移的格式），但时间段内不能有空格。

### 4.2 写与读用同一份契约

| 公开项 | 用途 |
|---|---|
| `SpanFields` | `tracing_subscriber::Layer`，保存 span 字段，供格式器取 TraceId、Layer、ME 与业务字段 |
| `GrepFormat<T>` | `FormatEvent`，按本节契约写一行 |
| `parse_line(&str) -> LogLine` | 按同一契约解析一行：时间、级别、TraceId、Layer、ME、EV、RT、Target、Msg；不符合格式的行原样放进 `message` 与 `raw`，不臆造字段 |

`LogHandle::init` 已安装 `SpanFields` + `GrepFormat::utc()`。自带 appender 的宿主（如按日滚动）自行组合：

```rust
tracing_subscriber::registry()
    .with(filter)
    .with(SpanFields)
    .with(fmt::layer().event_format(GrepFormat::new(timer)).with_writer(writer));
```

产品日志页只调用 `parse_line`，不再各写一套解析器。

常用检索：

```bash
grep "TraceId:<id>" app.log           # 一次操作的全部日志
grep "|EV:ERROR|" app.log      # 所有失败
grep "ME:ai.request|EV:EXIT" app.log  # 某入口的完成记录及耗时
grep -E "\|EV:(TIMEOUT|UNFINISHED)\|" app.log  # 超时和生命周期缺失
```

### 4.1 生命周期事件

| `EV` | 触发 | `Result` | 附加段 |
|---|---|---|---|
| `ENTRY` | `TraceContext::root` / `operation` 创建时 | — | — |
| `EXIT` | `finish(result)` | 调用方给的摘要，如 `SUCCESS`、`200` | `RT` |
| `ERROR` | `fail(error_code)` | `FAILED` | `RT`、`Error` |
| `CANCEL` | `cancel(reason)` | `CANCELLED` | `RT`、`Reason` |
| `TIMEOUT` | `timeout(limit_ms)` | `TIMEOUT` | `RT`、`Limit` |
| `UNFINISHED` | guard 被丢弃却没有终态（提前 `return`、`?`、panic 展开） | `UNFINISHED` | `RT`、`Reason:guard_dropped` |

每个操作恰好一条终态。`RT` 总是由结束时间减开始时间得出（毫秒），调用方不用自己算。

`Limit` 是调用方配置的时限，不能从耗时推出来。实际耗时与时限对比可以看出超时是否按时触发：例如 `RT:30012|Limit:30000` 正常；`RT:95000|Limit:30000` 说明取消迟到了。

## 5. API

```rust
/// 一次顶层操作及其子操作共享；克隆只复制引用计数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceContext { /* trace_id: Arc<str> */ }

impl TraceContext {
    /// 生成新的 TraceId，以 INFO 写出 EV:ENTRY。只在关键入口调用。
    pub fn root(operation: &'static str, layer: &'static str) -> (Self, OperationGuard);
    pub fn trace_id(&self) -> &str;
}
impl Display for TraceContext { /* 输出 trace_id */ }

/// 在当前 span 下开始下层操作，以 DEBUG 写出 EV:ENTRY。
pub fn operation(trace: &TraceContext, operation: &'static str, layer: &'static str) -> OperationGuard;

impl OperationGuard {
    pub fn span(&self) -> &tracing::Span;   // 跨线程 / async 前克隆并进入
    pub fn finish(self, result: &'static str);
    pub fn fail(self, error_code: &str);
    pub fn cancel(self, reason: &str);
    pub fn timeout(self, limit_ms: u64);
}
// Drop：没有终态时写 EV:UNFINISHED。
```

设计决定：

- **`root` 不返回 `Result`。** TraceId 是 SHA-256(进程号, 进程首次取号时间, 递增计数) 的前 16 字节，输出 32 位小写十六进制。进程内由计数保证唯一，跨进程由进程号和时间区分，没有失败路径。它只用来关联诊断，不用于安全用途。旧版用系统随机源，可能失败，导致每个调用方都要写降级分支（ThinkRail Native 有 5 处）。
- **`root` 直接返回入口 guard。** 入口的 ENTRY 和终态由同一个 guard 负责，调用方无法只建 TraceId 却漏写入口日志。
- **删除 `child()`。** 子操作统一用 `operation()`，保证每层都有 ENTRY 和终态，不会出现只有 span、没有生命周期的层。
- **操作 span 使用 ERROR 级别。** 无论过滤器是什么级别，span 都存在，所以链路内的普通日志和第三方库日志始终带 TraceId。span 本身不产生日志行。
- **root 不挂在当前 span 下。** 在一个链路中新开 root 会得到新的 TraceId 和 `ParentSpanId:-`，不会串到别的链路。
- **`operation` 取当前 span 作父级。** TraceId 由参数 `trace` 决定，父子关系由当前 span 决定。跨线程时不进入父 span，只会缺少 `ParentSpanId`，TraceId 不受影响。

## 6. 使用方式

### 6.1 关键入口

```rust
let (trace, root) = TraceContext::root("settings.catalog.refresh", "ui");
let _entered = root.span().clone().entered();
match refresh(&trace) {
    Ok(_) => root.finish("SUCCESS"),
    Err(error) => root.fail(error.code()),
}
```

关键入口指一次用户可感知的动作、一次 AI 请求、一次后台任务、一次工具调用或一次外部会话。service、repository 不创建 root。

### 6.2 下层

```rust
fn refresh(trace: &TraceContext) -> Result<Catalog, AppError> {
    let op = operation(trace, "catalog.http.fetch", "external");
    match fetch() {
 Ok(catalog) => { op.finish("200"); Ok(catalog) }
        Err(error) => { op.fail(error.code()); Err(error) }
    }
}
```

`?` 提前返回会写出 `EV:UNFINISHED`（WARN）。需要明确失败语义的边界，应先 `match`，再 `fail`。

### 6.3 跨线程与 async

```rust
let span = root.span().clone();
let worker_trace = trace.clone();
std::thread::spawn(move || {
    let _entered = span.entered();
    let op = operation(&worker_trace, "ai.stream", "external");
    // 这里的所有日志都带同一 TraceId，且 ParentSpanId 指向入口
    op.finish("SUCCESS");
});
```

async 任务用 `tracing::Instrument::instrument(fut, span)`；channel 消息需要关联时，在消息里放 `TraceContext`。guard 可以 move 到 worker，在 worker 内结束（参见 `lumakit-ai::chat_request` 的 `ai.request`）。

### 6.4 命名

- `operation`：`<模块>.<对象>.<动作>`，稳定、小写、点分，不带动态参数。例如 `ai.request`、`catalog.http.fetch`、`workspace.git.commit`。
- `layer`：`ui`、`command`、`service`、`repository`、`external`、`worker`，按项目实际分层选用，不强造不存在的层。
- `result`：`SUCCESS` 或有限摘要（如 HTTP 状态码），不放业务正文。
- `error_code`：稳定的大写错误码，如 `DATABASE_TIMEOUT`、`AI_REQUEST_FAILED`，不放错误消息。

## 7. 脱敏

禁止写入日志：API key、token、密码、Cookie、Authorization、完整 prompt、模型输出、文件正文、Base64 附件、带查询参数的 URL、无长度上限的响应正文。

可以记录摘要：`status`、白名单 `request_id`、`prompt_chars`、`body_bytes`、`content_hash`。

| 函数 | 行为 |
|---|---|
| `redact_secret` | 空值返回 `<empty>`，其他一律 `***` |
| `redact_url` | 删除 userinfo、query、fragment |
| `bounded_text` | 按 Unicode 字符截断并加 `…` |
| `diagnostic_hash` | SHA-256 前 8 字节十六进制，不可逆 |

这些函数自身不写日志。

## 8. 请求、错误与响应

- 请求 DTO 只放业务字段，不放 TraceId、连接或 UI 状态。上下文（`TraceContext`、取消令牌）作为独立参数传递。
- `request_id` 标识一次外部请求，`trace_id` 标识完整业务链路，二者不能互相替代。`chat_id`、`conversation_id` 也不能替代 TraceId。
- 业务层返回 `Result<T, E>`。`E` 用 `thiserror` 定义，每个可处理的变体都有稳定的 `code()`，保留 `source`。不按错误消息判断错误类别。
- 业务层不返回 GPUI、HTTP 或 MCP 响应类型。边界层把错误转换为安全响应，至少保留：

```rust
pub struct ErrorResponse { pub code: String, pub message: String, pub trace_id: String }
```

- 用户可见的失败提示附带可复制的 TraceId，不显示堆栈、SQL、密钥或上游原文。
- LumaKit 不定义产品的 `AppError`，只提供 TraceId 与日志。

不照搬 .NET 的做法：DispatchProxy 式全量反射拦截（Rust 在真实边界显式埋点）、Service 返回 API 响应类型、基类重复 try/catch。

## 9. 消费者

### LumaDesk / `lumakit-ai`

已完成（2026-11-09，实窗验证见任务列表 L6）：

| 操作 | 层 | 位置 | 终态 |
|---|---|---|---|
| `ai.request` | `service` | `ChatSession` 每次发送、重试，入口 | `SUCCESS` / `CANCEL user_cancelled` / `TIMEOUT` / `ERROR <AiError::code>` 或 `AI_EMPTY_REPLY`、`AI_REPLY_TOO_LARGE` |
| `ai.http.request` | `external` | 每轮上游模型请求 | 同 `AiError` |
| `ai.tool.call` | `external` | 每次工具执行（审批通过后） | 同 `AiError` |
| `ai.image.generate` | `external` | 生图工具调用的图片接口请求 | 同 `AiError` |

- `AiError::code()` 返回稳定错误码：`AI_MODEL_NOT_CONFIGURED`、`AI_TOOL_DENIED`、`AI_PROVIDER_ERROR`、`AI_HTTP_UNAUTHORIZED`（401/403）、`AI_HTTP_RATE_LIMITED`（429）、`AI_HTTP_REJECTED`（其他 4xx）、`AI_HTTP_UPSTREAM_ERROR`（5xx 等）、`AI_CANCELLED`、`AI_TIMEOUT`、`AI_INVALID_ENDPOINT`。
- 新增 `AiError::Timeout { stage, limit_ms }`。原先超时被折叠成 `Provider("... timed out")` 字符串；现在 reqwest 超时和读取超时都映射到它，日志写 `EV:TIMEOUT|Limit:120000`。
- `AiRuntime::run` / `run_with_history` 新增 `trace: Option<&TraceContext>` 参数；`None` 不写生命周期。`stream_chat` 无链路。
- 翻译入口（`ChatSession::translate`）暂未建 root，只走 `None`。
- 待做（L7）：设置页的目录刷新、连接检测、搜索测试和 MCP 发现入口。

### ThinkRail Native

当前固定在 LumaKit `b5c0207`，仍是旧 API（`root` 返回 `Result`，有 `child`）。迁移（任务列表 L8）：

- 删除 `agents/trace.rs`、`app/services.rs`、`app/state.rs`、`review/dispatch.rs`、`host-tools/bridge.rs`、`host-tools/mcp.rs` 中 `Err(_)` 的降级分支；
- `child(...)` 改为 `operation(&trace, ..., layer)`，每层都有 ENTRY 和终态；
- 保留按日滚动和北京时间，`FlatFormat` 换成 `GrepFormat::new(OffsetTime)` + `SpanFields`；`tail` 用 `parse_line`；删除旧的 `op()` / `finish()`。

## 10. 验证要求

公共 crate：`cargo test -p lumakit-observability`、`cargo clippy -p lumakit-observability --all-targets -- -D warnings`。测试通过内存 writer 读取真实格式化输出，覆盖：TraceId 格式与唯一性、链路父子关系与耗时、六种终态及其级别、Info 级别下的入口可见性、跨线程传播、嵌套 root 隔离、分隔符转义。

消费者：运行真实操作，按 TraceId 在 `app.log` 中找到入口、下层与终态；成功、失败、取消各观察一次；扫描日志确认没有密钥和正文。编译或单元测试通过不能代替链路观察。
