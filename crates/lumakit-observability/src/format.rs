//! grep 友好的单行文本格式，字段契约：
//! `TraceId:<id>|SpanId:<n>|ParentSpanId:<n|->|Layer:<layer>|ME:<operation>|EV:<event>|RT:<ms>|Result:<r>|Error:<code>`。
//! 不在操作内的事件没有 TraceId 段，以 `Target:` 开头。
//!
//! [`LogHandle::init`](crate::LogHandle::init) 已安装这两层；自带 appender 的宿主（例如按日滚动）
//! 用 `registry().with(SpanFields).with(fmt::layer().event_format(GrepFormat::new(timer)))` 组合。

use std::fmt::{self, Write as _};

use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};
use tracing_subscriber::{
    fmt::{
        FmtContext,
        format::{FormatEvent, FormatFields, Writer},
        time::{FormatTime, SystemTime},
    },
    layer::{Context, Layer},
    registry::{LookupSpan, SpanRef},
};

use crate::operation::{SPAN, TARGET};

/// span 创建或补记字段时的值快照；格式器只能从扩展读取 span 字段。
struct SpanValues(Vec<(&'static str, String)>);

impl SpanValues {
    fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find_map(|(name, value)| (*name == key).then_some(value.as_str()))
    }
}

struct Collect<'a>(&'a mut Vec<(&'static str, String)>);

impl Collect<'_> {
    fn set(&mut self, key: &'static str, value: String) {
        match self.0.iter_mut().find(|(name, _)| *name == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key, value)),
        }
    }
}

impl Visit for Collect<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.set(field.name(), value.to_owned());
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.set(field.name(), format!("{value:?}"));
    }
}

/// 把每个 span 的字段保存到扩展，供 [`GrepFormat`] 输出 TraceId、Layer、ME 与业务 span 字段。
///
/// 必须与 [`GrepFormat`] 安装在同一个 subscriber 上，否则日志行缺少 TraceId 段。
pub struct SpanFields;

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for SpanFields {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut values = Vec::new();
        attrs.record(&mut Collect(&mut values));
        span.extensions_mut().insert(SpanValues(values));
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        if let Some(SpanValues(fields)) = span.extensions_mut().get_mut::<SpanValues>() {
            values.record(&mut Collect(fields));
        }
    }
}

/// 写出转义后的值：`|` → `\|`，换行 → `\n`，保证一条事件只占一行且段边界可切分。
struct Escaped<'a, 'b>(&'a mut Writer<'b>);

impl fmt::Write for Escaped<'_, '_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for ch in s.chars() {
            match ch {
                '|' => self.0.write_str("\\|")?,
                '\n' => self.0.write_str("\\n")?,
                '\r' => self.0.write_str("\\r")?,
                ch => self.0.write_char(ch)?,
            }
        }
        Ok(())
    }
}

/// 输出 `key:value` 段；首段前不加分隔符。
struct Line<'a, 'b> {
    writer: &'a mut Writer<'b>,
    first: bool,
}

impl Line<'_, '_> {
    fn key(&mut self, key: &str) -> fmt::Result {
        if !self.first {
            self.writer.write_char('|')?;
        }
        self.first = false;
        write!(self.writer, "{key}:")
    }

    fn put(&mut self, key: &str, value: impl fmt::Display) -> fmt::Result {
        self.key(key)?;
        write!(Escaped(self.writer), "{value}")
    }
}

struct EventFields<'a, 'b, 'c> {
    line: &'a mut Line<'b, 'c>,
    /// 只有生命周期事件的字段映射为 `EV`/`RT` 等保留段名；普通事件原样输出字段名，
    /// 避免业务字段 `event=…` 伪装成生命周期并污染 `grep "|EV:ERROR|"`。
    lifecycle: bool,
    message: Option<String>,
    result: fmt::Result,
}

impl EventFields<'_, '_, '_> {
    fn label<'n>(&self, name: &'n str) -> &'n str {
        if !self.lifecycle {
            return name;
        }
        match name {
            "event" => "EV",
            "duration_ms" => "RT",
            "result" => "Result",
            "error_code" => "Error",
            "reason" => "Reason",
            "limit_ms" => "Limit",
            name => name,
        }
    }
}

impl Visit for EventFields<'_, '_, '_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_owned());
        } else if self.result.is_ok() {
            let label = self.label(field.name());
            self.result = self.line.put(label, value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{value:?}"));
        } else if self.result.is_ok() {
            let label = self.label(field.name());
            self.result = self
                .line
                .key(label)
                .and_then(|()| write!(Escaped(self.line.writer), "{value:?}"));
        }
    }
}

fn is_operation<S: for<'a> LookupSpan<'a>>(span: &SpanRef<'_, S>) -> bool {
    span.metadata().target() == TARGET && span.name() == SPAN
}

/// 文件日志的事件格式：`<时间> <级别> <段>|<段>…`；时间格式由 `T` 决定，默认 UTC RFC 3339。
pub struct GrepFormat<T = SystemTime> {
    timer: T,
}

impl GrepFormat {
    /// 使用 UTC RFC 3339 时间。
    pub fn utc() -> Self {
        Self { timer: SystemTime }
    }
}

impl<T: FormatTime> GrepFormat<T> {
    /// 使用宿主提供的时间格式；时间段不得包含空格，否则解析会把它与级别混淆。
    pub fn new(timer: T) -> Self {
        Self { timer }
    }
}

impl<S, N, T> FormatEvent<S, N> for GrepFormat<T>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
    T: FormatTime,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        self.timer.format_time(&mut writer)?;
        write!(writer, " {:<5} ", event.metadata().level())?;
        let mut line = Line {
            writer: &mut writer,
            first: true,
        };

        // 最近的操作 span 决定 TraceId/SpanId/Layer/ME；父操作只认同一 TraceId，避免误挂到别的链路。
        let mut current = None;
        let mut parent = None;
        for span in ctx.event_scope().into_iter().flatten() {
            if !is_operation(&span) {
                continue;
            }
            let extensions = span.extensions();
            let Some(values) = extensions.get::<SpanValues>() else {
                continue;
            };
            let trace_id = values.get("trace_id").unwrap_or_default();
            match &current {
                None => {
                    current = Some((
                        span.id().into_u64(),
                        trace_id.to_owned(),
                        values.get("layer").unwrap_or_default().to_owned(),
                        values.get("operation").unwrap_or_default().to_owned(),
                        values.get("source_file").unwrap_or_default().to_owned(),
                        values.get("source_line").unwrap_or_default().to_owned(),
                    ));
                }
                Some((_, current_trace, _, _, _, _)) if current_trace == trace_id => {
                    parent = Some(span.id().into_u64());
                    break;
                }
                Some(_) => break,
            }
        }
        if let Some((span_id, trace_id, layer, operation, source_file, source_line)) = &current {
            line.put("TraceId", trace_id)?;
            line.put("SpanId", span_id)?;
            match parent {
                Some(parent) => line.put("ParentSpanId", parent)?,
                None => line.put("ParentSpanId", "-")?,
            }
            line.put("Layer", layer)?;
            line.put("ME", operation)?;
            line.put("SourceFile", source_file)?;
            line.put("SourceLine", source_line)?;
        }
        if event.metadata().target() != TARGET {
            line.put("Target", event.metadata().target())?;
        }
        // 业务 span（非操作 span）的字段按外到内附加，保留宿主追加的 conversation_id 等上下文。
        if let Some(scope) = ctx.event_scope() {
            for span in scope.from_root().filter(|span| !is_operation(span)) {
                if let Some(values) = span.extensions().get::<SpanValues>() {
                    for (key, value) in &values.0 {
                        line.put(key, value)?;
                    }
                }
            }
        }
        let mut fields = EventFields {
            lifecycle: event.metadata().target() == TARGET,
            line: &mut line,
            message: None,
            result: Ok(()),
        };
        event.record(&mut fields);
        fields.result?;
        if let Some(message) = fields.message {
            line.put("Msg", message)?;
        }
        writeln!(writer)
    }
}
