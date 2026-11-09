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

### L8. 升级 LumaKit 依赖 — `DOING`

把 `Cargo.toml` 中固定的 `b5c0207` 升级到包含 L1–L6 的提交，并完成设计 §9 列出的改动：删除 `Err(_)` 降级分支，`child` 改为 `operation`；保留按日滚动和北京时间，格式换成 `GrepFormat` + `SpanFields`，`tail` 用 `parse_line`，删除旧 `op()` / `finish()`，同步 `tests/init.rs` 与 `tests/tail.rs`。

### L9. Native 下层 — `TODO`

在 `state::run`、HostTool bridge/MCP worker、Agent session/process、ReviewRunner 中补齐 `operation`，覆盖成功、失败、取消和超时终态。

## 6. 产品与验证

### L10. 日志查看页 — `TODO`

按 TraceId、`ME`、`Layer`、`EV` 过滤；复制 TraceId；按时间显示同一 trace 的事件和 `RT`；默认显示 INFO 及以上。解析规则以设计 §4 为准。

### L11. Workspace 验证与真实 smoke — `TODO`

LumaKit、LumaDesk、Native 分别运行 fmt、check、clippy、test；对 L6、L7、L9 的真实操作各观察一次成功、失败和取消链路，并扫描日志中是否有敏感信息。
