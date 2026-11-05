# Context usage status

状态：设计与实施合同，2026-11-05

## 目标

在 LumaDesk 与 ThinkRail Native 的聊天输入栏底部、发送按钮左侧显示一条紧凑状态：

- 当前上下文占用：`已用 / 上下文窗口` 与百分比；
- 可选累计 token 与费用；
- 当前请求/压缩状态；
- 按宿主系统显示 Windows、macOS 或 Linux 图标。

组件不得访问数据库、Agent、网络或操作系统服务。宿主只传入快照，组件只渲染并报告点击意图。

## 数据语义

状态必须区分三种数字，禁止互相替代：

1. **当前上下文占用**：当前提示词在模型上下文窗口中占用的 token。窗口未知时显示 `tokens/?`；占用未知时显示 `? / window`。本地估算必须标为“估算”。
2. **累计请求 token**：已完成请求的输入、输出、缓存读写和推理子集。缓存读写属于输入子集；推理 token 属于输出子集，不重复相加。
3. **累计费用**：只有存在请求时保存的价格快照和合法金额才显示；不同币种分开，不换算；缺失用量或价格显示“已知部分”或“未知”。费用不是服务商账单。

OMP 的对应事实来源：`get_state.contextUsage` 提供当前上下文 `{tokens, contextWindow, percent}`；`get_session_stats` 提供累计 `tokens` 与 `cost`。OMP 的 `inputTokens` 口径包含 `input + cacheRead + cacheWrite`，与 LumaKit `UsageTotals` 对齐。`contextUsage.percent` 为空时不伪造百分比。

LumaKit 已有：

- `ChatSession::context_usage`: 当前上下文估算，支持已报告输入锚点加本地尾部估算；不作为超窗准入保证；
- `ChatSession::usage_report`: 持久化请求账本，显式统计缺失用量/费用；
- `UsageTotals`: 输入、输出、缓存、推理子集与币种费用。

这次只增加适配快照和展示，不重写上述统计。

## LumaKit 公共合同

新增 `lumakit-usage-status` crate，依赖仅 `gpui-kit`，不依赖 `lumakit-ai`、`lumakit-design` 或产品仓库。

```rust
pub struct ContextStatus {
    pub used_tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub percent: Option<f32>,
    pub estimate: bool,
    pub cumulative_tokens: Option<u64>,
    pub cumulative_cost: Option<Money>,
    pub state: StatusState,
    pub auto_compact: bool,
    pub detail: String,
}

pub struct Money { pub currency: String, pub amount: f64, pub estimated: bool, pub partial: bool }
pub enum StatusState { Ready, Streaming, Compacting, Unknown }
```

`ContextStatusView` 是受控 GPUI 组件：

- 输入快照变化时更新，不在组件内计算 token；
- 点击状态条展开共享用量详情；宿主也可禁用内置弹层并处理 `OpenDetails`，不增加第二套账本；
- unknown/estimate 状态有可访问文本；
- 窄宽度优先保留上下文百分比和窗口，累计费用退化为 tooltip/详情；
- 状态条固定在输入工具栏中，发送按钮保持最右侧。

OS 图标由 `UsageStatusAssets<A>` 组合到宿主 AssetSource：Windows 使用四格透视标志，macOS 使用 Apple，Linux 使用 Linux/Tux 图标映射。Windows 为按参考绘制的组件图形；Apple/Linux 使用 Simple Icons CC0 资源，商标仍归原权利人。系统在编译目标选择，不查询外部服务；macOS/Linux 仅完成资源与接口设计，未声明平台运行验收。

## LumaDesk 接入

`ChatSession::context_usage` 提供当前上下文；`usage_report(Some(selected), None)` 提供当前对话累计。LumaDesk 在初始化、目录协调、动作执行或后台 session wake 后刷新 `ChatState.usage_status`，渲染期间不读数据库。发送按钮左侧插入共享 `ContextStatusView`，完整信息在共享详情与原有“用量”入口中查看；不复制统计逻辑。

## ThinkRail Native 接入

在 `thinkrail-core` 增加 `SessionUsage`/`UsageCost`（纯 serde 数据）及可选 `AgentSession::usage_snapshot()` 查询；默认返回 `None` 表示后端不支持，不改 `SessionSnapshot` 的持久化合同。Pi/OMP 使用真实 `get_state` 和 `get_session_stats`；已安装 pi 的当前上下文位于 stats 而不在 state，因此必须回退读取 stats.contextUsage。

输入原始桶为非缓存输入，归一化输入 = input + cacheRead + cacheWrite；引擎报告的累计 total 原样保存，不再次相加推理 token。OMP TUI 的 token_total 另有排除 cacheRead 的显示口径，本实现展示“累计请求 tokens（含缓存）”，不冒充该 TUI 指标。

`panels/chat/usage.rs` 对每个 chat 保存缓存；只在启动/恢复、结算、压缩、模型切换时通过宿主后台执行器查询。generation、模型完整身份和 session 指针过滤迟到结果，不从 stdout reader 发同步查询，不逐帧轮询。新请求使旧上下文占用失效，累计值仍保留。

Native 的投影模型有运行时默认窗口，展示回退只读取显式 `ProviderModel.context_window`，不把默认 128k 冒充已配置容量。零价格投影不构成真实价格，费用零值保持未知；正的引擎费用标已知部分。DSH 未提供 usage 时保持未知。UI 同样在输入栏底部、发送按钮左侧使用共享详情，不创建第二个账本。

## 颜色与状态

参考 OMP：

- 普通：主题默认/信息色；
- 约 50% 或达到 150k token：warning；
- 约 70% 或达到 270k token：thinking/high；
- 约 90% 或达到 500k token：error。

阈值只影响颜色，不改变数字。上下文窗口未知时只使用百分比（如果 Agent 报告），无法比较 token 阈值。
按用户追加的截图要求，摘要旁始终保留原版 `RiMagicLine` 魔杖（Remix Icon 4.9.0，原 SVG 路径、附带许可），并跟随上下文压力色；系统标志保持宿主前景色。魔杖在这里是上下文状态的视觉标记，不是可执行自动压缩按钮；详情另外显示自动压缩是否启用。`68.9% / 1.1M` 达到 500k token，因此是红色，而非只按百分比判断。
状态组之间按 OMP 无边框布局使用暗色 `·`，不使用竖线；圆点两侧额外各留 6px，魔杖与消费指标不拥挤。USD 使用 `$`，CNY 使用 `¥`，其他币种保留代码；这只是格式化，不做汇率换算。累计 token 与金额仍有独立明细。
消费指标规则：只有用户在中转模型配置中保存了有效单价，才按请求时保存的价格显示 `≈$金额`／`≈¥金额`；没有配置、价格无效、请求缺用量或涉及多币种时显示 token 消耗，不显示 `$—`，也不套用模型目录参考价。已保存的历史请求不按当前单价重算。`≈` 表示 token 乘用户配置单价的估算，不是服务商账单。

## 验收

1. OMP loopback/真实 Agent 报告的 context usage 与累计 token 不混算；
2. LumaDesk：Windows 实窗显示红框位置状态，unknown/estimate/已知费用三态可观察；
3. ThinkRail Native：Pi/OMP 真实 session settle 后显示快照，DSH/无 usage 显示未知；
4. Windows/macOS/Linux asset mapping 编译可用，Windows 实窗核对图标；
5. 320/375/414/768 宽度无发送按钮遮挡，图标和文本不纵向挤压；
6. 不引入第二套持久化账本、不把缓存 token 重复计入、不把估算写成服务商实测。

## 已实施与验证

- `lumakit-usage-status`：纯受控 GPUI 组件、数字格式/压力阈值、系统图标与详情，未依赖任何产品/网络/数据库。
- LumaDesk：`ChatState.usage_status` 在初始化、动作、目录协调、后台 wake 时更新；显示数据不在渲染帧内查询数据库。
- Native：真实 pi 与 OMP 在隔离配置下启动并发送到回环 HTTP，结算后取得 `1050` 累计 token、输入 `1000`（缓存读取 `200` 是输入子集）、输出 `50`；未伪造零费用。
- 高占用回环响应后 LumaDesk 实窗显示 `~68.9%/1.1M` 红色摘要和原版魔杖、`≈$1.52` 配置单价估算；Native 未配置价格时显示 token，未启动会话时显示 `? tokens`。
- Windows 实窗图标、详情点击、Esc、LumaDesk 320/375/414/768 客户区检查已执行；macOS/Linux 仅资源/编译映射，未实机验证。
- Native 实际拖窄聊天面板后，模型入口与“上下文＋费用＋发送”按组换行，费用和发送按钮仍可达；没有竖排标签。原有模型选择器图标未改动。
- 只保留现有请求账本和后台快照；未引入 Native 模型目录费用依赖或逐请求历史查询。
