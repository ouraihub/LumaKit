//! Native draggable discrete effort rail, clickable ticks and keyboard navigation.
use crate::{Model, Picker, effort::selected_effort, icons::glyph};
use gpui_kit::base::{Slider, SliderIndicator, SliderThumb, SliderTrack};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    slider::{SliderEvent, SliderState},
};
use gpui_kit::*;

impl Picker {
    pub(crate) fn rail(
        &mut self,
        model: &Model,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors;
        let steps: Vec<_> = model.efforts.iter().map(|e| e.id.clone()).collect();
        let current = selected_effort(model, self.data.selected.as_ref());
        let index = current
            .and_then(|e| steps.iter().position(|id| id == &e.id))
            .unwrap_or(0);
        let key = (model.id.clone(), steps.clone());
        if self.slider_key.as_ref() != Some(&key) {
            let state = cx.new(|_| {
                SliderState::new()
                    .min(0.)
                    .max(steps.len().saturating_sub(1).max(1) as f32)
                    .step(1.)
                    .default_value(index as f32)
            });
            let model_id = model.id.clone();
            let ids = steps.clone();
            self.slider_subscription = Some(cx.subscribe(&state, move |this, _, event, cx| {
                if let SliderEvent::Change(value) = event
                    && let Some(id) = ids.get(value.end().round() as usize)
                {
                    this.select(model_id.clone(), Some(id.clone()), cx);
                }
            }));
            self.slider = Some(state);
            self.slider_key = Some(key);
        }
        let Some(state) = self.slider.clone() else {
            return div().into_any_element();
        };
        if state.read(cx).value().end() != index as f32 {
            state.update(cx, |s, cx| s.set_value(index as f32, window, cx));
        }
        let label = current.map_or("默认", |e| e.label.as_str()).to_owned();
        let hint = current
            .map_or("使用服务商默认强度", |e| e.hint.as_str())
            .to_owned();
        let icon = glyph(current.map(|e| e.id.as_str()));
        let color = match current.map(|e| e.id.as_str()) {
            Some("off" | "minimal" | "low") => colors.info,
            Some("xhigh" | "max") => colors.warning,
            _ => colors.primary,
        };
        let mut fill = div().flex().flex_nowrap().h_full().w_full();
        for pair in model.efforts.windows(2) {
            let tone = |id: &str| match id {
                "off" | "minimal" | "low" => colors.info,
                "xhigh" | "max" => colors.warning,
                _ => colors.primary,
            };
            fill = fill.child(div().flex_1().h_full().bg(linear_gradient(
                90.,
                linear_color_stop(tone(&pair[0].id), 0.),
                linear_color_stop(tone(&pair[1].id), 1.),
            )));
        }
        let position = index as f32 / steps.len().saturating_sub(1).max(1) as f32;
        let enabled = self.data.enabled;
        let model_id = model.id.clone();
        let focus = self.slider_focus.clone();
        let rail = div()
            .id("effort-keyboard")
            .track_focus(&focus)
            .tab_stop(true)
            .role(Role::Slider)
            .aria_label(format!("推理强度：{label}"))
            .aria_value(label.clone())
            .aria_numeric_value(index as f64)
            .aria_min_numeric_value(0.)
            .aria_max_numeric_value(steps.len().saturating_sub(1) as f64)
            .aria_numeric_value_step(1.)
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                focus.focus(window, cx)
            })
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                if !enabled || steps.is_empty() {
                    return;
                }
                let next = match event.keystroke.key.as_str() {
                    "left" | "down" => index.saturating_sub(1),
                    "right" | "up" => (index + 1).min(steps.len() - 1),
                    "home" => 0,
                    "end" => steps.len() - 1,
                    _ => return,
                };
                cx.stop_propagation();
                this.select(model_id.clone(), Some(steps[next].clone()), cx);
            }))
            .child(
                Slider::new(&state)
                    .disabled(!enabled || model.efforts.len() < 2)
                    .w_full()
                    .h(px(32.))
                    .child(
                        SliderTrack::new(&state)
                            .disabled(!enabled)
                            .relative()
                            .w_full()
                            .h(px(32.))
                            .flex()
                            .items_center()
                            .px(px(34.))
                            .child(
                                SliderIndicator::new(&state)
                                    .relative()
                                    .w_full()
                                    .h(px(12.))
                                    .rounded(px(8.))
                                    .bg(colors.secondary)
                                    .border_1()
                                    .border_color(colors.border)
                                    .child(
                                        div()
                                            .absolute()
                                            .left_0()
                                            .top_0()
                                            .w(relative(position))
                                            .h_full()
                                            .rounded(px(8.))
                                            .overflow_hidden()
                                            .child(fill),
                                    )
                                    .child(
                                        SliderThumb::new(&state)
                                            .disabled(!enabled)
                                            .absolute()
                                            .left(relative(position))
                                            .top(px(-6.))
                                            .ml(px(-34.))
                                            .w(px(68.))
                                            .h(px(24.))
                                            .flex()
                                            .flex_nowrap()
                                            .gap(px(4.))
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(12.))
                                            .bg(colors.popover)
                                            .border_1()
                                            .border_color(colors.border)
                                            .shadow_sm()
                                            .child(icon.small().text_color(color))
                                            .child(div().whitespace_nowrap().child(label)),
                                    ),
                            ),
                    ),
            );
        let ticks = model
            .efforts
            .iter()
            .map(|level| {
                let id = model.id.clone();
                let effort = level.id.clone();
                Button::new(SharedString::from(format!("effort:{}", level.id)))
                    .xsmall()
                    .ghost()
                    .flex_1()
                    .min_w_0()
                    .px(px(0.))
                    .text_size(px(11.))
                    .whitespace_nowrap()
                    .label(level.label.clone())
                    .accessibility_label(format!("推理强度 {}", level.label))
                    .disabled(!enabled)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select(id.clone(), Some(effort.clone()), cx)
                    }))
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .px(px(8.))
            .pt(px(8.))
            .pb(px(4.))
            .gap(px(2.))
            .border_t_1()
            .border_color(colors.border)
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(format!("力度 · {}", model.name)),
            )
            .child(rail)
            .child(div().flex().flex_nowrap().w_full().children(ticks))
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(hint),
            )
            .into_any_element()
    }
}
