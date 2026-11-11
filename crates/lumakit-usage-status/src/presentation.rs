use crate::localization::text as tr;
use crate::{ContextStatus, Money, StatusState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pressure {
    Normal,
    Warning,
    High,
    Critical,
}

pub(crate) fn percent(status: &ContextStatus) -> Option<f32> {
    status
        .percent
        .filter(|n| n.is_finite() && (0.0..=100.0).contains(n))
}

pub(crate) fn pressure(status: &ContextStatus) -> Pressure {
    let p = percent(status).unwrap_or(0.0);
    // Absolute thresholds only apply when the context capacity is known.
    let tokens = if status.context_window.is_some_and(|n| n > 0) {
        status.used_tokens.unwrap_or(0)
    } else {
        0
    };
    if p >= 90.0 || tokens >= 500_000 {
        Pressure::Critical
    } else if p >= 70.0 || tokens >= 270_000 {
        Pressure::High
    } else if p >= 50.0 || tokens >= 150_000 {
        Pressure::Warning
    } else {
        Pressure::Normal
    }
}

pub(crate) fn compact(n: u64) -> String {
    let (divisor, suffix) = if n >= 1_000_000_000 {
        (1_000_000_000, "B")
    } else if n >= 1_000_000 {
        (1_000_000, "M")
    } else if n >= 1_000 {
        (1_000, "k")
    } else {
        return n.to_string();
    };
    // Integer arithmetic avoids rounding 999,999 up into a misleading 1000k.
    let tenth = (u128::from(n) * 10 / divisor) as u64;
    if tenth.is_multiple_of(10) {
        format!("{}{suffix}", tenth / 10)
    } else {
        format!("{}.{:01}{suffix}", tenth / 10, tenth % 10)
    }
}

pub(crate) fn money(value: &Money) -> Option<String> {
    if !value.amount.is_finite() || value.amount < 0.0 || value.currency.trim().is_empty() {
        return None;
    }
    let currency = value.currency.trim();
    let amount = if value.amount > 0.0 && value.amount < 0.0001 {
        "<0.0001".to_owned()
    } else if value.amount < 0.01 && value.amount > 0.0 {
        format!("{:.4}", value.amount)
    } else {
        format!("{:.2}", value.amount)
    };
    let rendered = match currency {
        "USD" => format!("${amount}"),
        "CNY" => format!("¥{amount}"),
        _ => format!("{currency} {amount}"),
    };
    Some(format!(
        "{}{}{}",
        if value.estimated { "≈" } else { "" },
        rendered,
        if value.partial { "+" } else { "" }
    ))
}

pub(crate) fn summary(status: &ContextStatus) -> String {
    let p = percent(status).map_or_else(|| "?".into(), |n| format!("{n:.1}"));
    let window = status
        .context_window
        .filter(|n| *n > 0)
        .map_or_else(|| "?".into(), compact);
    format!("{}{p}%/{window}", if status.estimate { "~" } else { "" })
}

pub(crate) fn state_label(state: StatusState) -> std::borrow::Cow<'static, str> {
    tr(match state {
        StatusState::Ready => "就绪",
        StatusState::Streaming => "生成中",
        StatusState::Compacting => "压缩中",
        StatusState::Unknown => "状态未知",
    })
}

pub(crate) fn spending(status: &ContextStatus) -> String {
    status
        .cumulative_cost
        .as_ref()
        .and_then(money)
        .unwrap_or_else(|| {
            status.cumulative_tokens.map_or_else(
                || "? tokens".into(),
                |tokens| format!("{} tokens", compact(tokens)),
            )
        })
}

pub(crate) fn details(status: &ContextStatus) -> String {
    let used = status
        .used_tokens
        .map_or_else(|| "?".into(), |n| n.to_string());
    let window = status
        .context_window
        .filter(|n| *n > 0)
        .map_or_else(|| "?".into(), |n| n.to_string());
    let p = percent(status).map_or_else(|| "?".into(), |n| format!("{n:.2}%"));
    let total = status
        .cumulative_tokens
        .map_or_else(|| "?".into(), |n| n.to_string());
    let spending = spending(status);
    let mut text = format!(
        "{}: {used} / {window} tokens ({p})\n{}: {}\n{}: {total}\n{}: {spending}\n{}: {}\n{}: {}\n{}",
        tr("上下文"),
        tr("来源"),
        tr(if status.estimate {
            "估算"
        } else {
            "宿主报告；缺失值保持未知"
        }),
        tr("累计请求 tokens"),
        tr("显示"),
        tr("状态"),
        state_label(status.state),
        tr("自动压缩"),
        tr(if status.auto_compact {
            "已启用"
        } else {
            "未启用或未报告"
        }),
        tr(
            "? 表示未知，~ 表示上下文估算；有请求时保存的用户单价才显示金额，否则显示 token。金额不是服务商账单。"
        ),
    );
    if !status.detail.trim().is_empty() {
        text.push_str("\n\n");
        text.push_str(&status.detail);
    }
    text
}

#[cfg(test)]
#[path = "presentation_tests.rs"]
mod tests;
