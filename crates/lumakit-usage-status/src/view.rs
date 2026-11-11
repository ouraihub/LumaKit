use crate::localization::text as tr;
use crate::{ContextStatus, Platform, UsageStatusEvent, assets, presentation};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    popover::Popover,
};
use gpui_kit::*;

/// A compact, keyboard-operable status button with optional built-in details.
/// Size its parent with `min_w_0`/`flex_1` before a nonshrinking send button.
/// The summary remains one line; optional cumulative facts truncate first.
/// Full exact facts remain available in its tooltip, accessible label and popup.
/// The host must install [`crate::UsageStatusAssets`] and GPUI component globals.
pub struct ContextStatusView {
    status: ContextStatus,
    builtin_details: bool,
    details_open: bool,
}
impl ContextStatusView {
    /// Creates an unknown snapshot, using the host's existing active theme.
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            status: ContextStatus::default(),
            builtin_details: true,
            details_open: false,
        }
    }

    /// Replaces the snapshot and redraws only when it changes.
    pub fn set_status(&mut self, status: ContextStatus, cx: &mut Context<Self>) {
        if self.status == status {
            return;
        }
        self.status = status;
        cx.notify();
    }

    /// Disables the built-in popup when a host handles `OpenDetails` itself.
    /// Events are emitted with either setting; the default is `true`.
    pub fn set_builtin_details(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.builtin_details == enabled {
            return;
        }
        self.builtin_details = enabled;
        self.details_open = false;
        cx.notify();
    }
}
impl EventEmitter<UsageStatusEvent> for ContextStatusView {}

impl Render for ContextStatusView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let platform = Platform::current();
        let detail = presentation::details(&self.status);
        let label = format!(
            "{} {}. {}. {}",
            platform.label(),
            tr("上下文用量"),
            detail,
            tr("查看用量详情")
        );
        let color = match presentation::pressure(&self.status) {
            presentation::Pressure::Normal => colors.muted_foreground,
            presentation::Pressure::Warning => colors.warning,
            presentation::Pressure::High => colors.magenta,
            presentation::Pressure::Critical => colors.danger,
        };
        let mut row = div()
            .flex()
            .items_center()
            .min_w_0()
            .gap(px(5.))
            .text_size(px(12.))
            .text_color(color);
        if let Some(icon) = assets::platform_icon(platform) {
            row = row.child(icon.xsmall().text_color(colors.foreground));
        }
        row = row.child(
            div()
                .flex_shrink_0()
                .whitespace_nowrap()
                .text_color(color)
                .font_family(cx.theme().mono_font_family.clone())
                .child(presentation::summary(&self.status)),
        );
        row = row.child(assets::icon("wand").xsmall().text_color(color));
        let spending = presentation::spending(&self.status);
        row = row
            .child(
                div()
                    .flex_shrink_0()
                    .mx(px(6.))
                    .text_color(colors.border)
                    .child("·"),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(colors.muted_foreground)
                    .child(spending),
            );
        if matches!(
            self.status.state,
            crate::StatusState::Streaming | crate::StatusState::Compacting
        ) {
            row = row.child(
                div()
                    .min_w_0()
                    .truncate()
                    .child(presentation::state_label(self.status.state)),
            );
        }
        let button = Button::new("context-usage-status")
            .ghost()
            .small()
            .min_w_0()
            .max_w_full()
            .px(px(3.))
            .accessibility_label(label)
            .tooltip(detail)
            .child(row);
        if !self.builtin_details {
            return div()
                .min_w_0()
                .overflow_hidden()
                .child(
                    button.on_click(
                        cx.listener(|_, _, _, cx| cx.emit(UsageStatusEvent::OpenDetails)),
                    ),
                )
                .into_any_element();
        }
        let weak = cx.entity().downgrade();
        let content_view = weak.clone();
        div()
            .min_w_0()
            .max_w_full()
            .child(
                Popover::new("context-usage-details")
                    .anchor(Anchor::BottomLeft)
                    .trigger_style(StyleRefinement::default().min_w_0().max_w_full())
                    .open(self.details_open)
                    .on_open_change(move |open, _, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.details_open = *open;
                            if *open {
                                cx.emit(UsageStatusEvent::OpenDetails);
                            }
                            cx.notify();
                        });
                    })
                    .trigger(button)
                    .content(move |_, window, cx| {
                        let text = content_view
                            .upgrade()
                            .map(|view| presentation::details(&view.read(cx).status))
                            .unwrap_or_default();
                        let width = (f32::from(window.viewport_size().width) - 24.).clamp(1., 420.);
                        div()
                            .w(px(width))
                            .max_h(px((f32::from(window.viewport_size().height) - 40.).max(40.)))
                            .id("context-usage-details-content")
                            .overflow_y_scroll()
                            .p(px(12.))
                            .text_size(px(12.))
                            .child(text)
                    }),
            )
            .into_any_element()
    }
}
