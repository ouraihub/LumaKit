# Rust / LumaKit 日志、请求、响应与异常封装任务列表

> 设计：[Rust-LumaKit-日志-请求-响应-异常封装设计.md](Rust-LumaKit-日志-请求-响应-异常封装设计.md)。
>
> 每个任务只服务一个可验证目标。完成时填写实际运行的验证命令和结果。

## 1. 规则

状态：`TODO`、`DOING`、`BLOCKED`（写明阻塞原因）、`DONE`、`SKIPPED`（写明范围决策）。

开始任务前必须：读取设计文档和本任务；搜索目标符号的全部调用方；不向 `lumakit-observability` 加产品依赖；不删除或放宽测试。

只有在以下条件都满足时才能标 `DONE`：实现已落盘；直接测试通过；受影响的 workspace 检查通过；文档已更新；真实链路已观察（消费者任务）。

Windows 下 Git 自带的 `link.exe` 会遮住 MSVC 链接器，cargo 命令需要经 `..\LumaDesk\scripts\cargo-msvc.bat` 执行（加 `--manifest-path Cargo.toml`）。

## 2. 依赖

```text
L1–L5 公共 crate（DONE）─ L6 LumaDesk AI 请求（DONE）
  ├─ L7 LumaDesk 设置/网络入口 ─┬─ L10 产品日志查看页 ─ L11 双 workspace 验证与真实 smoke
  └─ L8 Native 升级依赖 ─ L9 Native 下层 ─┘
```

## 3. 公共 crate `lumakit-observability`

| ID | 内容 | 状态 |
|---|---|---|
| L1 | `LogLevel` 五级、`level.json` 兼容、运行时重载 | `DONE` |
| L2 | `TraceContext::root(operation, layer) -> (Self, OperationGuard)`，不会失败；32 位小写十六进制 TraceId | `DONE` |
| L3 | `OperationGuard`：ENTRY 写成独立日志行；EXIT/ERROR/CANCEL/TIMEOUT/UNFINISHED 恰好一个终态；`RT` 自动计算；`timeout(limit_ms)` 输出 `Limit` | `DONE` |
| L4 | grep 格式 `TraceId:…\|SpanId:…\|ParentSpanId:…\|Layer:…\|ME:…\|EV:…\|RT:…`，值内分隔符转义，业务 span 字段展开；公开 `SpanFields`、`GrepFormat`、`parse_line` 供自带 appender 的宿主和日志页复用 | `DONE` |
| L5 | 脱敏：`redact_secret`、`redact_url`、`bounded_text`、`diagnostic_hash` | `DONE` |

L1–L5 记录（2026-11-09）：

- 修改：`src/trace.rs`、`src/operation.rs`、`src/format.rs`（新增）、`src/lib.rs`、`src/tests.rs`；`lumakit-ai` 新增依赖，`chat_request.rs` 的 `ai.request` 改用 root；`examples/lumakit-cli` 增加 `cli.run` 入口。
- 公共 API 变化：`root` 由 `Result<(Self, Span), String>` 改为 `(Self, OperationGuard)`，并新增 `layer` 参数；删除 `child`；新增 `OperationGuard::span`；`timeout` 参数改名为 `limit_ms`。
- 检查：`cargo test -p lumakit-observability`，8 项通过；`cargo test -p lumakit-ai -p lumakit-observability`，全部通过；`cargo clippy -p lumakit-observability --no-deps --all-targets -- -D warnings` 通过；LumaDesk `cargo test`，37 项通过。
- 真实链路：运行 `lumakit-cli`，`app.log` 中出现入口 ENTRY/EXIT（INFO）和下层 ENTRY/EXIT（DEBUG），父子 SpanId 正确。把 `LUMAKIT_AI_URL` 指向一个关闭的端口后，reqwest/hyper 的内部日志带同一 TraceId，终态为 `EV:ERROR|RT:2062|Error:AI_RUN_FAILED`（ERROR）。
- 文件规模：`format.rs` 241 行、`lib.rs` 240 行、`operation.rs` 157 行，都低于 300 行。
- 未验证：LumaDesk 实窗 AI 请求链路（已在 L6 完成）。
- 后续补充（同日）：
  - 公开 `SpanFields`、`GrepFormat<T>`（`utc()` / `new(timer)`）与 `parse_line`，新增 `src/parse.rs`；
  - 修正：普通事件的 `event` 字段不再映射为 `EV:`，只有生命周期事件使用保留段名；
  - 原有问题已处理：`lumakit-cli` 改为显式写 stdout，写失败返回错误；`lumakit-support` 补 `# Errors`；workspace 跑过 rustfmt，此前未格式化的 `chat_request.rs` 单独提交为 `style: rustfmt chat_request`。
  - 检查：`cargo test -p lumakit-observability`，10 项通过；`cargo clippy -p lumakit-observability --all-targets -- -D warnings`、`cargo clippy -p lumakit-support -p lumakit-cli --no-deps --lib --bins -- -D warnings` 通过。
  - 仍有：`cargo clippy --workspace --all-targets -D warnings` 不通过，`lumakit-ai`/`lumakit-support` 的测试代码有大量 `unwrap`，`lumakit-ai` 的旧公开函数缺 `# Errors`、可合并的 `if` 等，合计约 280 处，均为本次之前已有。

## 4. LumaDesk

### L6. AI 请求下层操作 — `DONE`

文件：`crates/lumakit-ai/src/lib.rs`、`chat_generation.rs`、`chat_request.rs`；`examples/lumakit-cli`。

- `AiError::code()` 与新变体 `AiError::Timeout { stage, limit_ms }`；reqwest 超时和读取超时不再折叠成字符串。
- `AiRuntime::run` / `run_with_history` 增加 `trace: Option<&TraceContext>`；下层操作 `ai.http.request`、`ai.tool.call`、`ai.image.generate`，终态由 `end_operation` 统一按 `AiError` 写 CANCEL / TIMEOUT / ERROR。
- 入口 `ai.request` 由 worker 按实际结果结束，失败写具体错误码（`AI_HTTP_RATE_LIMITED`、`AI_EMPTY_REPLY` 等），不再统一写 `AI_REQUEST_FAILED`。
- 上游拒绝的说明日志由 ERROR 降为 WARN，避免一次失败出现两条 ERROR。
- 未做：单独的 `ai.image.download` 操作（下载包含在 `ai.image.generate` 内，超时以 `stage:image_download`、`Limit:60000` 区分）；翻译入口未建 root。

验收记录（2026-11-09）：

- 方式：LumaDesk debug 构建、隔离数据目录 `LUMADESK_DATA_DIR`、本机回环 OpenAI Chat 服务（不连外网）；用真实 `CatalogStore` API 写入回环服务商；通过 UIA 操作实窗选择模型、发送和停止，用 PrintWindow 截取 LumaDesk 窗口。
- 成功：界面显示回答；日志 `ai.request` ENTRY → `ai.http.request` ENTRY/EXIT（DEBUG）→ `ai.request` `EV:EXIT|RT:604|Result:SUCCESS`。
- 上游 429：界面显示“回答失败，请重试或更换模型”，诊断详情默认折叠；日志 `ai.http.request` `EV:ERROR|Error:AI_HTTP_RATE_LIMITED`（WARN）、状态说明行带 `status:429|request_id:x-request-id=req-loop-429`（WARN）、入口 `EV:ERROR|Error:AI_HTTP_RATE_LIMITED`（ERROR，仅一条）。
- 取消：生成中点“停止生成”，界面显示“已停止，保留部分回答”；日志入口 `EV:CANCEL|RT:14504|Reason:user_cancelled`（INFO）。
- 每次请求的 TraceId 都不同；HTTP 子操作的 `ParentSpanId` 指向入口 `SpanId`。
- 敏感信息：在 `app.log` 中搜索测试密钥、上游错误正文和两条用户消息原文，0 处命中。
- 检查：`cargo test --workspace`（LumaKit）全部通过；LumaDesk `cargo test` 37 项通过。
- 未验证：真实供应商；超时终态只在单元路径上，未在实窗触发 120 秒超时。

### L7. 设置与网络入口 — `TODO`

文件：`LumaDesk/src/model_settings/*`、`search_settings/*`、`mcp_settings/*`。

每个可取消的网络任务（目录刷新、连接检测、搜索测试、MCP 发现）在用户动作处创建 root，HTTP 和持久化使用下层 `operation`。设置失败不能留下 `UNFINISHED`。

## 5. ThinkRail Native

### L8. 升级 LumaKit 依赖 — `DONE`

把 `Cargo.toml` 中固定的 `b5c0207` 升级到包含 L1–L6 的提交，并完成设计 §9 列出的改动：删除 `Err(_)` 降级分支，`child` 改为 `operation`；保留按日滚动和北京时间，格式换成 `GrepFormat` + `SpanFields`，`tail` 用 `parse_line`，删除旧 `op()` / `finish()`，同步 `tests/init.rs` 与 `tests/tail.rs`。

记录（2026-11-09，Native `c7046db`，分支 `feat/models-page-review`，LumaKit `a5b9bf3`）：

- 修改：`crates/observability/src/{lib,tail}.rs`，删除 `format.rs`、`op.rs`；`tests/{init,tail}.rs` 按新格式重写；`app/src/state.rs`、`services.rs`、`services/review/dispatch.rs`、`host-tools/src/{bridge,mcp}.rs`、`agents/src/trace.rs`、`app/src/panels/settings/logs.rs`。
- `LogEntry` 删除 `spans`、`target`，新增 `event`、`duration_ms`；`component` 优先取业务行的 `Target`，生命周期行取 `Layer`。
- 检查：`cargo fmt --all -- --check` 通过；`cargo clippy --workspace --all-targets -- -D warnings` 通过；`cargo test --workspace` 324 项通过、0 失败。
- 真实程序：用隔离 `--data-dir` 启动 debug 构建。日志文件中共 20 个 trace，每个都有终态；重启后调试级别从存储中恢复（`level:debug`）。日志页列表显示 `ENTRY` / `EXIT 25ms`；把 TraceId 填进筛选框后，只剩该操作的 ENTRY、业务行和 EXIT 三行。
- 文件规模：`mcp.rs` 311 行（原 306）、`services.rs` 356 行（原 369）、`logs.rs` 444 行（原 423），都超过 300 行评审线，且在本次之前就已超过；均未超过 500 行。

### L9. Native 下层 — `DOING`

在 `state::run`、HostTool bridge/MCP worker、Agent session/process、ReviewRunner 中补齐 `operation`，覆盖成功、失败、取消和超时终态。

已完成（随 L8）：

- `ui.background`：root，结果写 EXIT，或带 `ErrorCode` 写 ERROR。
- `host_tool.call`：root，`is_error` 时写 `host_tool_error`。
- `review.run`：root，覆盖取消、失败和 SUCCESS；线程启动失败时由 guard 写 UNFINISHED。
- `host_bridge.request`：每个请求一个 operation。
- `mcp.call`：每次调用一个 operation，引擎撤回写 CANCEL，管道失败写 `MCP_BRIDGE_FAILED`；`mcp.session` 在输入结束、全部 worker 退出后写 `CLOSED`。
- `agent.session` 与 `host_bridge.session` 是长期关联上下文，root 立即以 `REGISTERED` 结束，下层沿用其 span 和 TraceId。

未完成：Agent process/turn/request 边界目前只是普通 span，还没有 ENTRY/终态；超时终态没有覆盖；这些路径还没有实窗验证。

## 6. 产品与验证

### L10. 日志查看页 — `DOING`

按 TraceId、`ME`、`Layer`、`EV` 过滤；复制 TraceId；按时间显示同一 trace 的事件和 `RT`；默认显示 INFO 及以上。解析规则以设计 §4 为准。

Native 进度（`f92555c`）：

- 每行显示 TraceId 前 8 位，做成链接按钮。点击后把完整 TraceId 填入筛选框，只显示这条链路；再点一次取消筛选。
- 只在 ENTRY 行放「复制」按钮，复制完整 TraceId。生命周期行显示 `EV`、`RT` 和 `Error`/`Reason`。
- 实窗验证：点 `8300bcec` 后列表从 93 条减到 2 条（ENTRY、EXIT 1ms）；点「复制」后剪贴板内容为 `8300bcec198dff6e223aa6d44ba6e7f8`；再点一次恢复 93 条。
- 键盘与可访问性：用 Tab 能聚焦到标签，聚焦环可见；按空格可触发筛选，按回车无效（gpui 按钮的统一行为）。UIA 角色为 Hyperlink，可访问名称为「只看 Trace xxxxxxxx… 的日志；再点一次取消筛选」或「复制」。
- `logs.rs` 共 473 行，超过 300 行评审线，未超过 500 行。

未完成：`EV`/`Layer` 专用筛选；失败提示中显示可复制的 TraceId；LumaDesk 日志页尚未开始（LumaDesk 设置页目前没有日志页，`LogHandle` 只在启动时初始化）。

### L11. Workspace 验证与真实 smoke — `TODO`

LumaKit、LumaDesk、Native 分别运行 fmt、check、clippy、test；对 L6、L7、L9 的真实操作各观察一次成功、失败和取消链路，并扫描日志中是否有敏感信息。
