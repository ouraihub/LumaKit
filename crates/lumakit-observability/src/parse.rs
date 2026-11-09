//! 解析 [`GrepFormat`](crate::GrepFormat) 写出的一行，供产品日志页筛选与显示；写和读用同一份契约。

/// 一行日志的结构化视图；`raw` 保留原文以便复制。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// 写入时的时间文本；无法解析的行为空。
    pub time: String,
    /// `ERROR` | `WARN` | `INFO` | `DEBUG` | `TRACE`；无法解析的行为 `INFO`。
    pub level: String,
    /// 链路标识；不在操作内的事件为 `None`。
    pub trace_id: Option<String>,
    /// 操作层级，如 `ui`、`service`、`external`。
    pub layer: Option<String>,
    /// 操作名（`ME` 段）。
    pub operation: Option<String>,
    /// 生命周期事件（`EV` 段），如 `ENTRY`、`EXIT`、`ERROR`。
    pub event: Option<String>,
    /// 耗时毫秒（`RT` 段）。
    pub duration_ms: Option<u64>,
    /// 事件来源模块；生命周期事件为 `None`。
    pub target: Option<String>,
    /// `Msg` 段（已反转义）。
    pub message: String,
    /// 原始整行。
    pub raw: String,
}

/// 按 grep 契约解析一行；不符合格式的行完整保留在 `message` 与 `raw`，不臆造字段。
pub fn parse_line(line: &str) -> LogLine {
    let mut parsed = LogLine {
        time: String::new(),
        level: "INFO".to_owned(),
        trace_id: None,
        layer: None,
        operation: None,
        event: None,
        duration_ms: None,
        target: None,
        message: line.to_owned(),
        raw: line.to_owned(),
    };
    let Some((time, rest)) = line.split_once(' ') else {
        return parsed;
    };
    let Some((level, body)) = rest.trim_start().split_once(' ') else {
        return parsed;
    };
    if !time.contains('T') || !matches!(level, "ERROR" | "WARN" | "INFO" | "DEBUG" | "TRACE") {
        return parsed;
    }
    parsed.time = time.to_owned();
    parsed.level = level.to_owned();
    parsed.message.clear();
    for segment in segments(body.trim_start()) {
        let Some((key, value)) = segment.split_once(':') else {
            continue;
        };
        let value = unescape(value);
        match key {
            "TraceId" => parsed.trace_id = Some(value),
            "Layer" => parsed.layer = Some(value),
            "ME" => parsed.operation = Some(value),
            "EV" => parsed.event = Some(value),
            "RT" => parsed.duration_ms = value.parse().ok(),
            "Target" => parsed.target = Some(value),
            "Msg" => parsed.message = value,
            _ => {}
        }
    }
    parsed
}

/// 按未转义的 `|` 切段。
fn segments(body: &str) -> impl Iterator<Item = &str> {
    let mut start = 0;
    let mut escaped = false;
    let mut cuts = Vec::new();
    for (index, byte) in body.bytes().enumerate() {
        match (escaped, byte) {
            (true, _) => escaped = false,
            (false, b'\\') => escaped = true,
            (false, b'|') => {
                cuts.push(&body[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    cuts.push(&body[start..]);
    cuts.into_iter()
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}
