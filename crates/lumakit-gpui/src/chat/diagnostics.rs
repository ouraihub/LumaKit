//! Failure status and opt-in technical diagnostics; never expose upstream details by default.
use super::*;

impl ChatPanel {
    pub(super) fn diagnostics(&self, attempt: &Attempt, cx: &Context<Self>) -> Div {
        let r = LumaTheme::global(cx).roles;
        let status = match attempt.status {
            TurnStatus::Running if attempt.text.is_empty() => "正在等待回复…",
            TurnStatus::Running => "正在生成…",
            TurnStatus::Cancelled => "已停止，保留部分回答",
            TurnStatus::Failed => "回答失败，请重试或更换模型",
            _ => "",
        };
        let mut body = col().gap(space::S4);
        if !status.is_empty() {
            body = body.child(
                div()
                    .text_style(text::METADATA)
                    .whitespace_normal()
                    .text_color(r.text_muted)
                    .child(status),
            );
        }
        let failed = attempt.status == TurnStatus::Failed;
        if attempt.error.is_none() && !(failed && attempt.trace_id.is_some()) {
            return body;
        }
        let key = format!("error:{}", attempt.id);
        body = body.child(self.disclosure(key.clone(), "诊断详情", cx));
        if !self.details.contains(&key) {
            return body;
        }
        let mut details = col()
            .id(SharedString::from(key))
            .gap(space::S4)
            .max_h(DETAIL_MAX)
            .overflow_y_scroll()
            .whitespace_normal()
            .text_style(text::METADATA);
        if let Some(trace_id) = &attempt.trace_id {
            let trace_id = trace_id.clone();
            details = details.child(
                row().child(
                    Button::new(SharedString::from(format!("trace:{}", attempt.id)))
                        .small()
                        .ghost()
                        .icon(IconName::Copy)
                        .label("复制诊断 ID")
                        .accessibility_label("复制完整诊断 TraceId")
                        .tooltip(format!("TraceId: {trace_id}"))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(trace_id.clone()));
                        }),
                ),
            );
        }
        if let Some(error) = &attempt.error {
            details = details.child(div().child(error.clone()));
        }
        body.child(details)
    }
}
