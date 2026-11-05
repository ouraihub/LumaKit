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
    /// 按模型 ID 推断类型（ThinkRail `model_catalog::model_type`）：模型目录只返回 ID 时的初值，
    /// 用户可在同步列表或模型设置中改正。关键词按序匹配，首个命中生效；都不命中为聊天。
    pub fn infer(id: &str) -> Self {
        const TYPES: &[(&[&str], ModelType)] = &[
            (&["embed"], ModelType::Embedding),
            (&["rerank"], ModelType::Rerank),
            (&["tts", "speech"], ModelType::Tts),
            (&["whisper", "stt", "transcri"], ModelType::Stt),
            (&["dall-e", "image", "flux", "imagen"], ModelType::Image),
            (&["video", "sora", "veo"], ModelType::Video),
        ];
        let id = id.to_ascii_lowercase();
        TYPES.iter().find(|(words, _)| words.iter().any(|word| id.contains(word))).map_or(Self::Chat, |&(_, kind)| kind)
    }
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
/// 模型支持的推理方言、强度与预算；不定义请求默认强度。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ReasoningConfig {
    /// 实际协议编码方言。
    pub dialect: ReasoningDialect,
    /// 可用强度；顺序不代表请求默认值，未显式选择时交由服务商决定。
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
    /// 独立输入 token 预算；未设置时仅受上下文窗口及输出预留约束。
    pub max_input: Option<u32>,
    /// 输出 token 上限；未设置时保留原有的4096上限。
    pub max_output: Option<u32>,
    /// 推理方言、可用强度与预算。
    pub reasoning: Option<ReasoningConfig>,
    /// 本次请求显式选择的推理强度；None 不发送推理参数，不写入模型元数据。
    #[serde(skip)]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// 显式关闭目录推理补全；已有显式推理配置仍优先。
    pub reasoning_disabled: bool,
    /// 用户对推理能力的声明；与本次请求的推理强度独立。
    pub reasoning_override: Option<bool>,
    /// 用户声明的音频输入能力；不表示宿主已接入音频上传。
    pub supports_audio: bool,
    /// 用户声明的视频输入能力；不表示宿主已接入视频上传。
    pub supports_video: bool,
    /// 启用服务商执行的原生搜索，仅限 Responses 或 Anthropic。
    pub native_web_search: bool,
    /// 每百万 token 价格；每个完成的请求均保留快照。
    pub pricing: Option<ModelPricing>,
    /// 当前有效价格的来源；运行时目录补全不会写回用户设置。
    #[serde(skip)]
    pub pricing_reference: bool,
    /// 显式 false 禁用宿主工具；未设置时沿用目录能力声明。
    pub supports_tools: Option<bool>,
    /// 是否请求 SSE；false 使用同一协议的非流式 JSON 响应。
    pub supports_streaming: bool,
    /// 模型是否接受视觉输入。
    pub supports_vision: bool,
    /// 显式视觉开关；None 保留旧 true 声明并允许精确目录补全。
    #[serde(default)]
    pub vision_override: Option<bool>,
    /// 图片模型是否接受参考图进行编辑。
    pub image_edit: bool,
}
impl Default for ModelSettings {
    fn default() -> Self { Self { name: None, group_name: None, model_type: ModelType::Chat, context_window: None, max_input: None, max_output: None, reasoning: None, reasoning_effort: None, reasoning_disabled: false, reasoning_override: None, supports_audio: false, supports_video: false, native_web_search: false, pricing: None, pricing_reference: false, supports_tools: None, supports_streaming: true, supports_vision: false, vision_override: None, image_edit: false } }
}
impl ModelSettings {
    /// 能力声明优先；声明支持不代表已经知道可发送的推理协议或档位。
    pub fn supports_reasoning(&self) -> bool {
        self.reasoning_override.unwrap_or(!self.reasoning_disabled && self.reasoning.is_some())
    }
    /// 返回显示名，未设置或空白时返回模型标识。
    pub fn display_name<'a>(&'a self, id: &'a str) -> &'a str { self.name.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(id) }
    /// 返回分组，未设置时取模型标识中 '-'、'/' 或 ':' 之前的前缀。
    pub fn group_name<'a>(&'a self, id: &'a str) -> &'a str { self.group_name.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or_else(|| id.split(['-', '/', ':']).next().unwrap_or(id)) }
    /// 返回实际输出上限，包含原有默认值4096。
    pub fn output_limit(&self) -> u32 { self.max_output.unwrap_or(4096) }
    /// 保存或构造请求前校验所有模型设置。
    pub fn validate(&self, protocol: Protocol) -> Result<(), String> {
        for name in [&self.name, &self.group_name].into_iter().flatten() { if name.trim().chars().count() > 80 { return Err("模型名称和分组最多80个字符".into()); } }
        if self.context_window == Some(0) || self.max_input == Some(0) || self.max_output == Some(0) { return Err("上下文、输入和输出上限必须大于零".into()); }
        if self.max_input.zip(self.context_window).is_some_and(|(input, context)| input > context) { return Err("最大输入不能超过上下文窗口".into()); }
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
        if self.reasoning_effort.is_some_and(|effort| !self.reasoning.as_ref().is_some_and(|config| config.efforts.contains(&effort))) { return Err("模型未声明该推理强度".into()); }
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
    /// 已包含于输出中的推理 token；未报告不等于零。
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
}
/// 一个已完成上游请求的用量、原始价格与货币金额。
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq)]
pub struct RoundUsage {
    /// 服务商未返回用量时为 None，不伪造零费用。
    pub usage: Option<TokenUsage>,
    /// 请求当时的价格，不受后续模型设置变更影响。
    pub pricing: Option<ModelPricing>,
    /// pricing.currency 币种下的 token 费用，不包含搜索等额外服务费。
    pub cost: Option<f64>,
    /// 上游请求的稳定标识；复制对话时保持不变。
    #[serde(default)]
    pub request_id: String,
    /// 请求开始的 Unix 毫秒；旧记录无法推知时为空。
    #[serde(default)]
    pub started_at_ms: Option<u64>,
    /// 完整请求耗时；旧记录为空。
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// 实际请求模型，旧记录可从回答候选补全。
    #[serde(default)]
    pub model: Option<crate::ModelRef>,
    /// 金额的来源；所有估算均不代表服务商账单。
    #[serde(default)]
    pub cost_source: CostSource,
    /// 缺少原始请求标识的历史导入；历史副本无法可靠去重。
    #[serde(default)]
    pub legacy: bool,
    /// 仅在实际请求前缀和开销一致时可复用的上下文锚点。
    #[serde(default)]
    pub context: Option<ContextAnchor>,
}
/// 上下文前缀和请求开销的指纹；不持久化提示词内容。
#[allow(missing_docs)]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextAnchor { pub prefix: String, pub overhead: String }
/// 保存的 token 金额来源，不将未知费用表示为零。
#[derive(Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CostSource {
    /// 已配置单价计算的 token 费用估算。
    Estimate,
    /// 模型目录参考单价计算的 token 费用估算。
    Reference,
    /// 没有已知来源或金额。
    #[default]
    Unknown,
}
impl RoundUsage {
    pub(crate) fn start(model: Option<crate::ModelRef>) -> Result<(Self, std::time::Instant), String> {
        let started_at_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?.as_millis().try_into().map_err(|_| "请求时间超出范围")?;
        Ok((Self { request_id: lumakit_support::new_id().map_err(|error| error.to_string())?, started_at_ms: Some(started_at_ms), duration_ms: None, model, cost_source: CostSource::Unknown, legacy: false, context: None, usage: None, pricing: None, cost: None }, std::time::Instant::now()))
    }
    pub(crate) fn finish(mut self, usage: Option<TokenUsage>, pricing: Option<ModelPricing>, reference: bool, started: std::time::Instant) -> Self {
        self.cost = pricing.as_ref().zip(usage.as_ref()).and_then(|(price, usage)| price.cost(usage));
        self.cost_source = if self.cost.is_none() { CostSource::Unknown } else if reference { CostSource::Reference } else { CostSource::Estimate };
        self.usage = usage;
        self.pricing = pricing;
        self.duration_ms = Some(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX));
        self
    }
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

#[cfg(test)]
mod tests {
    use super::ModelType;
    #[test]
    fn infer_classifies_catalog_ids_by_name() {
        for (id, kind) in [
            ("gpt-5.6-luna", ModelType::Chat), ("codex-auto-review", ModelType::Chat), ("Qwen/Qwen3-VL-235B", ModelType::Chat),
            ("gpt-image-2", ModelType::Image), ("gpt-image-2.5-flare", ModelType::Image), ("dall-e-3", ModelType::Image), ("black-forest-labs/FLUX.1-dev", ModelType::Image),
            ("text-embedding-3-large", ModelType::Embedding), ("BAAI/bge-m3", ModelType::Chat),
            ("BAAI/bge-reranker-v2-m3", ModelType::Rerank), ("whisper-1", ModelType::Stt), ("gpt-4o-transcribe", ModelType::Stt),
            ("gpt-4o-mini-tts", ModelType::Tts), ("sora-2", ModelType::Video),
        ] { assert_eq!(ModelType::infer(id), kind, "{id}"); }
    }
}
