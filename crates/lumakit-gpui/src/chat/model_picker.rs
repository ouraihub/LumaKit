//! One model/effort pill and its grouped, keyboard-operable picker.
use super::*;
mod rows;
use gpui_kit::component::{
    Icon,
    slider::{SliderEvent, SliderState},
};
mod effort;
mod rules;
use rules::*;

pub(super) const WIDTH: Pixels = px(360.);
const LIST_HEIGHT: Pixels = px(248.);
const EFFORT_HEIGHT: Pixels = px(172.);
const PILL_WIDTH: Pixels = px(280.);
#[derive(Default)]
pub(super) struct PickerState {
    pub expanded: HashSet<String>,
    pub search_folded: HashSet<String>,
    pub show_all: bool,
    pub initialized: bool,
    pub highlighted: usize,
    pub list_scroll: ScrollHandle,
    pub row_anchors: RefCell<HashMap<String, ScrollAnchor>>,
    pub slider: Option<Entity<SliderState>>,
    pub slider_key: Option<(ModelRef, Vec<ReasoningEffort>)>,
    pub slider_subscription: Option<Subscription>,
    pub slider_focus: Option<FocusHandle>,
    pub refreshing: bool,
    pub refresh_notice: String,
}
impl ChatPanel {
    /// Reports the host's actual catalog refresh, including failures.
    pub fn set_model_refresh_state(&mut self, refreshing: bool, notice: String) {
        self.picker.refreshing = refreshing;
        self.picker.refresh_notice = notice;
    }
    pub(super) fn model_pill(
        &self,
        session: &ChatSession,
        options: &[ModelOption],
        cx: &Context<Self>,
    ) -> Button {
        let r = LumaTheme::global(cx).roles;
        let current = session
            .history()
            .model
            .as_ref()
            .and_then(|m| options.iter().find(|o| same_model(m, &o.model)));
        let name = current.map_or("选择模型", |o| o.name.as_str());
        let mut label = row()
            .flex_nowrap()
            .gap(space::S6)
            .child(
                div()
                    .text_style(text::METADATA)
                    .text_color(r.text_muted)
                    .child(
                        current
                            .and_then(|o| o.provider.chars().next())
                            .map(|c| c.to_uppercase().to_string())
                            .unwrap_or_else(|| "·".into()),
                    ),
            )
            .child(div().min_w_0().truncate().child(name.to_owned()));
        let level = current.and_then(|o| effective(o, session.composer().reasoning));
        if let (Some(option), Some(level)) = (current, level) {
            label = label.child(effort::bars(level, &levels(option), &r)).child(
                div()
                    .text_style(text::METADATA)
                    .text_color(r.text_muted)
                    .child(reasoning_label(Some(level))),
            );
        }
        if current.is_some_and(|o| o.has_key) {
            label = label.child(
                Icon::new(IconName::KeyRound)
                    .small()
                    .text_color(r.text_muted),
            );
        }
        label = label.child(Icon::new(IconName::ChevronDown).small());
        Button::new("model")
            .small()
            .ghost()
            .max_w(PILL_WIDTH)
            .child(label)
            .accessibility_label(format!(
                "模型与推理强度：{name} · {}",
                level.map_or("默认", |v| reasoning_label(Some(v)))
            ))
            .tooltip("选择模型与推理强度")
            .disabled(!self.unlocked)
            .on_click(cx.listener(|this, _, window, cx| {
                if matches!(this.overlay, Some(Overlay::ModelPicker)) {
                    this.close(window, cx);
                } else {
                    this.open(Overlay::ModelPicker, "model".into(), window, cx);
                }
            }))
    }
    pub(super) fn pick_model(
        &mut self,
        option: &ModelOption,
        current: Option<&ModelRef>,
        explicit: Option<ReasoningEffort>,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let typed = typed_level(query, option);
        if current.is_some_and(|m| same_model(m, &option.model)) && typed.is_none() {
            self.close(window, cx);
            return;
        }
        let effort = typed.or_else(|| {
            explicit.filter(|level| {
                option
                    .settings
                    .reasoning
                    .as_ref()
                    .is_some_and(|r| r.efforts.contains(level))
            })
        });
        self.emit(
            ChatAction::SelectModelPair {
                model: option.model.clone(),
                effort,
            },
            cx,
        );
        self.search
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.picker.expanded.insert(option.model.provider.clone());
        if typed.is_some() {
            self.close(window, cx);
        }
    }
    pub(super) fn model_picker(
        &mut self,
        session: &ChatSession,
        options: &[ModelOption],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let r = LumaTheme::global(cx).roles;
        self.picker.row_anchors.borrow_mut().clear();
        let selected = session.history().model.as_ref();
        let prefs = session.model_picker_preferences();
        if !self.picker.initialized {
            self.picker.initialized = true;
            if let Some(current) =
                selected.filter(|m| !prefs.favorites.iter().any(|p| same_model(p, m)))
            {
                self.picker.expanded.insert(current.provider.clone());
            }
        }
        let query = self.search.read(cx).value().to_string();
        let searching = !query.trim().is_empty();
        let favorites: Vec<_> = prefs
            .favorites
            .iter()
            .filter_map(|m| options.iter().find(|o| same_model(m, &o.model)))
            .collect();
        let recents: Vec<_> = prefs
            .recents
            .iter()
            .filter(|m| !prefs.favorites.iter().any(|p| same_model(m, p)))
            .filter_map(|m| options.iter().find(|o| same_model(m, &o.model)))
            .collect();
        let folded =
            !searching && !self.picker.show_all && (!favorites.is_empty() || !recents.is_empty());
        let mut rows = col().gap(space::S2);
        let mut visible = Vec::new();
        for (section, name, models) in [
            ("pinned", "固定", favorites),
            ("recent", "最近使用", recents),
        ] {
            let matches: Vec<_> = models
                .into_iter()
                .filter(|o| score(&query, o).is_some())
                .collect();
            if !matches.is_empty() {
                rows = rows.child(
                    div()
                        .px(space::S8)
                        .pt(space::S6)
                        .text_style(text::METADATA)
                        .text_color(r.text_muted)
                        .child(name),
                );
            }
            for option in matches {
                let index = visible.len();
                visible.push(option.clone());
                rows = rows.child(self.model_row(
                    option,
                    selected,
                    session.composer().reasoning,
                    prefs.favorites.iter().any(|m| same_model(m, &option.model)),
                    section,
                    &query,
                    index == self.picker.highlighted,
                    cx,
                ));
            }
        }
        if folded {
            rows = rows.child(
                Button::new("all-models")
                    .small()
                    .ghost()
                    .w_full()
                    .icon(IconName::ChevronRight)
                    .label(format!("全部模型 · {}", options.len()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.picker.show_all = true;
                        cx.notify();
                    })),
            );
        } else {
            let mut groups: Vec<(String, String, Vec<&ModelOption>)> = Vec::new();
            for option in options.iter().filter(|o| {
                !prefs.favorites.iter().any(|m| same_model(m, &o.model))
                    && score(&query, o).is_some()
            }) {
                if let Some((_, _, items)) = groups
                    .iter_mut()
                    .find(|(id, _, _)| id == &option.model.provider)
                {
                    items.push(option);
                } else {
                    groups.push((
                        option.model.provider.clone(),
                        option.provider.clone(),
                        vec![option],
                    ));
                }
            }
            for (id, name, mut models) in groups {
                models.sort_by_key(|o| score(&query, o).unwrap_or(usize::MAX));
                let expanded = if searching {
                    !self.picker.search_folded.contains(&id)
                } else {
                    self.picker.expanded.contains(&id)
                };
                let key = id.clone();
                rows = rows.child(
                    Button::new(SharedString::from(format!("provider:{id}")))
                        .small()
                        .ghost()
                        .w_full()
                        .icon(if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .child(
                            row()
                                .w_full()
                                .flex_nowrap()
                                .gap(space::S8)
                                .child(div().flex_1().text_left().child(name.clone()))
                                .child(
                                    div()
                                        .text_style(text::METADATA)
                                        .child(models.len().to_string()),
                                ),
                        )
                        .accessibility_label(format!(
                            "模型分组 {name}，{}",
                            if expanded { "已展开" } else { "已折叠" }
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let set = if searching {
                                &mut this.picker.search_folded
                            } else {
                                &mut this.picker.expanded
                            };
                            if !set.remove(&key) {
                                set.insert(key.clone());
                            }
                            cx.notify();
                        })),
                );
                if expanded {
                    for option in models {
                        let index = visible.len();
                        visible.push(option.clone());
                        rows = rows.child(div().pl(space::S16).child(self.model_row(
                            option,
                            selected,
                            session.composer().reasoning,
                            false,
                            "all",
                            &query,
                            index == self.picker.highlighted,
                            cx,
                        )));
                    }
                }
            }
        }
        if visible.is_empty() && (searching || options.is_empty()) {
            rows = rows.child(
                div()
                    .p(space::S12)
                    .text_color(r.text_muted)
                    .child(if searching {
                        "没有匹配的模型"
                    } else {
                        "没有可用模型，请在模型设置中添加并启用服务商。"
                    }),
            );
        }
        let current = selected.and_then(|m| options.iter().find(|o| same_model(m, &o.model)));
        let input = Input::new(&self.search)
            .aria_label("搜索模型或模型名 high")
            .prefix(Icon::new(IconName::Search).small());
        let count = visible.len();
        let selected_model = selected.cloned();
        let effort = session.composer().reasoning;
        let typed = query.clone();
        let mut content = col()
            .min_h_0()
            .gap(space::S4)
            .capture_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if !this.search.read(cx).focus_handle(cx).is_focused(window) {
                    return;
                }
                if this.search.update(cx, |input, cx| {
                    input.marked_text_range(window, cx).is_some()
                }) {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "down" if count > 0 => {
                        this.picker.highlighted = (this.picker.highlighted + 1) % count;
                        cx.stop_propagation();
                        if let Some(option) = visible.get(this.picker.highlighted) {
                            if let Some(anchor) = this
                                .picker
                                .row_anchors
                                .borrow()
                                .get(&format!("{:?}", option.model))
                            {
                                anchor.scroll_to(window, cx);
                            }
                        }
                        cx.notify();
                    }
                    "up" if count > 0 => {
                        this.picker.highlighted = (this.picker.highlighted + count - 1) % count;
                        cx.stop_propagation();
                        if let Some(option) = visible.get(this.picker.highlighted) {
                            if let Some(anchor) = this
                                .picker
                                .row_anchors
                                .borrow()
                                .get(&format!("{:?}", option.model))
                            {
                                anchor.scroll_to(window, cx);
                            }
                        }
                        cx.notify();
                    }
                    "enter" if count > 0 => {
                        if let Some(option) = visible.get(this.picker.highlighted.min(count - 1)) {
                            this.pick_model(
                                option,
                                selected_model.as_ref(),
                                effort,
                                &typed,
                                window,
                                cx,
                            );
                            cx.stop_propagation();
                        }
                    }
                    _ => {}
                }
            }))
            .child(div().p(space::S8).child(input))
            .child(
                div()
                    .id("model-picker-list")
                    .min_h_0()
                    .track_scroll(&self.picker.list_scroll)
                    .max_h(LIST_HEIGHT)
                    .overflow_y_scroll()
                    .child(rows),
            );
        if let Some(option) = current {
            content = content.child(
                self.effort_control(
                    option,
                    session.composer().reasoning,
                    prefs
                        .default_pair
                        .as_ref()
                        .filter(|pair| same_model(&pair.model, &option.model))
                        .and_then(|pair| effective(option, pair.effort)),
                    window,
                    cx,
                )
                .min_h(EFFORT_HEIGHT)
                .flex_none(),
            );
        }
        let mut footer = row()
            .gap(space::S4)
            .p(space::S8)
            .border_t_1()
            .border_color(r.border_muted);
        if let Some(option) = current {
            let pair = &prefs.default_pair;
            let same = pair.as_ref().is_some_and(|p| {
                same_model(&p.model, &option.model) && p.effort == session.composer().reasoning
            });
            let model = option.model.clone();
            let effort = session.composer().reasoning;
            footer = footer.child(
                Button::new("set-default-model")
                    .small()
                    .ghost()
                    .label(if same {
                        "新对话默认"
                    } else {
                        "设为默认"
                    })
                    .disabled(!self.unlocked || same)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.emit(
                            ChatAction::SetDefaultModelPair {
                                model: model.clone(),
                                effort,
                            },
                            cx,
                        )
                    })),
            );
            let default = pair
                .as_ref()
                .filter(|p| same_model(&p.model, &option.model))
                .map(|p| p.effort)
                .unwrap_or(None);
            if default != session.composer().reasoning {
                let model = option.model.clone();
                footer = footer.child(
                    Button::new("reset-effort")
                        .small()
                        .ghost()
                        .label("恢复默认强度")
                        .disabled(!self.unlocked)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.emit(
                                ChatAction::SelectModelPair {
                                    model: model.clone(),
                                    effort: default,
                                },
                                cx,
                            )
                        })),
                );
            }
        }
        footer = footer
            .child(
                Button::new("refresh-models")
                    .small()
                    .outline()
                    .icon(IconName::RefreshCw)
                    .label(if self.picker.refreshing {
                        "更新中…"
                    } else {
                        "刷新"
                    })
                    .disabled(self.picker.refreshing || !self.unlocked)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.emit(ChatAction::RefreshModels { force: true }, cx)
                    })),
            )
            .child(
                Button::new("picker-settings")
                    .small()
                    .ghost()
                    .icon(IconName::Settings2)
                    .accessibility_label("模型设置")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.emit(ChatAction::OpenSettings, cx)),
                    ),
            );
        if !self.picker.refresh_notice.is_empty() {
            content = content.child(
                div()
                    .px(space::S8)
                    .text_style(text::METADATA)
                    .text_color(r.text_muted)
                    .child(self.picker.refresh_notice.clone()),
            );
        }
        content.child(footer)
    }
}
