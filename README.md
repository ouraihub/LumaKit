# LumaKit

Rust 原生桌面应用基础库：不依赖浏览器、WebView 或 Tauri。当前服务两个产品：LumaDesk（AI agent 工作台）与冰蓝复刻（Windows 桌面伴侣，`../rust-desktop-preview`）。

修改前先读 [AGENTS.md](AGENTS.md)：什么能进 Kit、依赖方向、界面令牌规则，以及聊天面板的参考约束。

## Crate

| crate | 用途 | 消费方 |
|---|---|---|
| `lumakit-support` | 错误、数据根、文件锁、原子写、版本化 JSON、SQLite 迁移、系统加密凭据、脱敏、ID | 全部 |
| `lumakit-observability` | 结构化 tracing、文件日志、运行时改级别、状态、读尾部、滚动 | 全部 |
| `lumakit-design` | GPUI 设计系统：主题、令牌、字体、图标、组件、外观偏好与外观页 | GPUI 宿主 |
| `lumakit-ai` | AI 协议、会话、目录、搜索、MCP、技能；聊天动作 `chat::{ChatAction, ModelOption}` | AI 宿主 |
| `lumakit-gpui` | GPUI 聊天面板 | LumaDesk |
| `lumakit-egui` | egui 聊天面板，`examples/chat-host` 独立宿主 | LumaDesk `main` 分支；GPUI 版合并后删除 |

`examples/lumakit-cli` 是无界面组合根，验证 AI 运行时与日志。

## lumakit-design

GPUI（gpui-kit **0.9.0**）设计系统，提炼自 ThinkRail Native `crates/ui`：

- 主题目录：ThinkRail 六套 + 暖纸 `warm-light`/`warm-dark`，支持用户主题文件；颜色角色由调色板推导并映射到 Kit `ThemeColor`；`LumaTheme::with_accent` 强调色覆盖。
- `metrics`：间距、尺寸、圆角、字体样式令牌；`Typography`：中文回退与密度缩放。
- 内置 Geist / JetBrains Mono / Orbitron（OFL，许可随 `assets/fonts`）；图标源 `UiAssets`。
- `components`：无状态组件；`components::settings`：设置页模式（分组导航、group/row/field、链接标签、头像、虚线空态、提示、列表行与行菜单）。
- `appearance`：外观偏好 `Preferences`（主题 id、跟随系统的浅/深主题对、强调色、密度）、切换规则与 `apply`，以及无状态外观页 `AppearancePage`。序列化字段名是稳定 schema；持久化与旧格式迁移由宿主负责。

宿主启动顺序：注册 `UiAssets`、调用 `fonts::register_bundled`，再 `Preferences::apply`（或 `LumaTheme::install`）。

## lumakit-ai

- `lumakit-ai`：三协议 AI 请求、流式响应、工具调用、审批和取消；当前文本/视觉会话是已验证基线，运行事件、候选、搜索、翻译和媒体能力沿聊天设计继续扩展。
- `lumakit-ai::stream_chat`：接收 `AiConnection`、`Vec<ChatMessage>`、取消令牌和增量文本回调；当前负责带历史的文本/视觉三协议请求，工具循环和其它能力由统一 ChatSession 事件链按聊天设计接入。总时限 120 秒、输入上限 1 MiB、原始 SSE 上限 4 MiB；取消、截断及 Responses incomplete 均返回错误，不标记完整成功。
- 模型设置在 `catalog::ModelEntry.settings` 保存，`resolve` 克隆至 `AiConnection`；请求边界再次校验。`model_config` 提供分类、显示名/分组、输出上限、五种推理方言、工具开关、原生搜索和分档价格。模型能力声明只允许对应消费者使用，不能凭元数据冒充已完成的附件、工具或图片生成链路。
- `capabilities.streaming` 在目录解析时同步到 `settings.supports_streaming`：true 请求 SSE，false 请求三协议各自的非流式 JSON 并在完整解码后触发一次文本回调，取消/大小上限仍有效；`settings.supports_tools` 显式值优先，旧 `capabilities.tools=true` 补为已支持，旧 false 保持未知，不冒充明确不支持；显式 false 不向模型公开宿主工具。
- `reasoning.efforts` 只声明模型支持档位，不决定当前请求强度；`AiConnection::with_reasoning_effort` 设置临时请求参数，不重排能力列表。聊天未显式选择时使用服务商默认，不自动发送首档 low；显式关闭仅在模型支持时发送。未配置输出上限仍用4096；原生搜索和工具经过协议/能力/用户选择门控。
- `context_window` 使用保守预算：每个 Unicode scalar 预估4 token、每条消息32 token、工具 Schema 及完整输出预留；这是防超窗估算而非精确 tokenizer，可能提前拒绝。无法识别且未配置窗口时运行时使用32000预算，界面仍标窗口未知；1 MiB 正文边界独立保留。`max_input` 可另设输入上限，包含系统、工具与附件估算，超过上限会在发送前拒绝。
- `stream_chat_with_usage` / `AiRun.rounds` 保存每个成功上游请求的稳定请求 ID、时间、耗时、真实模型、usage、价格和费用来源。完整输入已含缓存，推理 token 是输出子集，不重复加总。缺 usage/价格不伪造零费用；取消或失败请求未取得的用量不冒充账单。
- `ChatSession::usage_report` 提供当前对话或持久化全局账本统计：按模型、服务商、UTC日期及时间范围汇总，费用按币种独立累计。chat.db schema 5 的 `usage_requests` 使用请求 ID 去重，复制/分支不新增消耗，删除聊天不删除既有账本。旧记录无请求 ID/时间时标历史导入与无日期；历史副本无法可靠追溯去重，日期筛选排除无日期记录。
- `ChatSession::context_usage` 是提示词占用估算，不是累计消耗或剩余输出额度：只复用模型及请求前缀指纹匹配的成功单轮输入锚点，新增回复/草稿仍按本地估算；历史、模型、工具或附件变化不匹配时回退当前内容估算，不参与安全准入判断。
- `model_reference` 内置 OMP 的版本化 MIT 模型目录投影，离线精确识别模型 ID 与规范提供方前缀，显式配置优先。仅支持的协议能力和可表示的参考价格会补全，不猜版本、不模糊识别别名；定时费率和歧义模型价格仍未知。参考金额是 token 估值，不是中转账单；来源 revision、SHA-256、日期与许可保存在目录资产并随发布声明分发。
- `ModelEntry::effective_settings(protocol)` 是保存目录、模型列表筛选、聊天选择器和运行连接共同使用的能力入口。精确目录的输入模态/明确工具声明补全未知视觉与工具能力；`vision_override: Some(false)`、`supports_tools: Some(false)` 优先，旧配置 true 保留。能力图标只显示已声明支持，未知不显示，不通过模型名字猜测。
- `lumakit-ai::providers`：Provider/Endpoint/Model 连接快照与非明文密钥状态。
- `lumakit-ai::skills`：受限 SKILL.md、digest 确认、revision 快照与预算。
- `lumakit-ai::mcp`：受信任 Streamable HTTP MCP 工具发现与调用边界。
- `lumakit-ai::images`：图片 MIME、数量、base64 和大小校验。
- `lumakit-ai::search`：搜索结果清理、黑名单和原生搜索能力判定。
- `lumakit-ai::history`：历史迁移、候选关系、崩溃恢复、图片元数据和有限重试策略；目标 schema 由聊天设计定义。
- `lumakit-ai::chat::ChatSession`：UI 中立的会话、尝试、候选、事件、媒体引用、流式任务、保存恢复与关闭仲裁；当前文本链路是基线，能力按设计继续增长。
- `ChatSession::toolbar/set_toolbar` 保存同一 `chat.db` 全局共用的工具栏显示偏好和顺序，去重时保留首次出现的位置，默认固定新对话和网络搜索；`ChatAction::SetToolbar` 由宿主转交会话。固定/取消固定及排序不创建对话、不修改搜索或生图请求控制；保存失败保留原偏好与顺序，旧记录缺字段使用默认值，未知工具忽略但不丢弃历史。
- `ChatAction`（`lumakit_ai::chat`）：渲染器交给宿主执行的聊天动作；`ModelOption`：无密钥、可展示的模型来源。两个聊天面板共用。
- 模型选择器偏好由 `ChatSession::model_picker_preferences` 提供：完整身份的收藏、最多十项最近使用、新对话默认 `ModelPair`。`select_model_pair` 在一次保存中提交模型、推理强度和最近记录，写入失败整体回滚；`None` 继承模型默认，`Some(ReasoningEffort::None)` 明确关闭推理。默认组合只用于新对话，首次输入保留已选待用组合，已有对话切换恢复其模型；目录协调裁剪失效偏好与不支持的强度，不改已发送请求快照。
- `lumakit-ai::catalog::ModelCatalog`：已保存的多线路、多密钥目录，按完整模型身份解析连接；`lumakit-support::secrets` 提供系统加密凭据。

模型目录 `lumakit_ai::catalog_store::CatalogStore` 使用 `providers.db` 的关系表保存服务商、线路、模型和密钥；首次打开在单事务中导入 `model-settings.json`，仅保存密钥密文原字节并以导入标记保证幂等。数据库初始化后不再回读旧 JSON，损坏、缺少初始化标记或未来 schema 版本会直接报错；运行中备份应复制整个数据目录，而非只复制活跃数据库文件。

## 聊天面板

- `lumakit-gpui::ChatPanel`：GPUI 聊天面板，样式只取自 `lumakit-design`；Markdown 先经 pulldown-cmark 清理，不加载图片。
- GPUI 输入区“＋”菜单使用 256px 锚定浮层；“自定义工具栏”提供图标、拖动手柄、显示开关与恢复默认。手柄可拖放或用上下键调整已固定工具的顺序，关闭后重开、应用重启均保留。只列出实际实现的新对话、网络搜索、上传附件、生成图片，不改变请求开关或执行工具。未固定工具仍可从“＋”菜单执行；Esc 或点击外部关闭浮层。
- GPUI 模型与推理强度合为一个入口及 360px 浮层：固定/最近/供应商分组、两行元数据、模糊搜索与 `模型名 high` 组合搜索；切换模型保持浮层打开，下方离散滑杆支持拖动、档位点击、左右键。默认状态独立于各档位，可恢复服务商默认；高开销档位提示费用与耗时。收藏/默认组合持久化，刷新通过 `RefreshModels` 交给宿主；元数据使用显式配置或可精确识别的内置目录，无法识别时保持未知。
- `lumakit-egui::ChatPanel`：egui/eframe **0.37.0** 版，Markdown 用 egui_commonmark **0.26.0**；`install_icon_font` 在 `set_fonts` 前注册 Phosphor 图标字体。只服务 LumaDesk `main` 分支与 `examples/chat-host`。
- 两个面板沿用现有“用量”入口，弹层展示上下文估算和当前对话累计，上次回答明细默认折叠；全局按日期/模型/服务商统计由宿主设置页提供，独立 egui 示例在同一菜单内按需展开。金额区分配置与目录参考价，不进行币种换算。

egui 与 GPUI 两条线在同一 `Cargo.lock` 解析版本，所以 `gpui-kit/test-support`（无头窗口测试）目前无法启用；删除 egui 线后再引入。

## 验证

使用锁定的 Rust 1.99.0：

```bat
cargo +1.99.0 fmt --all --check
cargo +1.99.0 check --workspace --all-targets
cargo +1.99.0 test --workspace
cargo +1.99.0 run -p lumakit-cli
```

CLI 默认不发请求；设置 `LUMAKIT_AI_URL` 后追加真实流式请求，`LUMAKIT_AI_KEY` 只在进程内使用。

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

LumaKit 不提供桌面业务、不接入 Windows Shell（窗口层级、AppBar、托盘等系统适配先留在产品仓库，出现第二个消费方再提取）、不把 AI 权限交给模型。应用 Host 必须显式声明模型可用的工具、参数校验和审批策略。
