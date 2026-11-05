//! Read-only usage presentation shared by the chat popover and host statistics page.
use super::*;
use lumakit_ai::{
    model_config::CostSource,
    usage::{ContextUsage, UsageTotals},
};
use lumakit_design::Roles;

fn known_tokens(total: &UsageTotals, tokens: u64) -> String {
    if total.requests > 0 && total.missing_usage == total.requests {
        "未知".into()
    } else if total.missing_usage > 0 {
        format!("{tokens}（已知部分）")
    } else {
        tokens.to_string()
    }
}

/// Render observed token subtotals and per-currency estimates without treating absent usage as zero.
/// The host owns fetching and refreshing the report; this component performs no storage access.
pub fn usage_totals(total: &UsageTotals, r: Roles) -> Div {
    let mut body = col()
        .gap(space::S4)
        .text_style(text::METADATA)
        .child(format!(
            "请求 {} · 缺少用量 {} · 无可用单价 {}",
            total.requests, total.missing_usage, total.missing_cost
        ))
        .child(format!(
            "输入 {} token（含缓存）",
            known_tokens(total, total.input_tokens)
        ))
        .child(format!(
            "输出 {} token",
            known_tokens(total, total.output_tokens)
        ))
        .child(format!(
            "缓存读取 {} · 缓存写入 {} token（输入子集）",
            known_tokens(total, total.cache_read_tokens),
            known_tokens(total, total.cache_write_tokens)
        ))
        .child(format!(
            "推理 {} token（已报告部分，输出子集，不另加总）",
            total
                .reasoning_tokens
                .map_or_else(|| "未知".into(), |tokens| tokens.to_string())
        ));
    if total.configured_costs.is_empty() {
        body = body.child("未配置完整用户单价，显示 token 消耗，不套用目录参考价");
    } else {
        for (currency, amount) in &total.configured_costs {
            body = body.child(format!("用户单价估算：{currency} {amount}"));
        }
    }
    body.child(
        label(
            text::METADATA,
            "金额仅来自请求时保存的用户配置单价，并非服务商账单；未配置时显示 token，不含搜索等额外服务费。",
        )
        .text_color(r.text_muted)
        .whitespace_normal(),
    )
}

pub(super) fn context_summary(context: &ContextUsage, r: Roles) -> Div {
    let window = context
        .context_window
        .map_or_else(|| "未知窗口".into(), |window| format!("{window} token"));
    let percentage = context
        .context_window
        .filter(|window| *window > 0)
        .map_or_else(String::new, |window| {
            format!(
                " · {:.1}%",
                context.tokens as f64 / f64::from(window) * 100.
            )
        });
    col().gap(space::S4)
        .child(label(text::TITLE_COMPACT, "提示词上下文占用估算"))
        .child(format!("{} / {window}{percentage}", context.tokens))
        .child(label(text::METADATA, if context.anchored {
            "以匹配请求的已报告输入为锚点，追加内容仍为估算。未预留输出空间，不代表精确剩余量。"
        } else {
            "按当前提示词估算，非服务商实测。未预留输出空间，不代表精确剩余量。"
        }).text_color(r.text_muted).whitespace_normal())
}

pub(super) fn usage_details(attempt: &Attempt) -> Div {
    let mut body = col().gap(space::S4).text_style(text::METADATA);
    if attempt.rounds.is_empty() {
        body = body.child("用量未提供 · 未配置用户单价");
    }
    for (index, round) in attempt.rounds.iter().enumerate() {
        let tokens = round.usage.as_ref().map_or_else(|| "用量未提供".into(), |usage| format!(
            "输入 {}（含缓存） · 输出 {} · 缓存读取 {} · 缓存写入 {} · 推理 {}（输出子集） token",
            usage.input_tokens, usage.output_tokens, usage.cache_read_tokens, usage.cache_write_tokens,
            usage.reasoning_tokens.map_or_else(|| "未知".into(), |tokens| tokens.to_string())));
        let cost = match (&round.pricing, round.cost, round.cost_source) {
            (Some(price), Some(cost), CostSource::Estimate)
                if cost.is_finite() && cost >= 0. && !price.currency.trim().is_empty() =>
            {
                format!("用户单价 token 费用估算：{} {cost}", price.currency)
            }
            _ => "未配置用户单价，不估算金额".into(),
        };
        body = body.child(
            label(
                text::METADATA,
                format!("第 {} 轮 · {tokens} · {cost}", index + 1),
            )
            .whitespace_normal(),
        );
    }
    body.child("按请求保存的用量与价格快照；不同币种不换算，估算不是账单且不含搜索等额外服务费。缓存读写已包含在输入中，推理已包含在输出中。")
}
