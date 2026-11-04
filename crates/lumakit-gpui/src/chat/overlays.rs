//! Chat menus, answer editor and viewport-constrained anchored surfaces.
use super::*;

impl ChatPanel {
    pub(super) fn render_overlay(&mut self, layer: Overlay, session: &ChatSession, options: &[ModelOption], image_options: &[ModelOption],
        settings: Option<&ModelSettings>, can_image: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let r = LumaTheme::global(cx).roles;
        let mut content = col().gap(space::S8);
        let menu = !matches!(layer, Overlay::Edit);
        let compact = matches!(layer, Overlay::Tools | Overlay::Quick);
        match layer {
            Overlay::History => {
                content = content.child("历史对话").child(Input::new(&self.search).aria_label("搜索对话"));
                if let Some(id) = self.delete.clone() {
                    content = content.child("确定删除这段对话？删除后无法恢复。")
                        .child(row().child(self.action("confirm-delete", "确定删除", self.unlocked, ChatAction::Delete(id), cx))
                            .child(Button::new("cancel-delete").small().label("取消").on_click(cx.listener(|this, _, _, cx| { this.delete = None; cx.notify(); }))));
                }
                let query = self.search.read(cx).value().to_lowercase();
                let mut found = false;
                for conversation in session.history().conversations.iter().rev().filter(|c| c.title.to_lowercase().contains(&query)) {
                    found = true;
                    let id = conversation.id.clone();
                    let select_id = id.clone();
                    content = content.child(row().flex_nowrap().child(div().flex_1().min_w_0().child(item(Button::new(SharedString::from(format!("history:{id}"))), conversation.title.clone())
                        .selected(session.history().selected.as_deref() == Some(id.as_str()))
                        .disabled(!self.unlocked).on_click(cx.listener(move |this, _, window, cx| {
                            this.emit(ChatAction::Select(select_id.clone()), cx); this.close(window, cx);
                        }))))
                        .child(icon_button(SharedString::from(format!("delete:{id}")), IconName::Trash, "删除对话").disabled(!self.unlocked)
                            .on_click(cx.listener(move |this, _, _, cx| { this.delete = Some(id.clone()); cx.notify(); }))));
                }
                if !found { content = content.child("没有匹配的历史对话"); }
                content = content.child(self.menu_action("new-history", "新对话", self.unlocked, ChatAction::New, cx));
            }
            Overlay::Tools => { content = self.tools_menu(session, image_options, can_image, cx); }
            Overlay::Quick => { content = self.customize_toolbar(session, cx); }
            Overlay::Models(image) => {
                content = content.child(if image { "选择生图模型" } else { "选择聊天模型" })
                    .child(Input::new(&self.search).aria_label("搜索模型、服务商、端点"));
                let query = self.search.read(cx).value().to_lowercase();
                let mut found = false;
                for option in (if image { image_options } else { options }).iter().filter(|o| model_label(o).to_lowercase().contains(&query)) {
                    found = true;
                    let action = if image { let mut next = session.composer().clone(); next.image_mode = true; next.image_model = Some(option.model.clone()); ChatAction::SetComposer(next) }
                        else { ChatAction::SelectModel(option.model.clone()) };
                    let picked = if image { session.composer().image_model.as_ref() } else { session.history().model.as_ref() };
                    content = content.child(item(Button::new(SharedString::from(model_label(option))), model_label(option))
                        .selected(picked.is_some_and(|m| same_model(m, &option.model))).disabled(!self.unlocked)
                        .on_click(cx.listener(move |this, _, window, cx| { this.emit(action.clone(), cx); this.close(window, cx); })));
                }
                if !found { content = content.child("没有匹配的模型"); }
                content = content.child(self.menu_action("configure-models", "模型设置", true, ChatAction::OpenSettings, cx));
            }
            Overlay::RetryModels => {
                content = content.child("换模型重答").child(Input::new(&self.search).aria_label("搜索重试模型"));
                let query = self.search.read(cx).value().to_lowercase();
                for option in options.iter().filter(|o| model_label(o).to_lowercase().contains(&query)) {
                    let model = option.model.clone();
                    content = content.child(item(Button::new(SharedString::from(model_label(option))), model_label(option)).disabled(!self.unlocked)
                        .on_click(cx.listener(move |this, _, window, cx| { this.emit(ChatAction::RetryWith(model.clone()), cx); this.close(window, cx); })));
                }
            }
            Overlay::Reasoning => {
                content = content.child("推理强度：默认沿用模型配置");
                let efforts = std::iter::once(None).chain(settings.and_then(|s| s.reasoning.as_ref()).into_iter().flat_map(|r| r.efforts.iter().copied().map(Some)));
                for effort in efforts {
                    let mut next = session.composer().clone(); next.reasoning = effort;
                    content = content.child(self.menu_action(reasoning_label(effort), reasoning_label(effort), self.unlocked,
                        ChatAction::SetComposer(next), cx).selected(session.composer().reasoning == effort));
                }
            }
            Overlay::Usage => {
                content = content.child("上一回答的实际用量；不是草稿估算");
                if let Some(attempt) = session.selected().and_then(|c| c.turns.last()).and_then(|t| t.attempts.last()) { content = content.child(usage_details(attempt)); }
                else { content = content.child("用量未知"); }
            }
            Overlay::More(conversation, turn, attempt_id) => {
                content = content.child("更多操作");
                if let Some(attempt) = session.history().conversations.iter().find(|c| c.id == conversation)
                    .and_then(|c| c.turns.iter().find(|t| t.id == turn)).and_then(|t| t.attempts.iter().find(|a| a.id == attempt_id)) {
                    let complete = self.unlocked && attempt.status == TurnStatus::Completed;
                    let (_, raw) = self.markdown.view(&attempt.id, &attempt.text);
                    let edit_conversation = conversation.clone(); let edit_turn = turn.clone(); let edit_id = attempt_id.clone();
                    content = content.child(item(Button::new("edit-answer"), "编辑").disabled(!complete)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.edit = Some(Edit { conversation: edit_conversation.clone(), turn: edit_turn.clone(), attempt: edit_id.clone(), expected: raw.clone() });
                            this.editor.update(cx, |state, cx| { state.set_value(raw.as_ref(), window, cx); state.focus(window, cx); });
                            this.overlay = Some(Overlay::Edit); cx.notify();
                        })))
                        .child(self.menu_action("fork", "分支", self.unlocked, ChatAction::ForkAttempt { conversation_id: conversation.clone(), turn_id: turn.clone(), attempt_id: attempt_id.clone() }, cx))
                        .child(self.menu_action("clone-conversation", "复制对话", self.unlocked, ChatAction::CopyConversation { conversation_id: conversation.clone() }, cx))
                        .child(self.menu_action("delete-answer", "删除回答", self.unlocked, ChatAction::DeleteAttempt { conversation_id: conversation, turn_id: turn, attempt_id }, cx));
                    // These host capabilities do not exist in LumaDesk. Never emit a fake successful action.
                    for (id, label) in [("note", "保存到笔记（宿主未接入）"), ("insert", "插入编辑器（宿主未接入）"), ("export", "导出 Markdown（宿主未接入）"), ("translate", "翻译（宿主未接入）")] {
                        content = content.child(item(Button::new(id), label).disabled(true));
                    }
                } else { content = content.child("回答已不存在"); }
            }
            Overlay::Edit => {
                content = content.child("编辑回答").child(Textarea::new(&self.editor).aria_label("回答编辑内容").h(EDITOR_HEIGHT).disabled(!self.unlocked))
                    .child(Button::new("save-edit").small().primary().label("保存").disabled(!self.unlocked)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(edit) = &this.edit {
                                let action = ChatAction::SubmitEdit { conversation_id: edit.conversation.clone(), turn_id: edit.turn.clone(),
                                    attempt_id: edit.attempt.clone(), expected_text: edit.expected.to_string(), next_text: this.editor.read(cx).value().to_string() };
                                this.emit(action, cx); this.close(window, cx);
                            }
                        })));
            }
        }
        let viewport = window.viewport_size();
        let (width, height) = (f32::from(viewport.width), f32::from(viewport.height));
        let trigger = if menu { self.anchor.as_ref().and_then(|key| self.anchors.borrow().get(key).copied()) } else { None };
        // Menus open below a trigger in the upper half of the window and above one in the lower half,
        // aligned to the trigger edge nearer the window centre. Dialogs, and menus whose trigger is gone, stay centered.
        let below = trigger.is_some_and(|t| f32::from(t.center().y) < height / 2.);
        let layer_width = if compact { toolbar::TOOL_MENU_WIDTH } else { LAYER_W };
        let (surface_w, surface_h) = match trigger {
            Some(t) => (f32::from(layer_width).min(width - 2. * MENU_MARGIN),
                if below { height - f32::from(t.bottom()) } else { f32::from(t.top()) } - MENU_MARGIN - f32::from(MENU_GAP)),
            None => (f32::from(layer_width).min(width - 2. * MENU_MARGIN), height - LAYER_MARGIN),
        };
        let surface = col().id("chat-layer").occlude().w(px(surface_w)).when(trigger.is_none(), |s| s.max_w_full())
            .max_h(px(surface_h.max(LAYER_MIN_HEIGHT))).p(if compact { space::S4 } else { space::S12 }).gap(space::S4).rounded(radius::LG)
            .bg(r.container_elevated_bg).border_1().border_color(r.border_default).shadow_lg()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    let search_ime = this.search.update(cx, |state, cx| state.marked_text_range(window, cx).is_some());
                    let edit_ime = this.editor.update(cx, |state, cx| state.marked_text_range(window, cx).is_some());
                    if !search_ime && !edit_ime { this.close(window, cx); cx.stop_propagation(); }
                }
            }))
            .child(div().id("chat-layer-scroll").flex_1().min_h_0().overflow_y_scroll().child(content))
            .when(!compact, |s| s.child(row().justify_end().child(Button::new("close-layer").small().ghost().label(if self.edit.is_some() { "取消" } else { "关闭" })
                .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))))))
            .focus_trap("chat-layer-trap", &self.focus);
        let Some(trigger) = trigger else {
            return div().absolute().inset_0().occlude().flex().items_center().justify_center().p(space::S12)
                .bg(r.overlay).child(surface).into_any_element();
        };
        let (corner, position) = match (below, f32::from(trigger.center().x) > width / 2.) {
            (true, false) => (Anchor::TopLeft, trigger.bottom_left()), (true, true) => (Anchor::TopRight, trigger.bottom_right()),
            (false, false) => (Anchor::BottomLeft, trigger.origin), (false, true) => (Anchor::BottomRight, trigger.top_right()),
        };
        // A transparent catcher closes the menu on any press outside it; the menu itself occludes the catcher.
        div().absolute().inset_0().occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close(window, cx)))
            .child(deferred(anchored().position(position).anchor(corner).offset(point(px(0.), if below { MENU_GAP } else { -MENU_GAP }))
                .snap_to_window_with_margin(px(MENU_MARGIN)).child(surface)).with_priority(1))
            .into_any_element()
    }
    pub(super) fn menu_layer(&self, id: &'static str, label: &'static str, enabled: bool, layer: Overlay, cx: &Context<Self>) -> Button {
        item(Button::new(id), label).disabled(!enabled)
            .on_click(cx.listener(move |this, _, window, cx| this.open(layer.clone(), id.into(), window, cx)))
    }
    fn menu_action(&self, id: &'static str, label: &'static str, enabled: bool, action: ChatAction, cx: &Context<Self>) -> Button {
        item(Button::new(id), label).disabled(!enabled).on_click(cx.listener(move |this, _, _, cx| this.emit(action.clone(), cx)))
    }
}
/// A full-width menu row: Kit buttons center their content, so the label fills the row and starts left.
fn item(button: Button, label: impl Into<SharedString>) -> Button {
    let label = label.into();
    button.small().ghost().w_full().accessibility_label(label.clone()).child(div().w_full().min_w_0().truncate().child(label))
}
pub(super) fn usage_details(attempt: &Attempt) -> Div {
    let mut body = col().gap(space::S4).text_style(text::METADATA);
    if attempt.rounds.is_empty() { body = body.child("用量未提供 · 费用未知"); }
    for (index, round) in attempt.rounds.iter().enumerate() {
        let label = if let Some(usage) = &round.usage {
            let tokens = format!("输入 {} · 输出 {} · 缓存读取 {} · 缓存写入 {} token", usage.input_tokens, usage.output_tokens, usage.cache_read_tokens, usage.cache_write_tokens);
            match (&round.pricing, round.cost) { (Some(p), Some(c)) if c.is_finite() && c >= 0. && !p.currency.is_empty() => format!("{tokens} · 费用 {} {c}", p.currency),
                _ => format!("{tokens} · 费用未知") }
        } else { "用量未提供 · 费用未知".into() };
        body = body.child(format!("第 {} 轮 · {label}", index + 1));
    }
    body.child("按请求保存的用量与价格快照；不同币种不换算，费用不含搜索等额外服务费。缓存读写已包含在输入 token 中。")
}
