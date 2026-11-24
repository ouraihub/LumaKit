//! 已完成请求的 token 用量、价格快照与金额来源；与模型设置分开维护。
use super::ModelPricing;
use serde::{Deserialize, Serialize};
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
pub struct ContextAnchor {
    pub prefix: String,
    pub overhead: String,
}
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
    pub(crate) fn start(
        model: Option<crate::ModelRef>,
    ) -> Result<(Self, std::time::Instant), String> {
        let started_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis()
            .try_into()
            .map_err(|_| "请求时间超出范围")?;
        Ok((
            Self {
                request_id: lumakit_support::new_id().map_err(|error| error.to_string())?,
                started_at_ms: Some(started_at_ms),
                duration_ms: None,
                model,
                cost_source: CostSource::Unknown,
                legacy: false,
                context: None,
                usage: None,
                pricing: None,
                cost: None,
            },
            std::time::Instant::now(),
        ))
    }
    pub(crate) fn finish(
        mut self,
        usage: Option<TokenUsage>,
        pricing: Option<ModelPricing>,
        reference: bool,
        started: std::time::Instant,
    ) -> Self {
        self.cost = pricing
            .as_ref()
            .zip(usage.as_ref())
            .and_then(|(price, usage)| price.cost(usage));
        self.cost_source = if self.cost.is_none() {
            CostSource::Unknown
        } else if reference {
            CostSource::Reference
        } else {
            CostSource::Estimate
        };
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
        let t = self
            .tiers
            .iter()
            .rev()
            .find(|t| u64::from(t.min_input_tokens) <= usage.input_tokens)?;
        let uncached = usage.input_tokens.checked_sub(
            usage
                .cache_read_tokens
                .checked_add(usage.cache_write_tokens)?,
        )?;
        Some(
            (uncached as f64 * t.input
                + usage.output_tokens as f64 * t.output
                + usage.cache_read_tokens as f64 * t.cache_read.unwrap_or(t.input)
                + usage.cache_write_tokens as f64 * t.cache_write.unwrap_or(t.input))
                / 1_000_000.0,
        )
    }
}
