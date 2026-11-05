//! Model/effort presentation rules; metadata is never inferred from a model name.
use super::*;

pub(super) fn levels(option: &ModelOption) -> Vec<ReasoningEffort> {
    use ReasoningEffort::*;
    let Some(config) = &option.settings.reasoning else {
        return Vec::new();
    };
    [None, Minimal, Low, Medium, High, Xhigh, Max]
        .into_iter()
        .filter(|level| config.efforts.contains(level))
        .collect()
}
pub(super) fn effective(
    option: &ModelOption,
    explicit: Option<ReasoningEffort>,
) -> Option<ReasoningEffort> {
    let config = option.settings.reasoning.as_ref()?;
    explicit.filter(|level| config.efforts.contains(level))
}
pub(super) fn word(level: ReasoningEffort) -> &'static str {
    use ReasoningEffort::*;
    match level {
        None => "off",
        Auto => "auto",
        Minimal => "minimal",
        Low => "low",
        Medium => "medium",
        High => "high",
        Xhigh => "xhigh",
        Max => "max",
    }
}
pub(super) fn hint(level: ReasoningEffort) -> &'static str {
    use ReasoningEffort::*;
    match level {
        None => "无推理 — 最快·最省",
        Auto => "由服务商自动决定推理深度",
        Minimal => "最简推理",
        Low => "轻量推理 — 最快",
        Medium => "均衡推理",
        High => "深度推理",
        Xhigh => "超深度推理",
        Max => "最高推理",
    }
}
pub(super) fn glyph(level: Option<ReasoningEffort>) -> IconName {
    match level {
        Some(ReasoningEffort::None) => IconName::Moon,
        Some(ReasoningEffort::Minimal) => IconName::Leaf,
        Some(ReasoningEffort::Low) => IconName::Zap,
        Some(ReasoningEffort::Medium) => IconName::Circle,
        Some(ReasoningEffort::High) => IconName::Sparkles,
        Some(ReasoningEffort::Xhigh) => IconName::Sparkle,
        Some(ReasoningEffort::Max) => IconName::Flame,
        Some(ReasoningEffort::Auto) | None => IconName::Brain,
    }
}
pub(super) fn color(level: ReasoningEffort, r: &lumakit_design::Roles) -> Rgba {
    use ReasoningEffort::*;
    match level {
        None | Minimal | Low => r.feedback_info,
        Xhigh | Max => r.feedback_warning,
        Auto | Medium | High => r.primary,
    }
}
pub(super) fn typed_level(query: &str, option: &ModelOption) -> Option<ReasoningEffort> {
    let (base, tail) = query.trim().rsplit_once(char::is_whitespace)?;
    if base.trim().is_empty() {
        return None;
    }
    option
        .settings
        .reasoning
        .as_ref()?
        .efforts
        .iter()
        .copied()
        .find(|level| word(*level).eq_ignore_ascii_case(tail))
}
pub(super) fn score(query: &str, option: &ModelOption) -> Option<usize> {
    let query = if typed_level(query, option).is_some() {
        query.trim().rsplit_once(char::is_whitespace)?.0
    } else {
        query
    };
    let text = format!(
        "{} {} {} {}",
        option.name, option.model.model, option.provider, option.endpoint
    )
    .to_lowercase();
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    if let Some(index) = text.find(&query) {
        return Some(index);
    }
    let mut chars = text.char_indices();
    let mut total = 1000;
    for wanted in query.chars().filter(|c| !c.is_whitespace()) {
        total += chars.find(|(_, c)| *c == wanted)?.0;
    }
    Some(total)
}
pub(super) fn detail(option: &ModelOption) -> String {
    let mut parts = vec![option.endpoint.clone()];
    if option.has_key {
        parts.push("API 密钥".into());
    }
    if let Some(price) = &option.settings.pricing {
        if let Some(tier) = price.tiers.first() {
            parts.push(format!(
                "{} {} / {} 每百万 token 起",
                price.currency, tier.input, tier.output
            ));
        }
    }
    if let Some(context) = option.settings.context_window {
        parts.push(if context >= 1_000_000 {
            format!("{:.1}M 上下文", context as f64 / 1_000_000.)
        } else if context >= 1000 {
            format!("{}K 上下文", context / 1000)
        } else {
            format!("{context} 上下文")
        });
    }
    parts.join(" · ")
}
