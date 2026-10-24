# LumaKit

Rust 原生桌面应用基础框架：不依赖浏览器、不依赖 WebView、不依赖 Tauri。


## 当前基线与目标能力

- `lumakit-support`：错误、数据根、版本化 JSON 设置、脱敏、ID。
- `lumakit-observability`：结构化 tracing、文件日志、级别、tail、滚动。
- `lumakit-runtime`：可取消后台任务与统一关闭。
- `lumakit-ports`：桌面系统边界的最小 trait 与 fake 实现。
- `lumakit-ai`：三协议 AI 请求、流式响应、工具调用、审批和取消；当前文本/视觉会话是已验证基线，运行事件、候选、搜索、翻译和媒体能力沿聊天设计继续扩展。
- `lumakit-ai::stream_chat`：接收 `AiConnection`、`Vec<ChatMessage>`、取消令牌和增量文本回调；当前负责带历史的文本/视觉三协议请求，工具循环和其它能力由统一 ChatSession 事件链按聊天设计接入。总时限 120 秒、输入上限 1 MiB、原始 SSE 上限 4 MiB；取消、截断及 Responses incomplete 均返回错误，不标记完整成功。
- 模型设置在 `catalog::ModelEntry.settings` 保存，`resolve` 克隆至 `AiConnection`；请求边界再次校验。`model_config` 提供分类、显示名/分组、输出上限、五种推理方言、工具开关、原生搜索和分档价格。模型能力声明只允许对应消费者使用，不能凭元数据冒充已完成的附件、工具或图片生成链路。
- `capabilities.streaming` 在目录解析时同步到 `settings.supports_streaming`：true 请求 SSE，false 请求三协议各自的非流式 JSON 并在完整解码后触发一次文本回调，取消/大小上限仍有效；`settings.supports_tools` 显式值优先，旧 `capabilities.tools=true` 补为已支持，旧 false 保持未知，不冒充明确不支持；显式 false 不向模型公开宿主工具。
- `reasoning.efforts` 是允许强度且首项是默认请求强度；`AiConnection::with_reasoning_effort` 选择列表中的另一项，三协议按实际字段编码。未配置输出上限仍用4096；原生搜索和工具必须经过协议/能力/用户选择门控。
- `context_window` 使用保守预算：每个 Unicode scalar 预估4 token、每条消息32 token、工具 Schema 及完整输出预留；这是防超窗估算而非精确 tokenizer，可能提前拒绝。未设置窗口保持 1 MiB 正文边界。
- `stream_chat_with_usage` / `AiRun.rounds` 返回每个成功上游请求的 usage、当时价格和币种金额。缺 usage 不伪造零费用；失败/取消请求不报告未知的实际账单。
- `lumakit-ai::providers`：Provider/Endpoint/Model 连接快照与非明文密钥状态。
- `lumakit-ai::skills`：受限 SKILL.md、digest 确认、revision 快照与预算。
- `lumakit-ai::mcp`：受信任 Streamable HTTP MCP 工具发现与调用边界。
- `lumakit-ai::images`：图片 MIME、数量、base64 和大小校验。
- `lumakit-ai::search`：搜索结果清理、黑名单和原生搜索能力判定。
- `lumakit-ai::history`：历史迁移、候选关系、崩溃恢复、图片元数据和有限重试策略；目标 schema 由聊天设计定义。
- `lumakit-ai::chat::ChatSession`：UI 中立的会话、尝试、候选、事件、媒体引用、流式任务、保存恢复与关闭仲裁；当前文本链路是基线，能力按设计继续增长。
- `lumakit-egui::install_icon_font`：宿主在 `set_fonts` 前注册随组件分发的 Phosphor 字体；回复栏使用无边框图标、可访问名称及悬停说明，次要动作进入原生更多菜单。LumaDesk 与独立 chat-host 使用同一注册函数，避免独立宿主显示方框。
- `lumakit-ai::catalog::ModelCatalog`：已保存的多线路、多密钥目录，按完整模型身份解析连接；`lumakit-support::secrets` 提供系统加密凭据。

模型目录 `lumakit_ai::catalog_store::CatalogStore` 使用 `providers.db` 的关系表保存服务商、线路、模型和密钥；首次打开在单事务中导入 `model-settings.json`，仅保存密钥密文原字节并以导入标记保证幂等。数据库初始化后不再回读旧 JSON，损坏、缺少初始化标记或未来 schema 版本会直接报错；运行中备份应复制整个数据目录，而非只复制活跃数据库文件。
- `lumakit-ui::chat`：无密钥 ModelOption 与 ChatAction；`lumakit-egui::ChatPanel`：可嵌入原生面板、历史弹窗、模型搜索、安全 Markdown 和公开布局几何，不依赖产品或 eframe。
- `examples/lumakit-cli`：无 UI 组合根；支持 loopback SSE 和环境变量联验入口。

原生组件与消费宿主统一 egui/eframe **0.37.0**，Markdown 使用 egui_commonmark **0.26.0**；未启用实验主题插件。App::logic 负责后台收尾，App::ui 负责绘制；CLI 核心仍不依赖 egui。

## 验证

使用锁定的 Rust 1.99.0：

```bat
cargo +1.99.0 fmt --all --check
cargo +1.99.0 check --workspace --all-targets
cargo +1.99.0 test --workspace
cargo +1.99.0 run -p lumakit-cli
```

CLI 默认只运行 fake 场景；设置 `LUMAKIT_AI_URL` 后追加真实流式请求，`LUMAKIT_AI_KEY` 只在进程内使用。

工作区保留 `missing_docs = "warn"`：公开模块、类型、字段、枚举变体与方法均需 rustdoc。取消不代表任务已完成清理，写入失败不代表旧数据已丢失；注释须说明实际边界，不能用 `allow(missing_docs)` 隐藏缺失。

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
