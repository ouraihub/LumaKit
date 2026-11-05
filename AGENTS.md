# 项目约束

LumaKit 是多个 Rust 桌面产品共用的基础库。当前消费方：LumaDesk（AI agent 工作台）、ThinkRail Native（独立模型选择器）。计划中的消费方：冰蓝复刻（`../rust-desktop-preview`，Windows 桌面伴侣），目前只有其验证工程 `spikes/gpui-surfaces` 使用 `lumakit-design`。

## 什么能进 Kit

- 进 Kit 的条件：已有两个消费方，或明显与产品无关（存储、日志、设计令牌、主题）。只有一个产品用、且带产品语义的代码留在产品仓库，出现第二个消费方再提取。
- 不为假想需求预建 trait 或 crate；一个 trait 至少服务两个真实实现或一个明确的测试替身。
- Kit 内不出现产品名、产品文案或产品数据格式；迁移某个产品的旧格式留在该产品。
- 删除优先于保留：没有消费方的公开 API 直接删，不留兼容别名。

- 开始编码前读取 `../LumaDesk/docs/AI工程规范.md`，共享库同样执行其中的模块规模检查与职责拆分要求：生产文件超过 300 行评审，超过 500 行优先拆分，交付前报告受影响文件规模与行为验证。不能通过压缩排版或搬成另一个大文件规避。

## 依赖方向

- `lumakit-support`、`lumakit-observability` 不依赖任何 UI 或 AI crate。
- `lumakit-design` 只依赖 gpui-kit 与通用库，不依赖 `lumakit-ai`；除读取主题与字体文件外不碰网络、数据库或设置存储，偏好持久化由宿主负责。
- `lumakit-ai` 不依赖任何 UI；`lumakit-gpui` / `lumakit-egui` 只是 AI 聊天面板，依赖 `lumakit-ai`。
- `lumakit-model-picker` 只依赖 gpui-kit，通过 `ActiveTheme` 读取宿主安装的主题；不依赖 `lumakit-ai`、`lumakit-design` 或任何产品 crate，不安装第二套全局主题。尺寸来自自身组件令牌；该边界专供不同设计系统的 GPUI 宿主共用选择器。
- 产品仓库依赖 Kit，Kit 不依赖产品。

## 界面

- 依赖 `lumakit-design` 的 GPUI 界面颜色只取 `LumaTheme::global(cx).roles`，间距、尺寸、圆角、字体样式只取 `lumakit_design::metrics`；不写十六进制颜色，主题调色板只在主题目录 JSON。独立 `lumakit-model-picker` 按上方边界读取 GPUI ActiveTheme，不引入 LumaTheme。
- 设计系统提炼自 ThinkRail Native `crates/ui`；新增组件先找 ThinkRail 的对应物，不自创布局。
- 修改共享组件必须运行每个受影响的消费宿主做实窗验证；没有实窗观察不能宣称 UI 验收完成。

## AI 聊天面板（`lumakit-gpui`、`lumakit-egui`）

用户明确要求：聊天 UI 以 AutoNote 实际使用的共享组件为首要参考，不允许自行另造布局。完整约束见 `../LumaDesk/AGENTS.md` 的「UI：参考优先，禁止擅自发挥」。

- 先读参考 `ChatPanel.svelte`、`MessageItem.svelte`、`ToolCallItem.svelte`、`ModelSelect.svelte`，再做最小适配；保留信息层级、控件归属、交互和宿主主题，不照搬许可不明的实现。
- 「快捷面板 / 自定义工具栏」是工具栏显示开关，不是提示词插入；固定状态须持久化，切换显示不执行工具、不改当前请求设置。未固定工具仍可通过添加菜单操作，不得用假开关占位未实现能力。
- 紧凑输入工具栏、可移除生图标签、内联图片、分层回答和更多菜单是约束，不得退回双下拉整行、图片打开按钮替代预览、逐轮用量和原始错误默认铺开的调试界面。
- 技术诊断默认折叠但必须可访问；失败事实、数据安全、无障碍和保存/取消语义不能为了简化而删除。
- 实质偏离参考须先获用户确认。共享问题在共享组件修复，不在每个宿主叠补丁。
- 检查成功/失败/生成中和窄窗；至少覆盖 320/375/414/768 逻辑宽度布局。
