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
  ├─ L7 LumaDesk 设置/网络入口 ─┬─ L10 产品日志查看页 ─ L11 三个 workspace 验证与真实 smoke
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
  - 当时严格 workspace clippy 被约 280 处旧警告阻塞；本次续作已用 Result 测试传播、准确 Errors 文档及等价 lint 修复清除，未屏蔽 lint 或放宽断言。2026-11-09 续作严格 workspace clippy 已通过。

## 4. LumaDesk

### L6. AI 请求下层操作 — `DONE`

文件：`crates/lumakit-ai/src/lib.rs`、`chat_generation.rs`、`chat_request.rs`；`examples/lumakit-cli`。

- `AiError::code()` 与新变体 `AiError::Timeout { stage, limit_ms }`；reqwest 超时和读取超时不再折叠成字符串。
- `AiRuntime::run` / `run_with_history` 增加 `trace: Option<&TraceContext>`；下层操作 `ai.http.request`、`ai.tool.call`、`ai.image.generate`，终态由 `end_operation` 统一按 `AiError` 写 CANCEL / TIMEOUT / ERROR。
- 入口 `ai.request` 由 worker 按实际结果结束，失败写具体错误码（`AI_HTTP_RATE_LIMITED`、`AI_EMPTY_REPLY` 等），不再统一写 `AI_REQUEST_FAILED`。
- 上游拒绝的说明日志由 ERROR 降为 WARN，避免一次失败出现两条 ERROR。
- 图片下载仍包含在 `ai.image.generate` 内，超时以 `stage:image_download`、`Limit:60000` 区分；续作已为翻译建立真实 `ai.request` root，不再走无链路路径。

验收记录（2026-11-09）：

- 方式：LumaDesk debug 构建、隔离数据目录 `LUMADESK_DATA_DIR`、本机回环 OpenAI Chat 服务（不连外网）；用真实 `CatalogStore` API 写入回环服务商；通过 UIA 操作实窗选择模型、发送和停止，用 PrintWindow 截取 LumaDesk 窗口。
- 成功：界面显示回答；日志 `ai.request` ENTRY → `ai.http.request` ENTRY/EXIT（DEBUG）→ `ai.request` `EV:EXIT|RT:604|Result:SUCCESS`。
- 上游 429：界面显示“回答失败，请重试或更换模型”，诊断详情默认折叠；日志 `ai.http.request` `EV:ERROR|Error:AI_HTTP_RATE_LIMITED`（WARN）、状态说明行带 `status:429|request_id:x-request-id=req-loop-429`（WARN）、入口 `EV:ERROR|Error:AI_HTTP_RATE_LIMITED`（ERROR，仅一条）。
- 取消：生成中点“停止生成”，界面显示“已停止，保留部分回答”；日志入口 `EV:CANCEL|RT:14504|Reason:user_cancelled`（INFO）。
- 每次请求的 TraceId 都不同；HTTP 子操作的 `ParentSpanId` 指向入口 `SpanId`。
- 敏感信息：在 `app.log` 中搜索测试密钥、上游错误正文和两条用户消息原文，0 处命中。
- 检查：`cargo test --workspace`（LumaKit）全部通过；LumaDesk `cargo test` 37 项通过。
- 未验证：真实供应商；超时终态只在单元路径上，未在实窗触发 120 秒超时。

### L7. 设置与网络入口 — `DONE`

文件：`LumaDesk/src/model_settings/*`、`search_settings/*`、`mcp_settings/*`。

目录刷新、模型检测/同步、搜索检测/搜索/读取在用户动作处创建 root；HTTP 和持久化使用下层 `operation`。`settings_task::Failure` 明确区分稳定错误码、HTTP、取消与配置时限，正常失败不留下 `UNFINISHED`。MCP 当前产品只提供配置保存，未提供发现动作；现有 load/save 已覆盖 root 与 repository，未额外增加发现功能。

续作验收（2026-11-09，debug 实窗、本机回环、隔离数据目录）：

- 模型检测成功 `9db1bb17…`：HTTP 子操作 EXIT 12ms，入口 EXIT 44ms；429 `2b5c0dee…`：子操作 WARN、入口唯一 ERROR，错误码 `SETTINGS_HTTP_RATE_LIMITED`，复制按钮得到完整对应 TraceId。
- 取消 `e1689e65…`：HTTP 与入口均 CANCEL；实际延迟响应 `7c7654e2…`：HTTP TIMEOUT 20004ms、入口 TIMEOUT 20022ms，两者 `Limit:20000`。
- 目录强制刷新 `f81413cb…`：load、HTTP、apply 与入口同链路，入口 SUCCESS 59ms；失败刷新 `4e4647b5…` 保留目录，picker 能复制完整失败 ID。
- 搜索读取：真实回环 HTML 成功 `170a2b06…` EXIT 48ms；取消 `200a64b1…` CANCEL 284ms；HTTP 429 `4ecf0232…` 以稳定码 ERROR；总时限 `adef2deb…` TIMEOUT 20027ms / Limit 20000。
- 所观察的 37 个生命周期各只有一个终态、无 UNFINISHED；INFO 期间两个下层 ENTRY 被过滤但 WARN 终态保留，符合阈值契约。日志中测试密钥、上游原文、用户消息、模型回答与网页正文均 0 命中。
- `cargo test --workspace`（LumaDesk）40 项通过；fmt、all-targets check 与严格 clippy 已通过。所有命令经 cargo-msvc.bat。

## 5. ThinkRail Native

### L8. 升级 LumaKit 依赖 — `DONE`

把 `Cargo.toml` 中固定的 `b5c0207` 升级到包含 L1–L6 的提交，并完成设计 §9 列出的改动：删除 `Err(_)` 降级分支，`child` 改为 `operation`；保留按日滚动和北京时间，格式换成 `GrepFormat` + `SpanFields`，`tail` 用 `parse_line`，删除旧 `op()` / `finish()`，同步 `tests/init.rs` 与 `tests/tail.rs`。

记录（2026-11-09，Native `c7046db`，分支 `feat/models-page-review`，LumaKit `a5b9bf3`）：

- 修改：`crates/observability/src/{lib,tail}.rs`，删除 `format.rs`、`op.rs`；`tests/{init,tail}.rs` 按新格式重写；`app/src/state.rs`、`services.rs`、`services/review/dispatch.rs`、`host-tools/src/{bridge,mcp}.rs`、`agents/src/trace.rs`、`app/src/panels/settings/logs.rs`。
- `LogEntry` 删除 `spans`、`target`，新增 `event`、`duration_ms`；`component` 优先取业务行的 `Target`，生命周期行取 `Layer`。
- 检查：`cargo fmt --all -- --check` 通过；`cargo clippy --workspace --all-targets -- -D warnings` 通过；`cargo test --workspace` 324 项通过、0 失败。
- 真实程序：用隔离 `--data-dir` 启动 debug 构建。日志文件中共 20 个 trace，每个都有终态；重启后调试级别从存储中恢复（`level:debug`）。日志页列表显示 `ENTRY` / `EXIT 25ms`；把 TraceId 填进筛选框后，只剩该操作的 ENTRY、业务行和 EXIT 三行。
- 文件规模：`mcp.rs` 311 行（原 306）、`services.rs` 356 行（原 369）、`logs.rs` 444 行（原 423），都超过 300 行评审线，且在本次之前就已超过；均未超过 500 行。

### L9. Native 下层 — `DONE`

在 `state::run`、HostTool bridge/MCP worker、Agent session/process、ReviewRunner 中补齐 `operation`，覆盖成功、失败、取消和超时终态。

已完成（随 L8）：

- `ui.background`：root，结果写 EXIT，或带 `ErrorCode` 写 ERROR。
- `host_tool.call`：root，`is_error` 时写 `host_tool_error`。
- `review.run`：root，覆盖取消、失败和 SUCCESS；拒绝线程启动时由未交接载体明确写 `REVIEW_WORKER_START_FAILED`。
- `host_bridge.request`：每个请求一个 operation。
- `mcp.call`：每次调用一个 operation，引擎撤回写 CANCEL，管道失败写 `MCP_BRIDGE_FAILED`；`mcp.session` 在输入结束、全部 worker 退出后写 `CLOSED`。
- `agent.session` 与 `host_bridge.session` 是长期关联上下文，root 立即以 `REGISTERED` 结束，下层沿用其 span 和 TraceId。

续作完成 Agent process start/exit、request waiter 与 turn 的 OperationGuard；请求达到配置时限写 TIMEOUT/Limit；错误与用户停止不会在后续 settle 被改写为 SUCCESS，取消后仍等待引擎真实 settle 才解锁输入。Pi/OMP/DSH 的既有 prompt/stop 调用方已迁移。新增回归覆盖 turn 错误/取消终态、状态顺序和真实子进程的请求超时。

## 6. 产品与验证

### L10. 日志查看页 — `DONE`

按 TraceId、`ME`、`Layer`、`EV` 过滤；复制 TraceId；按时间显示同一 trace 的事件和 `RT`；默认显示 INFO 及以上。解析规则以设计 §4 为准。

Native 进度（`f92555c`）：

- 每行显示 TraceId 前 8 位，做成链接按钮。点击后把完整 TraceId 填入筛选框，只显示这条链路；再点一次取消筛选。
- 只在 ENTRY 行放「复制」按钮，复制完整 TraceId。生命周期行显示 `EV`、`RT` 和 `Error`/`Reason`。
- 实窗验证：点 `8300bcec` 后列表从 93 条减到 2 条（ENTRY、EXIT 1ms）；点「复制」后剪贴板内容为 `8300bcec198dff6e223aa6d44ba6e7f8`；再点一次恢复 93 条。
- 键盘与可访问性：用 Tab 能聚焦到标签，聚焦环可见；按空格可触发筛选，按回车无效（gpui 按钮的统一行为）。UIA 角色为 Hyperlink，可访问名称为「只看 Trace xxxxxxxx… 的日志；再点一次取消筛选」或「复制」。
- `logs.rs` 共 473 行，超过 300 行评审线，未超过 500 行。

续作 LumaDesk / 共享聊天进度（2026-11-09）：

- 新增 `LumaDesk/src/log_settings.rs` 与 `log_settings/view.rs`，复用 `LogHandle`、`parse_line`，按时间显示最近 1000 行；支持 TraceId/ME/模块文本搜索、EV/Layer 交集筛选、点击 TraceId 看整链路、复制诊断 ID/筛选结果、运行时五级切换和打开目录。仅页面打开时后台读日志，不生成轮询日志。
- 实窗输入 `2b5c0dee…` 后复制完整失败链路，再选 EV ERROR + Layer external 只剩 HTTP WARN 失败行；设置 INFO 后重启恢复为 Info。
- `Attempt.trace_id` 保存真实请求 ID，SQLite 仅追加 v6 nullable 列，旧历史不伪造 ID；失败诊断保持默认折叠，展开可复制完整 ID。回环 429 复制 `4a8d64a8…` 与日志入口一致，重启后仍一致；键盘 Space 可触发复制。
- 已观察 320/375/414/768 逻辑宽度下日志页与失败诊断布局，并截取真实窗口；成功与生成中无新增常驻复制控件。原始日志行按需横向滚动，诊断默认折叠。
- Native 的 EV/Layer 专用筛选已在 `facfe73` 提交；失败提示已有用户未提交实现，续作保留，最终验证记录见 L11。

### L11. Workspace 验证与真实 smoke — `DONE`

LumaKit、LumaDesk、Native 分别运行 fmt、check、clippy、test；对 L6、L7、L9 的真实操作各观察一次成功、失败和取消链路，并扫描日志中是否有敏感信息。

续作已运行：

- LumaKit：`cargo test --workspace --manifest-path ..\LumaKit\Cargo.toml --target-dir target --locked`，186 项通过、0 失败；`cargo check --workspace --all-targets`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check` 均通过。均使用显式 Kit manifest 与 LumaDesk 的 cargo-msvc.bat。
- LumaDesk：`cargo test --workspace --locked` 40 项通过；`check --all-targets --locked`、`clippy --all-targets -- -D warnings`、`fmt --all -- --check` 均通过；debug 实际程序成功/失败/取消/超时与日志页验证见 L7/L10。
- 线程启动 guard 交接修正后另跑 `cargo test -p lumakit-ai`，105 项通过，包括失败/重试/翻译 ID 分离及 v5 数据迁移；未改真实 AI 请求行为。
- 实窗证据保存在临时隔离目录 `C:\Users\Administrator\AppData\Local\Temp\lumadesk-logging-te3ab0dd`，包含 app.log、隔离数据库与各宽度截图；无正式用户数据。
- Native：`check --workspace --all-targets`、`fmt --all -- --check`、`clippy --workspace --all-targets -- -D warnings` 均通过；`test --workspace` 332 项通过、0 失败，16 项既有下载/外部引擎/ConPTY smoke 保持 ignored。另已实窗运行本机真实 Pi 与回环服务。

Native 真实链路（隔离目录 `C:\Users\Administrator\AppData\Local\Temp\native-logging-ph7rednw`）：

- 真实 Node/Pi 启动，`agent.process.start` ENTRY/EXIT；同一 `9f40af18…` session trace 包含 request 及 turn。成功 turn EXIT 99ms；回环 HTTP 400 turn ERROR 10ms / EXTERNAL；生成中点停止 turn CANCEL 13112ms / user_cancelled，未改引擎状态机的 settle 顺序。
- 真实子进程请求超时回归 `pending_real_process_request_records_timeout_with_configured_limit` 已通过，终态 TIMEOUT、Limit 20ms；`turn_failure_and_cancellation_do_not_become_success_or_unlock_before_settle` 已通过。
- 最终实窗失败复制 `bfd28d61c9c340b4fb10ee3562b7b32b`，日志页粘贴并选「失败 + service」只剩该 turn 的 WARN ERROR 62ms 行，复制内容一致。`SessionError.trace_id` 缺省 None，投影边界附加真实 ID。
- 测试 prompt、模型回答、占位密钥与上游错误正文在 Native 日志中均 0 命中，无 UNFINISHED。真实外网供应商未连接；OMP/DSH 未做真实引擎发送，协议和状态路径由现有 workspace 测试覆盖。AI 120 秒超时没有实窗等待，设置20秒超时与Native请求20ms超时已观察/执行。

规模评审（续作格式化后）：

- LumaDesk `main.rs` 469 行（组合根/状态）、`app_view.rs` 323 行（壳层渲染），原 769 行主文件已分离渲染职责；日志控制器 169、日志 view 256、settings_task 131 行。
- 模型设置状态 403、目录刷新 378、搜索设置 403、搜索 HTTP 407 行；均按状态、worker、HTTP、持久化职责分离，新增模块未超过 500 行。
- Kit 聊天请求准备 369、翻译 156、worker 211 行；诊断视图 62 行。共享 GPUI 主视图仍为既有 1137 行，只抽离本次诊断职责，未改其它消息/输入布局；继续拆其它渲染职责会扩大此次日志范围，因此保留为明确例外，后续修改相应渲染时应继续拆分。已有模型设置大视图同理，本次只增加真实诊断复制并移出同步持久化。
- 非聊天 AI 原大文件此次只做准确文档、Result 测试传播与等价 lint 清理，不增加业务职责；不以机械搬运掩盖既有规模。行为验证由上述 workspace 测试覆盖。
- Native process 370、streams 90、projection 394、request waiter 157、trace 72、聊天view 260、core agent合同324行；均低于500行。进程拥有启动/退出资源，projection拥有turn终态，UI只消费错误诊断ID；未引入第二套状态机。

## 2026-11-10 LumaDesk 日志异常规范复查与整改

旧 L6/L7/L11 DONE 为当时网络链路验收，不代表普通保存、启动失败和panic均满足新规范。本轮补充共享安全panic、源码路径净化、真实flush、HTTP/SSE错误隐私、聊天保存失败去重，以及宿主配置保存和worker失败优先级。公共API签名不变；外部request-id改诊断哈希、HTTP body固定安全说明。`ai.request`保留，网络完成结果明确为 `NETWORK_COMPLETED_PENDING_SAVE`；最终保存单独结算。panic hook为唯一最终ERROR所有者，join WARN，异常中断允许UNFINISHED，不以正常取消掩盖。

本轮通过：LumaDesk63项、AI117项、observability15项单元和1项真实panic子进程回归；宿主及两个共享crate严格clippy、fmt与受影响编译通过。panic覆盖debug unwind与release abort、载荷隐私与链路。实窗观察启动失败、搜索成功/429/取消、搜索与模型保存trigger失败及恢复；16个root均一个终态，合成秘密/正文/SQL/URL路径零命中。完整命令、截图目录、模块规模及未验证范围见 [LumaDesk BUILD](../../LumaDesk/docs/BUILD.md#2026-11-10-日志与异常规范整改)。不将本轮验证扩大为Native或Kit其它crate复测，也不声称dist正式包已更新。
