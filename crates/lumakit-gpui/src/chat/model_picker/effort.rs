//! Native discrete slider; inherited default and automatic depth are distinct states.
use super::*;
use gpui_kit::base::{Slider as BaseSlider, SliderIndicator, SliderThumb, SliderTrack};

const RAIL_HEIGHT: Pixels = px(32.);
const TRACK_HEIGHT: Pixels = px(14.);
const THUMB_WIDTH: Pixels = px(64.);

fn native_rail(
    state: &Entity<SliderState>,
    index: usize,
    levels: &[ReasoningEffort],
    current: Option<ReasoningEffort>,
    disabled: bool,
    r: &lumakit_design::Roles,
) -> AnyElement {
    let position = index as f32 / levels.len().saturating_sub(1).max(1) as f32;
    let mut fill = row().flex_nowrap().gap(space::S0).h_full().w_full();
    for pair in levels.windows(2) {
        fill = fill.child(div().flex_1().h_full().bg(linear_gradient(
            90.,
            linear_color_stop(color(pair[0], r), 0.),
            linear_color_stop(color(pair[1], r), 1.),
        )));
    }
    BaseSlider::new(state)
        .disabled(disabled)
        .w_full()
        .h(RAIL_HEIGHT)
        .child(
            SliderTrack::new(state)
                .disabled(disabled)
                .relative()
                .w_full()
                .h(RAIL_HEIGHT)
                .flex()
                .items_center()
                .px(THUMB_WIDTH / 2.)
                .child(
                    SliderIndicator::new(state)
                        .relative()
                        .w_full()
                        .h(TRACK_HEIGHT)
                        .rounded(radius::LG)
                        .bg(r.control_bg)
                        .border_1()
                        .border_color(r.border_default)
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .w(relative(position))
                                .h_full()
                                .overflow_hidden()
                                .rounded(radius::LG)
                                .child(fill),
                        )
                        .child(
                            SliderThumb::new(state)
                                .disabled(disabled)
                                .absolute()
                                .left(relative(position))
                                .top(px(-6.))
                                .ml(-(THUMB_WIDTH / 2.))
                                .w(THUMB_WIDTH)
                                .h(px(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap(space::S4)
                                .rounded(radius::LG)
                                .bg(r.container_elevated_bg)
                                .border_1()
                                .border_color(r.border_default)
                                .shadow_sm()
                                .child(Icon::new(glyph(current)).small().text_color(
                                    current.map_or(r.text_muted, |level| color(level, r)),
                                ))
                                .child(
                                    div()
                                        .text_style(text::METADATA)
                                        .child(reasoning_label(current)),
                                ),
                        ),
                ),
        )
        .into_any_element()
}

pub(super) fn bars(
    level: ReasoningEffort,
    supported: &[ReasoningEffort],
    r: &lumakit_design::Roles,
) -> Div {
    let ranked: Vec<_> = supported
        .iter()
        .filter(|v| **v != ReasoningEffort::None)
        .collect();
    let lit = ranked
        .iter()
        .position(|v| **v == level)
        .map_or(0, |rank| ((rank + 1) * 4).div_ceil(ranked.len().max(1)));
    let mut bars = row().flex_nowrap().items_end().gap(space::S2).h(px(12.));
    for (index, height) in [4., 6., 8., 12.].into_iter().enumerate() {
        bars = bars.child(
            div()
                .w(px(2.))
                .h(px(height))
                .rounded(radius::XS)
                .bg(if index < lit {
                    color(level, r)
                } else {
                    r.border_default
                }),
        );
    }
    bars
}
impl ChatPanel {
    pub(super) fn effort_control(
        &mut self,
        option: &ModelOption,
        explicit: Option<ReasoningEffort>,
        _saved_default: Option<ReasoningEffort>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let r = LumaTheme::global(cx).roles;
        let supported = levels(option);
        if option.settings.reasoning.is_none() {
            return col();
        }
        let current = effective(option, explicit);
        let key = (option.model.clone(), supported.clone());
        let focus = self
            .picker
            .slider_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        if self.picker.slider_key.as_ref() != Some(&key) {
            self.picker.slider_subscription = None;
            let slider = cx.new(|_| {
                SliderState::new()
                    .min(0.)
                    .max(supported.len().saturating_sub(1).max(1) as f32)
                    .step(1.)
                    .default_value(0.)
            });
            let model = option.model.clone();
            let steps = supported.clone();
            let subscription = cx.subscribe(&slider, move |this, _, event, cx| {
                if !this.unlocked {
                    return;
                }
                if let SliderEvent::Change(value) = event {
                    if let Some(effort) = steps.get(value.end().round() as usize) {
                        this.emit(
                            ChatAction::SelectModelPair {
                                model: model.clone(),
                                effort: Some(*effort),
                            },
                            cx,
                        );
                    }
                }
            });
            self.picker.slider = Some(slider);
            self.picker.slider_subscription = Some(subscription);
            self.picker.slider_key = Some(key);
        }
        let mut section = col()
            .gap(space::S6)
            .px(space::S12)
            .py(space::S8)
            .border_t_1()
            .border_color(r.border_muted)
            .child(
                row().gap(space::S4).child(
                    div()
                        .text_style(text::METADATA)
                        .text_color(r.text_muted)
                        .truncate()
                        .child(format!("力度 · {}", option.name)),
                ),
            );
        if !supported.is_empty() {
            if let Some(slider) = &self.picker.slider {
                let index = supported
                    .iter()
                    .position(|value| Some(*value) == current)
                    .unwrap_or(0) as f32;
                if slider.read(cx).value().end() != index {
                    slider.update(cx, |state, cx| state.set_value(index, window, cx));
                }
                let model = option.model.clone();
                let steps = supported.clone();
                let focused = focus.clone();
                section = section.child(
                    div()
                        .id("effort-slider-focus")
                        .w_full()
                        .track_focus(&focus)
                        .tab_stop(true)
                        .role(Role::Slider)
                        .aria_label(format!(
                            "{} 推理强度：{}",
                            option.name,
                            reasoning_label(current)
                        ))
                        .aria_value(reasoning_label(current))
                        .aria_numeric_value(index as f64)
                        .aria_min_numeric_value(0.)
                        .aria_max_numeric_value(supported.len().saturating_sub(1) as f64)
                        .aria_numeric_value_step(1.)
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            focused.focus(window, cx)
                        })
                        .capture_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                            if !this.unlocked || steps.is_empty() {
                                return;
                            }
                            let i = index as usize;
                            let next = match event.keystroke.key.as_str() {
                                "left" | "down" => i.saturating_sub(1),
                                "right" | "up" => (i + 1).min(steps.len() - 1),
                                "home" => 0,
                                "end" => steps.len() - 1,
                                _ => return,
                            };
                            cx.stop_propagation();
                            this.emit(
                                ChatAction::SelectModelPair {
                                    model: model.clone(),
                                    effort: Some(steps[next]),
                                },
                                cx,
                            );
                        }))
                        .child(native_rail(
                            slider,
                            index as usize,
                            &supported,
                            current,
                            !self.unlocked || supported.len() < 2,
                            &r,
                        )),
                );
            }
            let mut labels = row().gap(space::S2).justify_between();
            for level in supported {
                let model = option.model.clone();
                labels = labels.child(
                    Button::new(SharedString::from(format!("effort-{}", word(level))))
                        .label(reasoning_label(Some(level)))
                        .accessibility_label(format!("推理强度 {}", reasoning_label(Some(level))))
                        .selected(current == Some(level))
                        .disabled(!self.unlocked)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.emit(
                                ChatAction::SelectModelPair {
                                    model: model.clone(),
                                    effort: Some(level),
                                },
                                cx,
                            )
                        })),
                );
            }
            section = section.child(labels);
            let model = option.model.clone();
            section = section.child(
                row().justify_end().child(
                    Button::new("effort-default")
                        .xsmall()
                        .ghost()
                        .label("恢复默认")
                        .accessibility_label("推理强度 默认")
                        .selected(explicit.is_none())
                        .disabled(!self.unlocked)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.emit(
                                ChatAction::SelectModelPair {
                                    model: model.clone(),
                                    effort: None,
                                },
                                cx,
                            )
                        })),
                ),
            );
        }
        if option
            .settings
            .reasoning
            .as_ref()
            .is_some_and(|config| config.efforts.contains(&ReasoningEffort::Auto))
        {
            let model = option.model.clone();
            section = section.child(
                Button::new("effort-auto")
                    .small()
                    .ghost()
                    .label("自动 · 服务商决定")
                    .selected(current == Some(ReasoningEffort::Auto))
                    .disabled(!self.unlocked)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.emit(
                            ChatAction::SelectModelPair {
                                model: model.clone(),
                                effort: Some(ReasoningEffort::Auto),
                            },
                            cx,
                        )
                    })),
            );
        }
        section.child(
            div()
                .text_style(text::METADATA)
                .text_color(
                    if matches!(current, Some(ReasoningEffort::Xhigh | ReasoningEffort::Max)) {
                        r.feedback_warning
                    } else {
                        r.text_muted
                    },
                )
                .child(current.map_or("默认：由服务商决定，不发送指定档位。", hint)),
        )
    }
}
