//! 已保存的模型元数据、请求限制、推理方言及每百万 token 价格。
use crate::Protocol;
use serde::{Deserialize, Serialize};

/// 模型实际接口分类；非聊天类型不会发送到文本聊天接口。
#[derive(Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModelType {
    /// 文本聊天。
    #[default]
    Chat,
    /// 图片生成或编辑。
    Image,
    /// 视频生成。
    Video,
    /// 文字转语音。
    Tts,
    /// 语音转文字。
    Stt,
    /// 向量嵌入。
    Embedding,
    /// 文档重排。
    Rerank,
    /// 未分类接口。
    Unknown,
}
impl ModelType {
    /// 模型选择器使用的分类名称。
    pub fn label(self) -> &'static str { match self { Self::Chat => "聊天", Self::Image => "图片", Self::Video => "视频", Self::Tts => "语音合成", Self::Stt => "语音识别", Self::Embedding => "向量", Self::Rerank => "重排", Self::Unknown => "未知" } }
}
/// 推理控制的协议编码方言。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReasoningDialect {
    /// OpenAI 的 reasoning_effort 或 reasoning.effort 字段。
    OpenAi,
    /// Anthropic 显式 token 预算。
    AnthropicBudget,
    /// Anthropic 自适应思考及 output_config 字段。
    AnthropicAdaptive,
    /// OpenAI 兼容接口的 thinking.type 开关。
    ThinkingType,
    /// Qwen 的 enable_thinking 与 thinking_budget 字段。
    Qwen,
}
/// 显式推理强度；序列化为小写协议值。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    /// 关闭推理。
    None,
    /// 由开关式方言的服务商决定推理深度。
    Auto,
    /// 最小深度。
    Minimal,
    /// 低深度。
    Low,
    /// 中等深度。
    Medium,
    /// 高深度。
    High,
    /// 超高深度。
    Xhigh,
    /// 最大深度。
    Max,
}
/// 按偏好排列的可用强度；第一项为默认请求强度。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ReasoningConfig {
    /// 实际协议编码方言。
    pub dialect: ReasoningDialect,
    /// 可用强度；未在连接中显式选择时使用第一项。
    pub efforts: Vec<ReasoningEffort>,
    /// 推理预算上限，发送时按强度缩放。
    pub budget_tokens: Option<u32>,
}
/// 按完整输入 token 数选择的价格档位。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PriceTier {
    /// 包含此值的输入门槛；第一档必须从零开始。
    pub min_input_tokens: u32,
    /// 每百万未缓存输入 token 的价格。
    pub input: f64,
    /// 每百万输出 token 的价格。
    pub output: f64,
    /// 每百万缓存读取 token 的价格；未设置时使用输入价。
    pub cache_read: Option<f64>,
    /// 每百万缓存写入 token 的价格；未设置时使用输入价。
    pub cache_write: Option<f64>,
}
/// 每个已完成请求保留的不可变价格快照。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ModelPricing {
    /// USD 或 CNY；不进行货币兑换。
    pub currency: String,
    /// 一至八档，按输入门槛严格递增。
    pub tiers: Vec<PriceTier>,
}
/// 模型选择器和实际请求编码共同消费的已保存设置。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModelSettings {
    /// 可选显示名，不替代请求中的模型标识。
    pub name: Option<String>,
    /// 可选分组；未设置时使用模型标识前缀。
    pub group_name: Option<String>,
    /// 实际接口分类。
    pub model_type: ModelType,
    /// 上下文 token 容量；运行时使用保守估算，而非精确分词。
    pub context_window: Option<u32>,
    /// 输出 token 上限；未设置时保留原有的4096上限。
    pub max_output: Option<u32>,
    /// 推理方言、可用强度与预算。
    pub reasoning: Option<ReasoningConfig>,
    /// 启用服务商执行的原生搜索，仅限 Responses 或 Anthropic。
    pub native_web_search: bool,
    /// 每百万 token 价格；每个完成的请求均保留快照。
    pub pricing: Option<ModelPricing>,
    /// 显式 false 禁用宿主工具；未设置时沿用目录能力声明。
    pub supports_tools: Option<bool>,
    /// 是否请求 SSE；false 使用同一协议的非流式 JSON 响应。
    pub supports_streaming: bool,
    /// 模型是否接受视觉输入。
    pub supports_vision: bool,
    /// 图片模型是否接受参考图进行编辑。
    pub image_edit: bool,
}
impl Default for ModelSettings {
    fn default() -> Self { Self { name: None, group_name: None, model_type: ModelType::Chat, context_window: None, max_output: None, reasoning: None, native_web_search: false, pricing: None, supports_tools: None, supports_streaming: true, supports_vision: false, image_edit: false } }
}
impl ModelSettings {
    /// 返回显示名，未设置或空白时返回模型标识。
    pub fn display_name<'a>(&'a self, id: &'a str) -> &'a str { self.name.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(id) }
    /// 返回分组，未设置时取模型标识中 '-'、'/' 或 ':' 之前的前缀。
    pub fn group_name<'a>(&'a self, id: &'a str) -> &'a str { self.group_name.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or_else(|| id.split(['-', '/', ':']).next().unwrap_or(id)) }
    /// 返回实际输出上限，包含原有默认值4096。
    pub fn output_limit(&self) -> u32 { self.max_output.unwrap_or(4096) }
    /// 保存或构造请求前校验所有模型设置。
    pub fn validate(&self, protocol: Protocol) -> Result<(), String> {
        for name in [&self.name, &self.group_name].into_iter().flatten() { if name.trim().chars().count() > 80 { return Err("模型名称和分组最多80个字符".into()); } }
        if self.context_window == Some(0) || self.max_output == Some(0) { return Err("上下文和输出上限必须大于零".into()); }
        let chat = self.model_type == ModelType::Chat;
        if chat && self.context_window.is_some_and(|n| self.output_limit() >= n) { return Err("输出上限（默认4096）必须小于上下文窗口".into()); }
        if !chat && (self.reasoning.is_some() || self.native_web_search || self.pricing.is_some() || self.supports_tools.is_some()) { return Err("推理、搜索、工具和聊天计费只适用于聊天模型".into()); }
        if self.native_web_search && protocol == Protocol::OpenAiChat { return Err("Chat Completions不支持原生搜索，请使用Responses或Anthropic".into()); }
        if let Some(r) = &self.reasoning {
            use ReasoningDialect::*;
            let fits = match r.dialect { OpenAi => protocol != Protocol::AnthropicMessages, AnthropicBudget | AnthropicAdaptive => protocol == Protocol::AnthropicMessages, ThinkingType | Qwen => protocol == Protocol::OpenAiChat };
            if !fits { return Err("推理方言与线路协议不匹配".into()); }
            if r.efforts.is_empty() || r.efforts.iter().enumerate().any(|(i,e)| r.efforts[..i].contains(e) || (*e == ReasoningEffort::Auto && !matches!(r.dialect, ThinkingType | Qwen))) { return Err("推理强度不能为空、重复或含不支持的auto".into()); }
            match r.dialect {
                AnthropicBudget if !r.budget_tokens.is_some_and(|n| n >= 1024 && n < self.output_limit()) => return Err("Anthropic推理预算必须至少1024且小于输出上限".into()),
                Qwen if r.budget_tokens == Some(0) => return Err("Qwen推理预算必须大于零".into()),
                OpenAi | AnthropicAdaptive | ThinkingType if r.budget_tokens.is_some() => return Err("此推理方言不接受token预算".into()),
                _ => {}
            }
        }
        if let Some(p) = &self.pricing {
            let valid = |n:f64| n.is_finite() && (0.0..=1_000_000.0).contains(&n);
            if !matches!(p.currency.as_str(), "USD" | "CNY") || p.tiers.is_empty() || p.tiers.len() > 8 || p.tiers[0].min_input_tokens != 0 || p.tiers.iter().enumerate().any(|(i,t)| (i > 0 && t.min_input_tokens <= p.tiers[i-1].min_input_tokens) || !valid(t.input) || !valid(t.output) || t.cache_read.is_some_and(|n| !valid(n)) || t.cache_write.is_some_and(|n| !valid(n))) { return Err("价格须为USD/CNY、0起始升序的1至8档，单价须为0至1000000的有限数值".into()); }
        }
        if self.image_edit && (!self.supports_vision || self.model_type != ModelType::Image) { return Err("图像编辑模型必须声明视觉输入".into()); }
        Ok(())
    }
}
/// 服务商返回的真实 token 计数，不从文本长度推算。
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
    /// 完整输入计数，包含缓存读取和写入。
    pub input_tokens: u64,
    /// 生成的输出 token 数。
    pub output_tokens: u64,
    /// 从缓存读取的输入 token 数。
    pub cache_read_tokens: u64,
    /// 写入缓存的输入 token 数。
    pub cache_write_tokens: u64,
}
/// 一个已完成上游请求的用量、原始价格与货币金额。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RoundUsage {
    /// 服务商未返回用量时为 None，不伪造零费用。
    pub usage: Option<TokenUsage>,
    /// 请求当时的价格，不受后续模型设置变更影响。
    pub pricing: Option<ModelPricing>,
    /// pricing.currency 币种下的 token 费用，不包含搜索等额外服务费。
    pub cost: Option<f64>,
}
impl ModelPricing {
    /// 计算 token 费用，缓存输入不会重复计费。
    ///
    /// 无适用价格档、缓存数溢出或超过完整输入数时返回 None。
    pub fn cost(&self, usage: &TokenUsage) -> Option<f64> {
        let t = self.tiers.iter().rev().find(|t| u64::from(t.min_input_tokens) <= usage.input_tokens)?;
        let uncached = usage.input_tokens.checked_sub(usage.cache_read_tokens.checked_add(usage.cache_write_tokens)?)?;
        Some((uncached as f64 * t.input + usage.output_tokens as f64 * t.output + usage.cache_read_tokens as f64 * t.cache_read.unwrap_or(t.input) + usage.cache_write_tokens as f64 * t.cache_write.unwrap_or(t.input)) / 1_000_000.0)
    }
}
