# Rust / LumaKit 日志、请求、响应与异常封装任务列表

> 配套设计：
>
> - `Rust-LumaKit-日志-请求-响应-异常封装概要设计.md`
> - `Rust-LumaKit-日志-请求-响应-异常封装详细设计.md`
> - `observability-trace-design.md`
>
> 本表供人和大模型跟进实施。每个任务只服务一个可验证目标；完成任务时必须填写实际验证命令和结果。

## 1. 状态与任务规则

状态值：

```text
TODO       尚未开始
BLOCKED    有明确外部依赖，必须写明阻塞原因
DOING      当前正在实施
DONE       代码、测试和验收均完成
SKIPPED    有记录的范围决策，不是静默删除
```

每个任务必须记录：

- 负责人/执行会话；
- 修改文件；
- 依赖任务；
- 公共 API 变化；
- 验证命令与真实结果；
- 未验证项；
- 生产文件行数是否超过 LumaKit 300 行评审线。

大模型开始任务前 MUST：

1. 读取本表对应任务和三个设计文档；
2. 搜索目标符号的全部调用方；
3. 不新增产品依赖到 `lumakit-observability`；
4. 不为了通过检查删除或放宽测试；
5. 先写实施计划，再修改代码。

## 2. 总依赖图

```text
L1 日志级别
  ↓
L2 TraceContext
  ↓
L3 OperationGuard + 脱敏
  ↓
L4 observability tests
  ↓
L5 LumaDesk AI request
  ↓
L6 LumaDesk settings/network
  ↓
L7 Native state::run
  ↓
L8 Native HostTool/Agent/ReviewRunner
  ↓
L9 产品日志查看器
  ↓
L10 双 workspace 验证与 smoke
```

## 3. LumaKit 公共 crate

### L1. 扩展日志级别

| 字段 | 内容 |
|---|---|
| ID | L1 |
| 状态 | `TODO` |
| 文件 | `crates/lumakit-observability/src/lib.rs` |
| 依赖 | 无 |
| 目标 | 支持 `Error/Warn/Info/Debug/Trace` |

实施：

- 给 `LogLevel` 增加 `Trace`；
- 更新 `as_filter()`；
- 保持 serde 兼容；
- 保持 `level.json` 读取；
- 保持 `LogHandle::init/set_level/status/tail` API；
- 环境变量优先于持久化级别；
- 不修改日志目录、文件名、轮转行为。

验收：

- 每个级别过滤正确；
- 旧 `level.json` 可读取；
- 运行时 reload 不重启进程；
- `cargo test -p lumakit-observability` 通过。

### L2. 实现 TraceContext

| 字段 | 内容 |
|---|---|
| ID | L2 |
| 状态 | `TODO` |
| 文件 | `crates/lumakit-observability/src/trace.rs`、`src/lib.rs` |
| 依赖 | L1 |
| 目标 | 统一 root/child trace 和显式传播 |

API：

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceContext {
    pub trace_id: String,
}

impl TraceContext {
    pub fn root(operation: &'static str) -> (Self, tracing::Span);
    pub fn child(&self, operation: &'static str) -> tracing::Span;
    pub fn trace_id(&self) -> &str;
}
```

实施约束：

- 32 位小写十六进制 TraceId；
- 不使用全局 mutable current trace；
- context 可 clone；
- root/child span 包含 `trace_id`、`operation`、`event`；
- 不依赖产品字段；
- 不引入第二个 tracing subscriber。

验收：

- root id 格式稳定；
- child 保留 trace id；
- 两个并发 root 不串线；
- 公开 API 有 rustdoc。

### L3. 实现 OperationGuard

| 字段 | 内容 |
|---|---|
| ID | L3 |
| 状态 | `TODO` |
| 文件 | `crates/lumakit-observability/src/operation.rs`、`src/lib.rs` |
| 依赖 | L2 |
| 目标 | 统一方法 ENTRY/EXIT/ERROR/CANCEL/TIMEOUT 生命周期 |

API：

```rust
pub struct OperationGuard { /* private */ }

pub fn operation(
    trace: &TraceContext,
    operation: &'static str,
    layer: &'static str,
) -> OperationGuard;

impl OperationGuard {
    pub fn finish(self, result: &'static str);
    pub fn fail(self, error_code: &str);
    pub fn cancel(self, reason: &str);
    pub fn timeout(self, timeout_ms: u64);
}
```

字段：

```text
TraceId
SpanId
ParentSpanId
Layer
ME / operation
EV
RT / duration_ms
Result
Error
```

规则：

- 创建时记录 `event=entry`；
- 完成记录 `event=exit`、`duration_ms`、`result`；
- 失败记录 `event=error`、`error_code`；
- 取消使用 Warn/Info，不伪装成 Error；
- Drop 未结束时记录 `event=unfinished`；
- 不记录业务正文和敏感参数。

验收：

- success/error/cancel/timeout/unfinished 都有测试；
- duration 非负；
- 重复终结调用不会重复写终态；
- `cargo clippy -p lumakit-observability --all-targets -- -D warnings` 通过。

### L4. 实现脱敏与摘要

| 字段 | 内容 |
|---|---|
| ID | L4 |
| 状态 | `TODO` |
| 文件 | `crates/lumakit-observability/src/redact.rs`、`src/lib.rs` |
| 依赖 | L1 |
| 目标 | 公共安全边界 |

API：

```rust
pub fn redact_secret(value: &str) -> String;
pub fn redact_url(value: &str) -> String;
pub fn bounded_text(value: &str, max_chars: usize) -> String;
pub fn diagnostic_hash(value: &str) -> String;
```

验收：

- password/token/API key 不泄漏；
- URL userinfo/query/fragment 被处理；
- Unicode 截断不破坏字符；
- 长正文有上限；
- hash 不可逆且测试稳定。

### L5. observability 单元测试与兼容检查

| 字段 | 内容 |
|---|---|
| ID | L5 |
| 状态 | `TODO` |
| 依赖 | L1-L4 |
| 目标 | 锁定公共 crate 行为 |

测试：

- LogLevel 序列化和 filter mapping；
- `level.json` 重启读取；
- writer rotation；
- TraceContext root/child；
- thread/async/channel 传播；
- OperationGuard 所有终态；
- 脱敏 helper；
- 现有 LumaKit consumers 编译不变。

验收命令：

```bat
cargo +1.99.0 fmt --all -- --check
cargo +1.99.0 test -p lumakit-observability
cargo +1.99.0 clippy -p lumakit-observability --all-targets -- -D warnings
```

## 4. LumaDesk 迁移

### L6. AI 请求 TraceContext

| 字段 | 内容 |
|---|---|
| ID | L6 |
| 状态 | `TODO` |
| 文件 | `crates/lumakit-ai/src/chat_request.rs` |
| 依赖 | L5 |
| 目标 | 把当前 `ai.request` span 迁移到公共 TraceContext |

实施：

- 每次 AI turn 创建 root `ai.request`；
- 保留 `conversation_id/turn_id/attempt_id/model/protocol/image_mode`；
- worker thread 显式 clone context；
- HTTP 请求创建 child `ai.http.request`；
- tool、stream、usage、cancel、failure 使用 child operation；
- `AiError::Http` 记录状态码和白名单 request id；
- 不记录完整 prompt、授权 header、完整响应正文。

验收：

- 一次真实 AI request 能按 TraceId 找到 start/http/complete 或 failure；
- cancel 记录 Info/Warn，不记录 ERROR；
- failure 保留 typed error；
- `lumakit-ai` tests 通过。

### L7. LumaDesk settings/network

| 字段 | 内容 |
|---|---|
| ID | L7 |
| 状态 | `TODO` |
| 文件 | `LumaDesk/src/model_settings/*`、`search_settings/*`、相关网络模块 |
| 依赖 | L5、L6 |
| 目标 | 配置刷新、搜索和 MCP 也可以按 TraceId 还原 |

验收：

- 每个可取消网络 job 有 root/child；
- 取消、超时、HTTP 失败有终态；
- 密钥不出日志；
- 设置失败不会留下未完成 operation。

## 5. ThinkRail Native 迁移

### L8. Native state::run

| 字段 | 内容 |
|---|---|
| ID | L8 |
| 状态 | `TODO` |
| 文件 | ThinkRail Native `crates/app/src/state.rs` |
| 依赖 | L5 |
| 目标 | 替代当前单纯 `op()` 传播，保留兼容期行为 |

实施：

- root `ui.background`；
- background executor 显式 clone context；
- completion callback 重新 enter；
- 旧 `op()` 先作为 wrapper；
- success/error/cancel 都有终态。

### L9. Native HostTool/Agent

| 字段 | 内容 |
|---|---|
| ID | L9 |
| 状态 | `TODO` |
| 文件 | HostTool bridge、MCP、OMP、Agent process/session |
| 依赖 | L8 |
| 目标 | tool call、engine turn、protocol 和 cleanup 同 TraceId |

验收：

- HostTool 入口、handler、workspace/plan、返回可串联；
- Agent process start/request/response/settled/exit 可串联；
- MCP worker thread 不丢 TraceId；
- cancellation/timeout 有终态；
- 不输出 token、prompt、文件正文。

### L10. Native ReviewRunner

| 字段 | 内容 |
|---|---|
| ID | L10 |
| 状态 | `TODO` |
| 文件 | Native app ReviewRunner、ReviewEvent |
| 依赖 | L9 |
| 目标 | worker chat、reviewer chat、verdict、cleanup 可串联 |

验收：

- 每次 review 一个 root trace；
- 临时 reviewer session 是 child；
- verdict parse/apply 有 child；
- router release、dispose、support cleanup 有终态；
- DSH async 事件保持 trace_id；
- cancel/failure/timeout 不泄漏 running entry。

## 6. 产品日志查看器

### L11. TraceId 搜索

| 字段 | 内容 |
|---|---|
| ID | L11 |
| 状态 | `TODO` |
| 依赖 | L5、L8 |
| 目标 | LumaDesk/Native 日志页能够按 TraceId 查询 |

功能：

- TraceId 文本过滤；
- operation/layer/component 过滤；
- 一键复制 TraceId；
- 显示事件时间顺序和 duration；
- 默认显示 Info/Warn/Error，Debug/Trace 可开启；
- 不改变日志文件 rotation/tail 契约。

## 7. 全局验证

### L12. Workspace 验证

| 字段 | 内容 |
|---|---|
| ID | L12 |
| 状态 | `TODO` |
| 依赖 | L1-L11 |
| 目标 | 两个消费者同时稳定 |

LumaKit：

```bat
cargo +1.99.0 fmt --all -- --check
cargo +1.99.0 check --workspace --all-targets
cargo +1.99.0 test --workspace
cargo +1.99.0 clippy --workspace --all-targets -- -D warnings
```

LumaDesk：

```bat
scripts\cargo-msvc.bat fmt --all -- --check
scripts\cargo-msvc.bat check --workspace --all-targets
scripts\cargo-msvc.bat test --workspace
scripts\cargo-msvc.bat clippy --workspace --all-targets -- -D warnings
```

Native：使用其现有 MSVC wrapper 跑同等检查。

### L13. 真实 smoke

| 字段 | 内容 |
|---|---|
| ID | L13 |
| 状态 | `TODO` |
| 依赖 | L6-L12 |
| 目标 | 真实边界行为证明链路，不只证明编译 |

必须实际观察：

- LumaDesk AI 请求成功/失败/取消；
- LumaDesk settings network 请求；
- Native host tool；
- Native Agent turn；
- Native ReviewRunner；
- 通过 TraceId 找到入口、出口和错误；
- 敏感信息扫描没有命中。

## 8. 状态跟进模板

每次开始任务复制一行到变更记录：

```text
ID: L1
Status: DOING
Owner: <name/session>
Changed: <files>
Depends: -
Check: <command>
Result: <actual result>
Unverified: <none or exact limitation>
```

完成只能在以下条件同时满足时标记 `DONE`：

- 实现已落盘；
- 直接测试通过；
- 受影响 workspace 检查通过；
- 文档/公共 API 已更新；
- 没有未说明的兼容风险。

## 9. 大模型实施交接模板

> 请实施任务 `<ID>`。先阅读本任务、概要设计和详细设计，搜索目标符号全部调用方。不要扩大范围。
>
> 实施前报告：当前调用链、预计修改文件、公共 API 影响、TraceContext 传播点、脱敏点和验证命令。
>
> 实施后报告：修改文件、实际行为、测试命令及结果、未验证项、文件规模和后续依赖。不要把编译通过当作运行链路验证。
