//! Screenshot-aligned grouped surface and bottom-composer trigger; host theme only.
use crate::{Model, Picker, effort};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::Input,
};
use gpui_kit::*;

pub(super) fn row() -> Div {
    div().flex().items_center()
}
pub(super) fn col() -> Div {
    div().flex().flex_col()
}
const WIDTH: f32 = 360.;

impl Picker {
    pub(crate) fn surface(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let query = self.query.read(cx).value().to_string();
        let searching = !query.trim().is_empty();
        let mut groups: Vec<(String, String, Vec<Model>)> = Vec::new();
        for model in self
            .data
            .models
            .iter()
            .filter(|model| effort::score(model, &query).is_some())
        {
            if let Some((_, _, items)) = groups
                .iter_mut()
                .find(|(id, _, _)| *id == model.provider_id)
            {
                items.push(model.clone());
            } else {
                groups.push((
                    model.provider_id.clone(),
                    model.provider.clone(),
                    vec![model.clone()],
                ));
            }
        }
        let mut rows = col().gap(px(2.));
        let mut visible = Vec::new();
        for (id, provider, mut models) in groups {
            models.sort_by_key(|model| effort::score(model, &query).unwrap_or(usize::MAX));
            let expanded = if searching {
                !self.search_folded.contains(&id)
            } else {
                self.expanded.contains(&id)
            };
            let key = id.clone();
            let source = models
                .first()
                .map_or("", |m| m.detail.split('·').next().unwrap_or("").trim())
                .to_owned();
            let initial = provider
                .chars()
                .next()
                .map_or_else(String::new, |c| c.to_uppercase().to_string());
            rows = rows.child(
                Button::new(SharedString::from(format!("provider:{id}")))
                    .small()
                    .ghost()
                    .w_full()
                    .accessibility_label(format!(
                        "模型分组 {provider}，{}",
                        if expanded { "已展开" } else { "已折叠" }
                    ))
                    .child(
                        row()
                            .w_full()
                            .min_w_0()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.muted_foreground)
                                    .child(initial),
                            )
                            .child(div().min_w_0().truncate().child(provider))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .max_w(px(130.))
                                    .truncate()
                                    .text_xs()
                                    .text_color(colors.muted_foreground)
                                    .child(format!(
                                        "{}{} {}",
                                        if models.iter().any(|m| m.has_key) {
                                            "API 密钥 · "
                                        } else {
                                            ""
                                        },
                                        source,
                                        models.len()
                                    )),
                            )
                            .child(
                                Icon::new(if expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .xsmall(),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let set = if searching {
                            &mut this.search_folded
                        } else {
                            &mut this.expanded
                        };
                        if !set.remove(&key) {
                            set.insert(key.clone());
                        }
                        cx.notify();
                    })),
            );
            if expanded {
                for model in models {
                    rows = rows.child(div().pl(px(24.)).child(self.model_row(
                        &model,
                        &query,
                        searching && self.highlighted == visible.len(),
                        cx,
                    )));
                    visible.push(model);
                }
            }
        }
        if self.data.models.is_empty() || (searching && visible.is_empty()) {
            rows = rows.child(div().p(px(12.)).text_color(colors.muted_foreground).child(
                if searching {
                    "没有匹配的模型"
                } else {
                    "没有可用模型，请先配置服务商"
                },
            ));
        }
        let current = self
            .data
            .selected
            .as_ref()
            .and_then(|s| self.data.models.iter().find(|m| m.id == s.model))
            .cloned();
        let width = WIDTH
            .min(f32::from(window.viewport_size().width) - 16.)
            .max(180.);
        let list_max = (f32::from(window.viewport_size().height) - 250.).clamp(48., 248.);
        let query_focus = self.query.read(cx).focus_handle(cx);
        let mut surface = col()
            .w(px(width))
            .max_h(px(
                (f32::from(window.viewport_size().height) - 64.).max(120.)
            ))
            .rounded(px(6.))
            .bg(colors.popover)
            .text_color(colors.popover_foreground)
            .text_size(px(12.))
            .border_1()
            .border_color(colors.border)
            .shadow_lg()
            .capture_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if !query_focus.is_focused(window)
                    || this.query.update(cx, |input, cx| {
                        input.marked_text_range(window, cx).is_some()
                    })
                    || visible.is_empty()
                {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "down" => this.highlighted = (this.highlighted + 1) % visible.len(),
                    "up" => {
                        this.highlighted = (this.highlighted + visible.len() - 1) % visible.len()
                    }
                    "enter" => {
                        let selected = &visible[this.highlighted.min(visible.len() - 1)];
                        let next = effort::selection(selected, this.data.selected.as_ref(), &query);
                        this.select(next.model, next.effort, cx);
                    }
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .child(
                div().p(px(4.)).child(
                    Input::new(&self.query)
                        .appearance(false)
                        .small()
                        .aria_label("搜索模型，可附加等级")
                        .prefix(Icon::new(IconName::Search).small()),
                ),
            )
            .child(
                div()
                    .id("model-picker-list")
                    .min_h_0()
                    .max_h(px(list_max))
                    .track_scroll(&self.scroll)
                    .overflow_y_scroll()
                    .child(rows),
            );
        if let Some(model) = &current
            && !model.efforts.is_empty()
        {
            surface = surface.child(self.rail(model, window, cx));
        }
        if !self.data.notice.is_empty() {
            surface = surface.child(
                div()
                    .px(px(8.))
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(self.data.notice.clone()),
            );
        }
        if let Some(id) = self.diagnostic_id.clone() {
            surface = surface.child(
                Button::new("picker-copy-diagnostic")
                    .link()
                    .small()
                    .label("复制诊断 ID")
                    .accessibility_label(format!("复制目录刷新诊断 ID {id}"))
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(id.clone()));
                    }),
            );
        }
        surface
            .child(self.footer(current.as_ref(), cx))
            .into_any_element()
    }
}
