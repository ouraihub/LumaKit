//! Model rows, action footer and bottom trigger; grouped surface layout lives in view.
use super::view::{col, row};
use crate::localization::text as tr;
use crate::{Model, Picker, PickerEvent, effort, icons::icon};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    popover::Popover,
};
use gpui_kit::{prelude::FluentBuilder as _, *};

impl Picker {
    pub(super) fn model_row(
        &self,
        model: &Model,
        query: &str,
        highlighted: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors;
        let selected = self
            .data
            .selected
            .as_ref()
            .is_some_and(|s| s.model == model.id);
        let next = effort::selection(model, self.data.selected.as_ref(), query);
        let mut details = row().gap(px(4.));
        if model.has_key {
            details = details.child(icon("RiKey2Line").xsmall());
        }
        details = details.child(div().min_w_0().truncate().child(model.detail.clone()));
        Button::new(SharedString::from(format!("model:{}", model.id)))
            .ghost()
            .w_full()
            .h_auto()
            .py(px(4.))
            .px(px(8.))
            .selected(selected || highlighted)
            .disabled(!self.data.enabled)
            .accessibility_label(format!(
                "{} {} · {}",
                tr("选择模型"),
                model.name,
                model.provider
            ))
            .child(
                row()
                    .w_full()
                    .min_w_0()
                    .gap(px(8.))
                    .child(
                        col()
                            .flex_1()
                            .min_w_0()
                            .items_start()
                            .child(div().truncate().child(model.name.clone()))
                            .child(details.text_xs().text_color(colors.muted_foreground)),
                    )
                    .when(selected, |r| {
                        r.child(
                            Icon::new(IconName::Check)
                                .small()
                                .text_color(colors.primary),
                        )
                    }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select(next.model.clone(), next.effort.clone(), cx)
            }))
            .into_any_element()
    }

    pub(super) fn footer(&self, current: Option<&Model>, cx: &Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let pair = self.data.selected.clone();
        let same = pair.is_some() && pair == self.data.default;
        let off = current
            .filter(|m| m.efforts.iter().any(|e| e.id == "off"))
            .map(|m| m.id.clone());
        let enabled = self.data.enabled;
        row()
            .gap(px(2.))
            .border_t_1()
            .border_color(colors.border)
            .p(px(4.))
            .child(
                Button::new("picker-default")
                    .small()
                    .ghost()
                    .icon(icon("RiPushpin2Line"))
                    .label(tr("设为默认"))
                    .disabled(!enabled || pair.is_none() || same)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.data.enabled
                            && let Some(pair) = pair.clone()
                        {
                            cx.emit(PickerEvent::SetDefault(pair));
                        }
                    })),
            )
            .child(
                Button::new("picker-reset")
                    .small()
                    .ghost()
                    .icon(icon("RiRefreshLine"))
                    .label(tr("重置为 off"))
                    .disabled(!enabled || off.is_none())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(id) = &off {
                            this.select(id.clone(), Some("off".into()), cx);
                        }
                    })),
            )
            .child(div().flex_1())
            .child(
                Button::new("picker-refresh")
                    .small()
                    .outline()
                    .icon(icon("RiRefreshLine"))
                    .label(tr(if self.data.refreshing {
                        "更新中…"
                    } else {
                        "刷新"
                    }))
                    .disabled(self.data.refreshing)
                    .on_click(
                        cx.listener(|_, _, _, cx| cx.emit(PickerEvent::Refresh { force: true })),
                    ),
            )
    }
}
impl Render for Picker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let selected = self
            .data
            .selected
            .as_ref()
            .and_then(|s| self.data.models.iter().find(|m| m.id == s.model));
        let choose = tr("选择模型");
        let name = selected.map_or(choose.as_ref(), |m| m.name.as_str());
        let level =
            selected.and_then(|model| effort::selected_effort(model, self.data.selected.as_ref()));
        let default = tr("默认");
        let label = level.map_or(default.as_ref(), |level| level.label.as_str());
        let mut pill = row()
            .gap(px(5.))
            .min_w_0()
            .child(
                div().text_xs().text_color(colors.muted_foreground).child(
                    selected
                        .and_then(|m| m.provider.chars().next())
                        .map_or_else(|| "·".into(), |c| c.to_uppercase().to_string()),
                ),
            )
            .child(div().min_w_0().truncate().child(name.to_owned()));
        if let Some(model) = selected {
            if let Some(level) = level {
                let reasoning: Vec<_> = model.efforts.iter().filter(|e| e.id != "off").collect();
                let lit = reasoning
                    .iter()
                    .position(|e| e.id == level.id)
                    .map_or(0, |rank| {
                        (((rank + 1) as f32 / reasoning.len() as f32) * 4.).round() as usize
                    });
                let color = match level.id.as_str() {
                    "off" | "minimal" | "low" => colors.info,
                    "xhigh" | "max" => colors.warning,
                    _ => colors.primary,
                };
                let mut bars = row().items_end().gap(px(2.)).h(px(12.));
                for (i, height) in [4., 6., 8., 12.].into_iter().enumerate() {
                    bars = bars.child(
                        div()
                            .w(px(2.))
                            .h(px(height))
                            .rounded(px(1.))
                            .bg(if i < lit { color } else { colors.border }),
                    );
                }
                pill = pill.child(bars).child(
                    div()
                        .text_xs()
                        .text_color(colors.muted_foreground)
                        .child(label.to_owned()),
                );
            }
            if model.has_key {
                pill = pill.child(icon("RiKey2Line").xsmall());
            }
        }
        let trigger = Button::new("model-picker-pill")
            .small()
            .ghost()
            .max_w(px(280.))
            .child(pill)
            .icon(IconName::ChevronDown)
            .disabled(!self.data.enabled)
            .accessibility_label(format!("{}: {name} · {label}", tr("模型与推理强度")));
        let entity = cx.entity().downgrade();
        let open_entity = entity.clone();
        let focus = self.query.read(cx).focus_handle(cx);
        Popover::new("model-picker-popover")
            .anchor(Anchor::BottomLeft)
            .offset(px(4.))
            .appearance(false)
            .track_focus(&focus)
            .trigger(trigger)
            .on_open_change(move |open, window, cx| {
                if *open {
                    let _ = open_entity.update(cx, |this, cx| {
                        this.query.update(cx, |input, cx| {
                            input.set_value("", window, cx);
                            input.focus(window, cx);
                        });
                        cx.emit(PickerEvent::Refresh { force: false });
                    });
                }
            })
            .content(move |_, window, cx| {
                entity
                    .update(cx, |this, cx| this.surface(window, cx))
                    .unwrap_or_else(|_| div().into_any_element())
            })
    }
}
