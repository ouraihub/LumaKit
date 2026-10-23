# LumaKit

Rust 原生桌面应用基础框架：不依赖浏览器、不依赖 WebView、不依赖 Tauri。


## 当前能力

- `lumakit-support`：错误、数据根、版本化 JSON 设置、脱敏、ID。
- `lumakit-observability`：结构化 tracing、文件日志、级别、tail、滚动。
- `lumakit-runtime`：可取消后台任务与统一关闭。
- `lumakit-ports`：桌面系统边界的最小 trait 与 fake 实现。
- `lumakit-ai`：三协议 AI 请求、流式响应、工具调用、审批和取消。
- `lumakit-ai::stream_chat`：接收 `AiConnection`、`Vec<ChatMessage>`、取消令牌和增量文本回调；支持带历史的纯文本三协议对话，不执行工具。总时限 120 秒、输入上限 1 MiB、原始 SSE 上限 4 MiB；取消、截断及 Responses incomplete 均返回错误，不标记完整成功。
- `lumakit-ai::providers`：Provider/Endpoint/Model 连接快照与非明文密钥状态。
- `lumakit-ai::skills`：受限 SKILL.md、digest 确认、revision 快照与预算。
- `lumakit-ai::mcp`：受信任 Streamable HTTP MCP 工具发现与调用边界。
- `lumakit-ai::images`：图片 MIME、数量、base64 和大小校验。
- `lumakit-ai::search`：搜索结果清理、黑名单和原生搜索能力判定。
- `lumakit-ai::history`：会话轮次、崩溃恢复、图片元数据和有限重试策略。
- `lumakit-ai::chat::ChatSession`：UI 中立的 v1 会话、尝试、源模型重试、流式任务、保存恢复与关闭仲裁；文件锁保证一个历史写入者。
- `lumakit-ai::catalog::ModelCatalog`：已保存的多线路、多密钥目录，按完整模型身份解析连接；`lumakit-support::secrets` 提供系统加密凭据。
- `lumakit-ui::chat`：无密钥 ModelOption 与 ChatAction；`lumakit-egui::ChatPanel`：可嵌入原生面板、历史弹窗、模型搜索、安全 Markdown 和公开布局几何，不依赖产品或 eframe。
- `examples/lumakit-cli`：无 UI 组合根；支持 loopback SSE 和环境变量联验入口。

## 验证

使用锁定的 Rust 1.99.0：

```bat
cargo +1.99.0 fmt --all --check
cargo +1.99.0 check --workspace --all-targets
cargo +1.99.0 test --workspace
cargo +1.99.0 run -p lumakit-cli
```

CLI 默认只运行 fake 场景；设置 `LUMAKIT_AI_URL` 后追加真实流式请求，`LUMAKIT_AI_KEY` 只在进程内使用。

## Windows EXE

发布脚本仍收集 CLI 程序；另外提供独立的原生聊天消费宿主（不是完整业务 App）。CLI 打包输出：

```text
dist/lumakit/lumakit-cli.exe
```

在 Windows cmd 中执行：

```bat
scripts\build-app.bat
scripts\run-cli.bat
```

自定义便携 MSVC 路径：

```bat
set PORTABLE_MSVC_ROOT=D:\Applications\msvc-portable\msvc
scripts\build-app.bat
```

`run-cli.bat` 只运行已收集 EXE，不自动编译。独立聊天宿主运行方式：设置 `LUMAKIT_CHAT_DATA` 为隔离可写目录、`LUMAKIT_AI_URL` 为真实服务或回环基础地址、`LUMAKIT_AI_MODEL` 为模型 ID，再执行 `cargo +1.99.0 run -p lumakit-chat-host --release`；可选 `LUMAKIT_AI_KEY` 和 `LUMAKIT_AI_PROTOCOL=responses/messages`。它与 LumaDesk 消费同一个 ChatSession/ChatPanel，不包含笔记或业务页签。

## 边界

LumaKit 不提供桌面业务、不默认接入 Windows Shell、不把 AI 权限交给模型。应用 Host 必须显式声明模型可用的工具、参数校验和审批策略。
