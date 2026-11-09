# LumaKit Observability 与 TraceContext 概要设计

状态：概要设计，待实施。

## 1. 背景

LumaKit 当前的 `lumakit-observability` 已提供：

- `LogLevel::{Error, Warn, Info, Debug}`；
- 文件日志与大小轮转；
- `LogHandle::init/set_level/status/tail`；
- `tracing-subscriber`、运行时 reload；
- LumaDesk 的实际日志初始化与 AI 请求结构化事件。

LumaDesk 和 ThinkRail Native 都需要更强的诊断链路：一个用户动作、AI 请求或后台任务发生后，可以通过一个稳定 `trace_id` 搜索入口、业务层、存储/网络层、外部进程、完成、异常和清理日志。

目标不是为每个产品建立一套日志系统，而是把**通用日志基础设施和传播契约**放入 LumaKit；产品自己的业务字段、日志设置页和日志查看页仍留在产品仓库。

## 2. 目标与非目标

### 2.1 目标

- 每个顶层操作拥有稳定的 `trace_id`；
- 子操作拥有 `tracing` span 和父子层级；
- TraceContext 可以显式跨线程、async task、channel 和 worker 传播；
- 方法边界统一记录 `entry / exit / error / cancel` 和 `duration_ms`；
- 日志级别支持 `Error / Warn / Info / Debug / Trace`；
- 文件日志级别可持久化、启动恢复、运行时 reload；
- 统一字段命名，支持纯文本 tail 和未来 JSON sink；
- 提供脱敏、截断和有限摘要工具；
- LumaDesk 与 ThinkRail Native 复用同一套 API。

### 2.2 非目标

- 不提供 GPUI/egui 日志设置页；
- 不拥有 AppPrefs、SQLite schema 或产品配置格式；
- 不定义 `chat_id`、`workspace_id`、`conversation_id`、`review_id` 等产品字段；
- 不依赖 `lumakit-ai`、UI crate、网络或具体数据库；
- 第一阶段不接 OpenTelemetry、OTLP、Jaeger 或远端 collector；
- 不自动通过反射/宏扫描所有业务方法；调用方必须在真实边界显式埋点；
- 不把日志消息翻译成 UI locale。

## 3. Crate 边界

继续使用：

```text
LumaKit/crates/lumakit-observability
```

依赖保持：

```text
lumakit-support
serde / serde_json
thiserror
tracing
tracing-subscriber
```

依赖方向：

```text
LumaDesk ───────────────┐
ThinkRail Native ───────┼──> lumakit-observability
其他 Rust desktop host ─┘
```

LumaKit 不反向依赖产品。

## 4. 日志级别

### 4.1 级别语义

```text
Error：系统错误、未处理异常、进程失败
Warn ：取消、超时、业务拒绝、可恢复错误
Info ：正常启动、完成、关键生命周期
Debug：方法入口、方法出口、耗时、关键摘要
Trace：协议帧、底层通信和极细节诊断
```

过滤规则：选择的级别包含该级别和更严重事件。

```rust
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}
```

默认仍然是 `Info`。产品可通过自己的偏好存储保存当前级别，启动后调用 `LogHandle::set_level` 恢复。

### 4.2 环境覆盖

保留环境变量覆盖机制，例如：

```text
LUMAKIT_LOG=lumakit_ai=debug
THINKRAIL_LOG=thinkrail_native=trace
```

环境覆盖优先于产品保存的级别，适用于启动故障、CI 和临时诊断。环境覆盖存在时，产品 UI 的级别写入仍可保存，但不覆盖当前进程过滤器。

## 5. TraceContext

在 `lumakit-observability` 中增加通用上下文：

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

约定：

- `trace_id` 为 32 位小写十六进制字符串；
- root id 使用随机源生成，不使用自增数字作为跨组件诊断 id；
- span id 由 tracing subscriber 管理；
- 不引入全局 mutable current trace；
- Context 可 clone，调用方显式传给 worker；
- `operation` 使用稳定、机器可搜索的名称，例如 `ai.request`、`host_tool.call`、`workspace.git_commit`。

### 5.1 统一字段

```text
trace_id
span_id
parent_span_id
operation
component
layer
event
workspace_id
chat_id
backend
duration_ms
result
error_code
```

LumaKit 只定义通用字段；产品可以追加领域字段。

## 6. Operation API

提供一个轻量的公共 operation 工具：

```rust
pub struct OperationGuard { /* private */ }

pub fn operation(
    trace: &TraceContext,
    name: &'static str,
    layer: &'static str,
) -> OperationGuard;

impl OperationGuard {
    pub fn finish(self, result: &'static str);
    pub fn fail(self, error_code: &str);
    pub fn cancel(self, reason: &str);
}
```

行为：

- 创建 child span；
- 记录 `event=entry`；
- `finish` 记录 `event=exit`、`duration_ms`、`result`；
- `fail` 记录 `event=error`、`duration_ms`、`error_code`；
- `cancel` 记录 `event=cancel`、`duration_ms`、`reason`；
- Drop 未显式结束时记录受控的 `event=unfinished`，避免静默丢失生命周期。

`#[tracing::instrument]` 仍可使用，但不取代 operation：重要边界需要明确的完成/失败语义。

## 7. 跨线程传播

公共 crate 不维护线程本地业务上下文。调用方必须显式传播：

```rust
let trace = TraceContext::root("ai.request").0;
let worker_trace = trace.clone();
std::thread::spawn(move || {
    let _span = worker_trace.child("ai.http").entered();
    // worker logs keep the same trace_id
});
```

必须传播的边界：

- LumaDesk `lumakit-ai` 请求 worker；
- Native `state::run` background job；
- host tool bridge/MCP worker；
- Agent process reader；
- ReviewRunner；
- `async_channel` 事件；
- workspace、存储和网络 worker。

TraceContext 需要实现轻量 clone，不持有锁、不持有文件、不绑定 runtime。

## 8. 脱敏与摘要

公共 crate 提供通用安全边界：

```rust
pub fn redact_secret(value: &str) -> String;
pub fn redact_url(url: &str) -> String;
pub fn bounded_text(value: &str, max_chars: usize) -> String;
pub fn diagnostic_hash(value: &str) -> String;
```

规则：

- API key、token、password、Cookie、Authorization 不记录；
- prompt 正文不记录，必要时只记录长度和 hash；
- 文件正文、附件、Base64 不记录；
- URL 去掉 query、fragment 和凭据；
- 错误正文有界并脱敏；
- request id 只接受明确白名单字段。

## 9. 两个真实消费者的字段边界

### LumaDesk

允许追加：

```text
conversation_id
turn_id
attempt_id
model
protocol
provider
request_id
```

`lumakit-ai` 保留现有请求 id、HTTP request id、usage 和取消日志；改为使用公共 TraceContext 作为请求根。

### ThinkRail Native

允许追加：

```text
workspace_id
chat_id
tool
item_id
reviewer_chat_id
backend
```

Native 的 `ReviewRunner`、HostTool、Workspace、Router 和 Agent session 使用公共 Context。

### 不进入 LumaKit

- AI 业务状态；
- ChatSession；
- PlanReview；
- Git 归属；
- 用户语言；
- GPUI 日志页面；
- 产品错误文案。

## 10. 日志输出与产品 UI

LumaKit 继续提供：

- file writer；
- level reload；
- status；
- tail；
- rotation。

产品各自负责：

- AppPrefs / 设置存储；
- 日志级别选择 UI；
- trace id 搜索；
- operation/layer/component 过滤；
- 复制诊断 id；
- 错误提示和诊断报告。

纯文本输出必须稳定、grep 友好；未来 JSON layer 使用同一字段名，不重新设计业务字段。

## 11. 迁移顺序

### Phase 1：LumaKit

- 扩展 `LogLevel` 增加 `Trace`；
- 抽取 `TraceContext`；
- 实现 `OperationGuard`；
- 增加脱敏工具；
- 增加 root/child/finish/fail/cancel/跨线程测试；
- 保持现有 `LogHandle` 调用兼容，除非迁移完成后可以做干净切换。

### Phase 2：LumaDesk

先迁移已有、边界清晰的 `lumakit-ai`：

1. chat request root；
2. HTTP request child；
3. stream / usage / cancellation；
4. search、MCP 和模型目录刷新；
5. LumaDesk 启动和 settings background job。

用 LumaDesk 验证两个真实产品级场景：AI 请求全链路与设置后台任务。

### Phase 3：ThinkRail Native

- `crates/observability` 替换为 LumaKit 依赖；
- `state::run` 传播 context；
- HostTool bridge/MCP/OMP worker；
- Agent session/process；
- ReviewRunner/ReviewEvent；
- Router/Workspace/PlanReview/SettingsStore；
- 日志页 trace 搜索。

## 12. 验收

LumaKit crate：

- Error/Warn/Info/Debug/Trace 过滤正确；
- level reload 与文件持久化调用不耦合；
- root/child span 字段稳定；
- thread、async、channel context 传播测试通过；
- operation success/error/cancel/unfinished 测试通过；
- 脱敏测试覆盖 URL、token、prompt、错误正文；
- 不依赖 UI、AI 或产品 crate。

LumaDesk：

- 一次 AI 请求可按 trace_id 找到 request、HTTP、stream、usage、cancel/fail、complete；
- 正常取消记录 Info，不伪装成 Error；
- API key、Cookie、prompt 正文不出现在日志。

ThinkRail Native：

- 一个 host tool 可串起 bridge、handler、plan/workspace、completion；
- 一个 Agent turn 可串起 process、protocol、session、settled、cleanup；
- Reviewer 可串起 worker chat、temporary reviewer chat、verdict、cleanup；
- trace 不跨 chat/request 串线；
- 日志页可按 trace_id 搜索和复制。

## 13. 设计决策

- 公共能力进入 `lumakit-observability`，不新建第二个日志 crate；
- 业务字段由消费者追加，不把产品模型塞进 LumaKit；
- 默认 Info，Debug/Trace 用于诊断；
- TraceId 是稳定业务诊断键，span 是局部调用层级；
- 显式传播优先于线程本地推断；
- 先做本地结构化日志，不提前接远端 tracing 基础设施；
- 先迁 LumaDesk，再迁 Native，因为 LumaDesk 已经在使用 LumaKit observability，迁移风险最低。
