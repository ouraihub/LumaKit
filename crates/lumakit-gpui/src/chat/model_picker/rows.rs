//! Two-line model rows and independent favorite controls.
use super::*;
impl ChatPanel {
    pub(super) fn model_row(
        &self,
        option: &ModelOption,
        selected: Option<&ModelRef>,
        explicit: Option<ReasoningEffort>,
        pinned: bool,
        section: &str,
        query: &str,
        highlighted: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let r = LumaTheme::global(cx).roles;
        let chosen = selected.is_some_and(|m| same_model(m, &option.model));
        let anchor = ScrollAnchor::for_handle(self.picker.list_scroll.clone());
        self.picker
            .row_anchors
            .borrow_mut()
            .insert(format!("{:?}", option.model), anchor.clone());
        let saved = option.clone();
        let selected = selected.cloned();
        let query = query.to_owned();
        let id = format!(
            "{section}:{}:{}:{}",
            option.model.provider, option.model.endpoint, option.model.model
        );
        let name = option.name.clone();
        let content = row()
            .flex_nowrap()
            .w_full()
            .min_w_0()
            .text_left()
            .gap(space::S8)
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .items_start()
                    .gap(space::S2)
                    .child(div().truncate().child(name.clone()))
                    .child(
                        div()
                            .text_style(text::METADATA)
                            .text_color(r.text_muted)
                            .truncate()
                            .child(detail(option)),
                    ),
            )
            .child(crate::model_capability_badges(&option.settings, r))
            .when(chosen, |d| {
                d.child(Icon::new(IconName::Check).small().text_color(r.primary))
            });
        let pick = Button::new(SharedString::from(id.clone()))
            .ghost()
            .w_full()
            .h_auto()
            .py(space::S6)
            .min_w_0()
            .child(content)
            .accessibility_label(format!(
                "选择模型 {name} · {} / {}",
                option.provider, option.endpoint
            ))
            .selected(chosen || highlighted)
            .disabled(!self.unlocked)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.pick_model(&saved, selected.as_ref(), explicit, &query, window, cx)
            }));
        let model = option.model.clone();
        row()
            .id(SharedString::from(format!("row:{id}")))
            .flex_nowrap()
            .gap(space::S2)
            .min_w_0()
            .anchor_scroll(Some(anchor))
            .child(div().flex_1().min_w_0().child(pick))
            .child(
                Button::new(SharedString::from(format!("pin:{id}")))
                    .xsmall()
                    .ghost()
                    .icon(IconName::Star)
                    .selected(pinned)
                    .accessibility_label(format!(
                        "{}模型 {name}",
                        if pinned { "取消固定" } else { "固定" }
                    ))
                    .disabled(!self.unlocked)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.emit(ChatAction::ToggleFavorite(model.clone()), cx)
                    })),
            )
    }
}
