//! Stateful GPUI rendering over a borrowed ChatSession; emits existing business actions.
use std::{cell::RefCell, collections::{HashMap, HashSet}, rc::Rc};
use gpui_kit::assets::IconName;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::{*, prelude::FluentBuilder as _};
use gpui_kit::component::{Disableable, Selectable, Sizable, FocusTrapElement,
    button::{Button, ButtonVariants}, input::{Enter, Input, InputState, InputEvent, Textarea, TextareaState}};
use lumakit_ai::{ModelRef, chat::{Attempt, ChatSession, ComposerSettings, SearchStatus, ToolbarTool},
    history::TurnStatus, model_config::{ModelSettings, ReasoningEffort}};
use lumakit_ai::chat::{ChatAction, ModelOption};
use lumakit_design::{LumaTheme, components::TextStyled as _, metrics::{radius, size, space, text}};
use crate::{markdown::{MarkdownCache, web_link}, media::MediaPreview};

/// Composer height when collapsed, and the bounds of the expanded height.
const COMPOSER_HEIGHT: f32 = 72.;
const COMPOSER_EXPANDED: (f32, f32) = (64., 320.);
/// Height of the scrolling notice above the composer.
const NOTICE_MAX: Pixels = px(72.);
/// Distance from the bottom that still counts as following new output.
const FOLLOW_SLOP: Pixels = px(48.);
/// Max height of expanded diagnostic, reasoning and tool details.
const DETAIL_MAX: Pixels = px(180.);
/// Generated image preview.
const IMAGE_W: Pixels = px(240.);
const IMAGE_H: Pixels = px(220.);
/// Floating layer width and the margin it keeps from the window edge.
const LAYER_W: Pixels = px(460.);
const LAYER_MARGIN: f32 = 48.;
const LAYER_MIN_HEIGHT: f32 = 80.;
/// Distance between a menu and its trigger, and the margin a menu keeps from the window edge.
const MENU_GAP: Pixels = px(4.);
const MENU_MARGIN: f32 = 8.;
/// Model button width in the toolbar.
const MODEL_BUTTON_MAX: Pixels = px(160.);
/// Answer editor height.
const EDITOR_HEIGHT: Pixels = px(220.);
/// Pixel distance of one wheel line when converting line-based scroll deltas.
const SCROLL_LINE: Pixels = px(20.);

#[derive(Clone)]
enum Overlay { History, Tools, Quick, Models(bool), Reasoning, Usage, More(String, String, String), RetryModels, Edit }
struct Edit { conversation: String, turn: String, attempt: String, expected: SharedString }
/// Last laid-out bounds of each menu trigger, written during prepaint so a menu opens beside its button.
type Anchors = Rc<RefCell<HashMap<SharedString, Bounds<Pixels>>>>;

/// Per-instance inputs, navigation layers and bounded display caches. No credentials or history copy.
pub struct ChatPanel {
    input: Entity<TextareaState>, search: Entity<InputState>, editor: Entity<TextareaState>,
    _subscriptions: Vec<Subscription>,
    conversation: Option<String>, draft: SharedString, external_busy: bool,
    unlocked: bool, valid_model: bool, expanded: bool, overlay: Option<Overlay>,
    delete: Option<String>, edit: Option<Edit>, details: HashSet<String>,
    focus: FocusHandle, scroll: ScrollHandle, follow: bool,
    markdown: MarkdownCache, media: MediaPreview,
    anchors: Anchors, anchor: Option<SharedString>,
}
impl EventEmitter<ChatAction> for ChatPanel {}
impl ChatPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextareaState::new(window, cx).placeholder("输入消息…").rows(3));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索"));
        let editor = cx.new(|cx| TextareaState::new(window, cx).rows(8));
        let subscriptions = vec![
            cx.subscribe(&input, |this, state, event, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = state.read(cx).value();
                    if value != this.draft && this.unlocked && !this.external_busy {
                        this.draft = value.clone();
                        if this.conversation.is_none() { cx.emit(ChatAction::New); }
                        cx.emit(ChatAction::Draft(value.to_string()));
                        cx.notify();
                    }
                }
            }),
            cx.subscribe(&search, |_, _, event, cx| { if matches!(event, InputEvent::Change) { cx.notify(); } }),
        ];
        Self { input, search, editor, _subscriptions: subscriptions,
            conversation: None, draft: "".into(), external_busy: false, unlocked: true, valid_model: false,
            expanded: false, overlay: None, delete: None, edit: None, details: HashSet::new(),
            focus: cx.focus_handle(), scroll: ScrollHandle::new(), follow: true,
            markdown: MarkdownCache::default(), media: MediaPreview::default(), anchors: Anchors::default(), anchor: None }
    }
    /// Disable writes while the host owns an asynchronous file picker.
    pub fn set_external_busy(&mut self, busy: bool) { self.external_busy = busy; }
    /// `trigger` names the button that opened the layer; a layer opened from inside a menu keeps that menu's anchor.
    fn open(&mut self, overlay: Overlay, trigger: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if self.anchors.borrow().contains_key(&trigger) { self.anchor = Some(trigger); }
        if matches!(overlay, Overlay::History | Overlay::Models(_) | Overlay::RetryModels) {
            self.search.update(cx, |state, cx| { state.set_value("", window, cx); state.focus(window, cx); });
        } else { self.focus.focus(window, cx); }
        self.overlay = Some(overlay); cx.notify();
    }
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = None; self.delete = None; self.edit = None;
        self.input.update(cx, |state, cx| state.focus(window, cx)); cx.notify();
    }
    fn emit(&mut self, action: ChatAction, cx: &mut Context<Self>) {
        if matches!(action, ChatAction::Send | ChatAction::Retry | ChatAction::RetryWith(_)) {
            self.follow = true; self.scroll.scroll_to_bottom();
        }
        if matches!(action, ChatAction::Delete(_)) { self.delete = None; }
        if matches!(action, ChatAction::New | ChatAction::Select(_) | ChatAction::DeleteAttempt { .. }
            | ChatAction::ForkAttempt { .. } | ChatAction::CopyConversation { .. } | ChatAction::OpenSettings
            | ChatAction::UploadAttachment) {
            self.overlay = None;
        }
        cx.emit(action); cx.notify();
    }
    fn action(&self, id: impl Into<ElementId>, label: impl Into<SharedString>, enabled: bool,
        action: ChatAction, cx: &Context<Self>) -> Button {
        Button::new(id).small().ghost().label(label).disabled(!enabled)
            .on_click(cx.listener(move |this, _, _, cx| this.emit(action.clone(), cx)))
    }
    fn layer_button(&self, id: impl Into<SharedString>, label: impl Into<SharedString>, enabled: bool,
        layer: Overlay, cx: &Context<Self>) -> Button {
        let key = id.into();
        Button::new(key.clone()).small().ghost().label(label).disabled(!enabled)
            .on_click(cx.listener(move |this, _, window, cx| this.open(layer.clone(), key.clone(), window, cx)))
    }
    fn icon_action(&self, id: impl Into<ElementId>, icon: IconName, label: &'static str,
        enabled: bool, action: ChatAction, cx: &Context<Self>) -> Button {
        icon_button(id, icon, label).disabled(!enabled)
            .on_click(cx.listener(move |this, _, _, cx| this.emit(action.clone(), cx)))
    }
    fn icon_layer(&self, id: impl Into<SharedString>, icon: IconName, label: &'static str,
        enabled: bool, layer: Overlay, cx: &Context<Self>) -> Button {
        let key = id.into();
        icon_button(key.clone(), icon, label).disabled(!enabled)
            .on_click(cx.listener(move |this, _, window, cx| this.open(layer.clone(), key.clone(), window, cx)))
    }
    /// Wrap a menu trigger so its bounds are known when the menu opens. `key` MUST equal the trigger's id.
    fn tracked(&self, key: impl Into<SharedString>, trigger: Button) -> Div {
        let (key, anchors) = (key.into(), self.anchors.clone());
        div().on_children_prepainted(move |bounds, _, _| {
            if let Some(bounds) = bounds.first() { anchors.borrow_mut().insert(key.clone(), *bounds); }
        }).child(trigger)
    }
    /// A left-aligned toggle row; the chevron, not a filled background, shows whether it is open.
    fn disclosure(&self, key: String, label: impl Into<SharedString>, cx: &Context<Self>) -> Div {
        let open = self.details.contains(&key);
        row().child(Button::new(SharedString::from(key.clone())).small().ghost().label(label)
            .icon(if open { IconName::ChevronDown } else { IconName::ChevronRight })
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.details.remove(&key) { this.details.insert(key.clone()); } cx.notify();
            })))
    }

    /// Host calls `panel.update(cx, |panel, cx| panel.render_session(...))` and observes this entity.
    pub fn render_session(&mut self, session: &ChatSession, options: &[ModelOption], image_options: &[ModelOption],
        model_settings: Option<&ModelSettings>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let selected = session.selected();
        let id = selected.map(|c| c.id.as_str());
        if self.conversation.as_deref() != id {
            self.conversation = id.map(str::to_owned); self.details.clear(); self.markdown.clear(); self.anchors.borrow_mut().clear();
            self.edit = None; self.overlay = None; self.delete = None; self.follow = true;
        }
        let source_draft = selected.map_or("", |c| c.draft.as_str());
        if self.draft.as_ref() != source_draft {
            self.draft = source_draft.to_owned().into();
            self.input.update(cx, |state, cx| state.set_value(source_draft, window, cx));
        }
        self.unlocked = !session.busy() && !session.save_failed() && !self.external_busy;
        let picked = session.history().model.as_ref();
        let valid_chat = picked.is_some_and(|m| options.iter().any(|o| same_model(m, &o.model)));
        let tools_supported = model_settings.is_none_or(|s| s.supports_tools != Some(false));
        let composer = session.composer();
        let valid_image = composer.image_model.as_ref().is_some_and(|m| image_options.iter().any(|o| same_model(m, &o.model)));
        self.valid_model = valid_chat && (!composer.image_mode || (valid_image && tools_supported));
        let model_error = if options.is_empty() { "尚未配置聊天模型，请打开模型设置。" }
            else if !valid_chat { "请选择有效聊天模型。" }
            else if composer.image_mode && !tools_supported { "当前聊天模型不支持工具调用，请更换聊天模型或关闭生图。" }
            else if composer.image_mode && !valid_image { "请选择有效生图模型，或关闭生图。" } else { "" };
        let r = LumaTheme::global(cx).roles;
        let header = row().flex_nowrap().justify_between().px(space::S12).py(space::S8)
            .bg(r.container_header_bg).border_b_1().border_color(r.border_muted)
            .child(div().flex_1().min_w_0().truncate().text_style(text::TITLE_SECTION)
                .child(selected.map_or("新对话", |c| c.title.as_str()).to_owned()))
            .child(self.tracked("history", self.icon_layer("history", IconName::Menu, "历史对话", self.unlocked, Overlay::History, cx)))
            .child(self.icon_action("settings", IconName::Settings, "模型设置", true, ChatAction::OpenSettings, cx));
        // The transcript and composer share one centered column so wide windows keep a readable line length.
        let mut transcript = col().w_full().max_w(size::CONTENT_MAX).gap(space::S16);
        if let Some(conversation) = selected {
            for (index, turn) in conversation.turns.iter().enumerate() {
                let mut question = col().gap(space::S4).px(space::S12).py(space::S8).rounded(radius::LG)
                    .bg(r.bubble_user_bg).border_1().border_color(r.bubble_user_border).max_w(relative(0.85))
                    .text_style(text::READING).text_color(r.text_default).child(turn.input.clone());
                for attachment in session.turn_attachments(&turn.id) {
                    question = question.child(div().text_style(text::METADATA).text_color(r.text_muted).child(format!("附件 · {}", attachment.name)));
                }
                let mut group = col().gap(space::S12).child(row().justify_end().child(question));
                if let Some((last, previous)) = turn.attempts.split_last() {
                    if !previous.is_empty() {
                        let key = format!("previous:{}", turn.id);
                        group = group.child(self.disclosure(key.clone(), format!("之前的 {} 次回答", previous.len()), cx));
                        if self.details.contains(&key) {
                            for attempt in previous {
                                group = group.child(self.answer(session, &conversation.id, &turn.id, attempt,
                                    turn.selected_attempt.as_deref() == Some(attempt.id.as_str()), false, options, image_options, cx));
                            }
                        }
                    }
                    group = group.child(self.answer(session, &conversation.id, &turn.id, last,
                        turn.selected_attempt.as_deref().unwrap_or(&last.id) == last.id,
                        index + 1 == conversation.turns.len(), options, image_options, cx));
                }
                transcript = transcript.child(group);
            }
            if conversation.turns.is_empty() { transcript = transcript.child(div().text_style(text::METADATA).text_color(r.text_muted).child("开始一个新对话")); }
        } else { transcript = transcript.child(div().text_style(text::METADATA).text_color(r.text_muted).child("输入消息，开始一个新对话")); }
        if self.follow { self.scroll.scroll_to_bottom(); }

        let mut footer = col().w_full().max_w(size::CONTENT_MAX).gap(space::S8);
        if session.save_failed() || !session.notice().is_empty() || !model_error.is_empty() {
            let mut notice = div().id("chat-notice").max_h(NOTICE_MAX).overflow_y_scroll().text_style(text::METADATA).text_color(r.feedback_error);
            if session.save_failed() { notice = notice.child(div().child("保存失败，恢复保存前不能修改聊天。")); }
            if !model_error.is_empty() { notice = notice.child(div().child(model_error)); }
            if !session.notice().is_empty() { notice = notice.child(div().child(session.notice().to_owned())); }
            footer = footer.child(notice);
        }
        let mut attachments = row();
        for attachment in session.draft_attachments() {
            attachments = attachments.child(self.action(SharedString::from(format!("attachment:{}", attachment.id)),
                attachment.name.clone(), self.unlocked, ChatAction::RemoveAttachment(attachment.id.clone()), cx)
                .icon(IconName::X).accessibility_label(format!("移除附件 {}", attachment.name)).tooltip(format!("移除附件 · {} 字节", attachment.bytes)));
        }
        if !session.draft_attachments().is_empty() { footer = footer.child(attachments); }
        let height = if self.expanded { (f32::from(window.viewport_size().height) * 0.35).clamp(COMPOSER_EXPANDED.0, COMPOSER_EXPANDED.1) } else { COMPOSER_HEIGHT };
        // Kit's Textarea binds Ctrl/⌘+Enter to `Enter { secondary: true }`, and GPUI dispatches bindings
        // before raw key listeners, so the shortcut is intercepted as that action in the capture phase.
        let editor = div().capture_action(cx.listener(|this, action: &Enter, window, cx| {
            if !action.secondary || action.shift { return; }
            // The input exposes the platform's real IME marked range, not a guessed key sequence.
            let composing = this.input.update(cx, |state, cx| state.marked_text_range(window, cx).is_some());
            if composing { return; }
            cx.stop_propagation();
            if this.unlocked && this.valid_model && this.overlay.is_none() && !this.draft.trim().is_empty() {
                this.emit(ChatAction::Send, cx);
            }
        })).child(Textarea::new(&self.input).aria_label("消息输入；Enter 换行，Ctrl+Enter 发送")
            .disabled(!self.unlocked).appearance(false).h(px(height)));
        let mut toolbar = row().gap(space::S4);
        for &tool in session.toolbar() {
            toolbar = toolbar.child(self.tool_button(tool, composer, valid_chat && tools_supported && valid_image, cx));
        }
        toolbar = toolbar.child(self.tracked("tools", self.icon_layer("tools", IconName::Plus, "添加附件与输入工具", self.unlocked, Overlay::Tools, cx)));
        if composer.image_mode {
            let mut next = composer.clone(); next.image_mode = false;
            toolbar = toolbar.child(self.action("remove-image-mode", "生成图片", self.unlocked, ChatAction::SetComposer(next), cx)
                .icon(IconName::X).accessibility_label("关闭生成图片"));
        }
        let model_label = picked.and_then(|m| options.iter().find(|o| same_model(m, &o.model))).map_or("选择模型", |o| o.name.as_str());
        toolbar = toolbar.child(self.tracked("model", self.layer_button("model", model_label.to_owned(), self.unlocked, Overlay::Models(false), cx)
            .max_w(MODEL_BUTTON_MAX).tooltip("选择聊天模型")))
            .child(self.tracked("reasoning", self.layer_button("reasoning", reasoning_label(composer.reasoning), self.unlocked, Overlay::Reasoning, cx).tooltip("推理强度")))
            .child(self.tracked("usage", self.layer_button("usage", "用量", true, Overlay::Usage, cx)));
        if !self.follow { toolbar = toolbar.child(icon_button("bottom", IconName::ArrowDown, "回到底部")
            .on_click(cx.listener(|this, _, _, cx| { this.follow = true; this.scroll.scroll_to_bottom(); cx.notify(); }))); }
        if session.save_failed() { toolbar = toolbar.child(self.icon_action("save", IconName::RefreshCw, "重试保存", !session.busy(), ChatAction::Save, cx)); }
        toolbar = toolbar.child(div().flex_1())
            .child(if session.busy() { self.icon_action("stop", IconName::Square, "停止生成", true, ChatAction::Stop, cx) }
                else { self.icon_action("send", IconName::ArrowUp, "发送消息 · Ctrl+Enter", self.unlocked && self.valid_model && !self.draft.trim().is_empty(), ChatAction::Send, cx).primary() });
        // Text area and toolbar share one raised card, as in the reference composer.
        footer = footer.child(col().gap(space::S4).p(space::S8).rounded(radius::LG).bg(r.control_bg)
            .border_1().border_color(r.border_default).child(editor).child(toolbar));
        let transcript = div().id("transcript").flex_1().min_h_0().min_w_0().overflow_y_scroll().track_scroll(&self.scroll)
            .flex().flex_col().items_center().px(space::S16).py(space::S16).child(transcript)
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                let delta = event.delta.pixel_delta(SCROLL_LINE);
                if delta.y > px(0.) { this.follow = false; }
                else if this.scroll.offset().y.abs() + FOLLOW_SLOP >= this.scroll.max_offset().y { this.follow = true; }
                cx.notify();
            }));
        let footer = div().flex_shrink_0().flex().flex_col().items_center().p(space::S12).border_t_1().border_color(r.border_muted).child(footer);
        let mut root = col().id(("chat-panel", cx.entity_id())).relative().size_full().min_h_0().min_w_0().text_style(text::UI)
            .text_color(r.text_default).bg(r.container_workspace_bg).child(header).child(transcript).child(footer);
        if let Some(overlay) = self.overlay.clone() {
            root = root.child(self.render_overlay(overlay, session, options, image_options, model_settings, valid_chat && tools_supported, window, cx));
        }
        root.into_any_element()
    }

    fn tool_button(&self, tool: ToolbarTool, composer: &ComposerSettings, can_image: bool, cx: &Context<Self>) -> Button {
        let (label, action, enabled, selected) = match tool {
            ToolbarTool::NewConversation => ("新对话", ChatAction::New, self.unlocked, false),
            ToolbarTool::Attachment => ("附件", ChatAction::UploadAttachment, self.unlocked, false),
            ToolbarTool::WebSearch => { let mut next = composer.clone(); next.web_search = !next.web_search;
                ("联网", ChatAction::SetComposer(next), self.unlocked, composer.web_search) },
            ToolbarTool::GenerateImage => { let mut next = composer.clone(); next.image_mode = !next.image_mode;
                ("生图", ChatAction::SetComposer(next), self.unlocked && (composer.image_mode || can_image), composer.image_mode) },
        };
        let icon = match tool { ToolbarTool::NewConversation => IconName::MessageSquarePlus,
            ToolbarTool::Attachment => IconName::Paperclip, ToolbarTool::WebSearch => IconName::Globe,
            ToolbarTool::GenerateImage => IconName::Image };
        self.icon_action(label, icon, label, enabled, action, cx).selected(selected)
    }

    fn answer(&mut self, session: &ChatSession, conversation: &str, turn: &str, attempt: &Attempt,
        chosen: bool, retry: bool, options: &[ModelOption], image_options: &[ModelOption], cx: &mut Context<Self>) -> AnyElement {
        let r = LumaTheme::global(cx).roles;
        // The chosen attempt keeps a stronger outline; the selected-context button carries the state.
        // READING on the card is what the Markdown answer inherits; other children set their own style.
        let mut body = col().id(SharedString::from(attempt.id.clone())).gap(space::S8).p(space::S12).rounded(radius::LG)
            .text_style(text::READING).bg(r.container_elevated_bg).border_1().border_color(if chosen { r.border_default } else { r.border_muted });
        let source = options.iter().chain(image_options).find(|o| same_model(&o.model, &attempt.model));
        let source_detail = source.map(model_label).unwrap_or_else(|| format!("{} · {} / {}", attempt.model.model, attempt.model.provider, attempt.model.endpoint));
        body = body.child(div().id(SharedString::from(format!("source:{}", attempt.id))).text_style(text::METADATA).text_color(r.text_muted)
            .tooltip(move |window, cx| Tooltip::new(source_detail.clone()).build(window, cx))
            .child(source.map_or(&attempt.model.model, |option| &option.name).clone()));
        if attempt.search.status != SearchStatus::Idle {
            let label = match attempt.search.status { SearchStatus::Searching => "正在搜索…", SearchStatus::Complete => "搜索来源",
                SearchStatus::Failed => "搜索失败", SearchStatus::Cancelled => "搜索已取消", SearchStatus::Idle => "" };
            let key = format!("sources:{}", attempt.id);
            body = body.child(self.disclosure(key.clone(), label, cx));
            if self.details.contains(&key) {
                for (index, hit) in attempt.search.hits.iter().enumerate() {
                    let url = hit.url.clone();
                    body = body.child(row().child(Button::new(SharedString::from(format!("source:{}:{index}", attempt.id))).small().link()
                        .icon(IconName::ExternalLink).label(hit.title.clone()).tooltip(hit.snippet.clone()).disabled(!web_link(&url))
                        .on_click(move |_, _, cx| { if web_link(&url) { cx.open_url(&url); } })));
                }
                if let Some(error) = &attempt.search.error { body = body.child(div().text_style(text::METADATA).text_color(r.feedback_error).child(error.clone())); }
            }
        }
        if !attempt.reasoning.is_empty() {
            let key = format!("reasoning:{}", attempt.id);
            body = body.child(self.disclosure(key.clone(), "思考过程", cx));
            if self.details.contains(&key) {
                let mut reasoning = div().id(SharedString::from(key)).max_h(DETAIL_MAX).overflow_y_scroll().text_style(text::METADATA).text_color(r.text_muted);
                for block in &attempt.reasoning { reasoning = reasoning.child(div().child(block.get("text").or_else(|| block.get("thinking"))
                    .and_then(|v| v.as_str()).map_or_else(|| block.to_string(), str::to_owned))); }
                body = body.child(reasoning);
            }
        }
        for event in &attempt.tool_events {
            let key = format!("tool:{}:{}", attempt.id, event.id);
            let name = if event.name == "generate_image" { "图片生成" } else { &event.name };
            let status = if event.is_error { "失败，请重试或更换模型" } else if event.result.is_some() { "完成" } else { "进行中…" };
            body = body.child(self.disclosure(key.clone(), format!("{name}{status}"), cx));
            if self.details.contains(&key) {
                let mut details = div().id(SharedString::from(key)).max_h(DETAIL_MAX).overflow_y_scroll().text_style(text::METADATA)
                    .child(format!("工具：{}\n调用 ID：{}\n参数：{}", event.name, event.id, event.arguments));
                if let Some(result) = &event.result { details = details.child(div().child(result.clone())); }
                if let Some(error) = &event.error { details = details.child(div().text_color(r.feedback_error).child(error.clone())); }
                body = body.child(details);
            }
        }
        let mut images = row().gap(space::S8);
        for image in &attempt.generated {
            images = images.child(match self.media.get(session, image) {
                Ok(preview) => {
                    let id = image.id.clone();
                    div().id(SharedString::from(format!("image:{}", id))).w(IMAGE_W).max_w_full().h(IMAGE_H)
                        .cursor_pointer().aria_label("生成图片，点击打开")
                        .on_click(cx.listener(move |this, _, _, cx| this.emit(ChatAction::OpenMedia(id.clone()), cx)))
                        .child(img(preview).size_full().object_fit(ObjectFit::Contain)).into_any_element()
                }
                Err(error) => div().text_style(text::METADATA).text_color(r.feedback_error).child(error).into_any_element(),
            });
        }
        if !attempt.generated.is_empty() { body = body.child(images); }
        let (text_view, raw) = self.markdown.view(&attempt.id, &attempt.text);
        if !attempt.text.is_empty() { body = body.child(text_view); }
        let status = match attempt.status { TurnStatus::Running if attempt.text.is_empty() => "正在等待回复…",
            TurnStatus::Running => "正在生成…", TurnStatus::Cancelled => "已停止，保留部分回答",
            TurnStatus::Failed => "回答失败，请重试或更换模型", _ => "" };
        if !status.is_empty() { body = body.child(div().text_style(text::METADATA).text_color(r.text_muted).child(status)); }
        if let Some(error) = &attempt.error {
            let key = format!("error:{}", attempt.id);
            body = body.child(self.disclosure(key.clone(), "诊断详情", cx));
            if self.details.contains(&key) { body = body.child(div().id(SharedString::from(key)).max_h(DETAIL_MAX).overflow_y_scroll().text_style(text::METADATA).child(error.clone())); }
        }
        let key = format!("usage:{}", attempt.id);
        body = body.child(self.disclosure(key.clone(), "用量详情", cx));
        if self.details.contains(&key) { body = body.child(usage_details(attempt)); }
        let mut actions = row().gap(space::S4).child(icon_button(SharedString::from(format!("copy:{}", attempt.id)), IconName::Copy, "复制原文")
            .disabled(raw.is_empty()).on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(raw.to_string()))));
        if retry {
            let available = options.iter().chain(image_options).any(|o| same_model(&o.model, &attempt.model));
            actions = actions.child(self.icon_action(SharedString::from(format!("retry:{}", attempt.id)), IconName::RefreshCw, "重试 / 重新生成", self.unlocked && available, ChatAction::Retry, cx));
            let image_attempt = !options.iter().any(|o| same_model(&o.model, &attempt.model))
                && !attempt.tool_events.iter().any(|e| e.name == "generate_image")
                && (!attempt.generated.is_empty() || image_options.iter().any(|o| same_model(&o.model, &attempt.model)));
            if !image_attempt { actions = actions.child(self.tracked("retry-model", self.icon_layer("retry-model", IconName::Bot, "换模型重答", self.unlocked && !options.is_empty(), Overlay::RetryModels, cx))); }
        }
        actions = actions.child(self.icon_action(SharedString::from(format!("select:{}", attempt.id)), if chosen { IconName::Check } else { IconName::Menu }, if chosen { "已选上下文" } else { "选择上下文" },
            self.unlocked && attempt.status == TurnStatus::Completed, ChatAction::SelectAttempt {
                conversation_id: conversation.into(), turn_id: turn.into(), attempt_id: attempt.id.clone() }, cx).selected(chosen))
            .child(self.tracked(format!("more:{}", attempt.id), self.icon_layer(format!("more:{}", attempt.id), IconName::Ellipsis, "更多操作", true,
                Overlay::More(conversation.into(), turn.into(), attempt.id.clone()), cx)));
        body.child(actions).into_any_element()
    }

    fn render_overlay(&mut self, layer: Overlay, session: &ChatSession, options: &[ModelOption], image_options: &[ModelOption],
        settings: Option<&ModelSettings>, can_image: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let r = LumaTheme::global(cx).roles;
        let mut content = col().gap(space::S8);
        let menu = !matches!(layer, Overlay::Edit);
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
            Overlay::Tools => {
                content = content.child("添加附件与输入工具");
                let valid_image = session.composer().image_model.as_ref().is_some_and(|m| image_options.iter().any(|o| same_model(m, &o.model)));
                for &(tool, label) in TOOLS { if !session.toolbar().contains(&tool) {
                    content = content.child(item(self.tool_button(tool, session.composer(), can_image && valid_image, cx), label));
                }}
                content = content.child(self.menu_layer("image-model", "选择生图模型", self.unlocked && can_image && !image_options.is_empty(), Overlay::Models(true), cx))
                    .child(self.menu_layer("customize", "快捷面板 / 自定义工具栏", self.unlocked, Overlay::Quick, cx))
                    .child(item(Button::new("expand"), if self.expanded { "收起输入" } else { "展开输入" })
                        .on_click(cx.listener(|this, _, window, cx| { this.expanded = !this.expanded; this.close(window, cx); })))
                    .child(div().text_style(text::METADATA).text_color(r.text_muted).child("Ctrl / ⌘ + Enter 发送；Enter 换行"))
                    .child(div().text_style(text::METADATA).text_color(r.text_muted).child("最多 4 个附件；UTF-8 文本合计 1 MiB，图片每张 4 MiB。"));
                if !can_image || image_options.is_empty() { content = content.child("生图需要支持工具调用的聊天模型及已配置的生图模型。"); }
            }
            Overlay::Quick => {
                content = content.child("自定义工具栏").child("只改变底栏显示，不执行工具或改变请求设置。");
                for &(tool, label) in TOOLS {
                    let selected = session.toolbar().contains(&tool);
                    let pending = TOOLS.iter().map(|(t, _)| *t).filter(|t| if *t == tool { !selected } else { session.toolbar().contains(t) }).collect();
                    content = content.child(self.menu_action(label, label, self.unlocked, ChatAction::SetToolbar(pending), cx).selected(selected));
                }
                content = content.child(self.menu_action("reset-toolbar", "恢复默认", self.unlocked,
                    ChatAction::SetToolbar(vec![ToolbarTool::NewConversation, ToolbarTool::WebSearch]), cx));
            }
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
        let (surface_w, surface_h) = match trigger {
            Some(t) => (f32::from(LAYER_W).min(width - 2. * MENU_MARGIN),
                if below { height - f32::from(t.bottom()) } else { f32::from(t.top()) } - MENU_MARGIN - f32::from(MENU_GAP)),
            None => (f32::from(LAYER_W), height - LAYER_MARGIN),
        };
        let surface = col().id("chat-layer").occlude().w(px(surface_w)).when(trigger.is_none(), |s| s.max_w_full())
            .max_h(px(surface_h.max(LAYER_MIN_HEIGHT))).p(space::S12).gap(space::S8).rounded(radius::LG)
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
            .child(row().justify_end().child(Button::new("close-layer").small().ghost().label(if self.edit.is_some() { "取消" } else { "关闭" })
                .on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))))
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
    fn menu_layer(&self, id: &'static str, label: &'static str, enabled: bool, layer: Overlay, cx: &Context<Self>) -> Button {
        item(Button::new(id), label).disabled(!enabled)
            .on_click(cx.listener(move |this, _, window, cx| this.open(layer.clone(), id.into(), window, cx)))
    }
    fn menu_action(&self, id: &'static str, label: &'static str, enabled: bool, action: ChatAction, cx: &Context<Self>) -> Button {
        item(Button::new(id), label).disabled(!enabled).on_click(cx.listener(move |this, _, _, cx| this.emit(action.clone(), cx)))
    }
}
fn row() -> Div { div().flex().items_center().flex_wrap().gap(space::S8).min_w_0() }
fn col() -> Div { div().flex().flex_col().min_w_0() }
fn icon_button(id: impl Into<ElementId>, icon: IconName, label: &'static str) -> Button {
    Button::new(id).small().ghost().icon(icon).accessibility_label(label).tooltip(label)
}
/// A full-width menu row: Kit buttons center their content, so the label fills the row and starts left.
fn item(button: Button, label: impl Into<SharedString>) -> Button {
    let label = label.into();
    button.small().ghost().w_full().accessibility_label(label.clone()).child(div().w_full().min_w_0().truncate().child(label))
}
const TOOLS: &[(ToolbarTool, &str)] = &[(ToolbarTool::NewConversation, "新对话"), (ToolbarTool::WebSearch, "网络搜索"),
    (ToolbarTool::Attachment, "上传附件"), (ToolbarTool::GenerateImage, "生成图片")];
fn same_model(a: &ModelRef, b: &ModelRef) -> bool { a.provider == b.provider && a.endpoint == b.endpoint && a.model == b.model && a.protocol == b.protocol }
fn model_label(option: &ModelOption) -> String { format!("{} · {} / {}", option.name, option.provider, option.endpoint) }
fn reasoning_label(effort: Option<ReasoningEffort>) -> &'static str {
    match effort { None => "默认", Some(ReasoningEffort::None) => "关闭", Some(ReasoningEffort::Auto) => "自动",
        Some(ReasoningEffort::Minimal) => "最小", Some(ReasoningEffort::Low) => "低", Some(ReasoningEffort::Medium) => "中",
        Some(ReasoningEffort::High) => "高", Some(ReasoningEffort::Xhigh) => "超高", Some(ReasoningEffort::Max) => "最大" }
}
fn usage_details(attempt: &Attempt) -> Div {
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

/// Call on the host's file-picker worker, never during render.
pub fn pick_attachment() -> Result<Option<std::path::PathBuf>, String> {
    #[cfg(windows)] {
        let script = "[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); Add-Type -AssemblyName System.Windows.Forms; $d=[System.Windows.Forms.OpenFileDialog]::new(); $d.Title='上传附件'; $d.Multiselect=$false; if($d.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK){[Console]::Write($d.FileName)}; $d.Dispose()";
        let output = std::process::Command::new("powershell.exe").args(["-NoProfile", "-STA", "-WindowStyle", "Hidden", "-Command", script]).output().map_err(|e| format!("无法打开附件选择器：{e}"))?;
        if !output.status.success() { return Err(format!("附件选择器失败：{}", String::from_utf8_lossy(&output.stderr))); }
        let path = String::from_utf8(output.stdout).map_err(|_| "附件选择器返回了无效路径编码".to_owned())?;
        Ok((!path.is_empty()).then(|| std::path::PathBuf::from(path)))
    }
    #[cfg(not(windows))] { Err("当前宿主的附件选择器仅支持 Windows".into()) }
}
/// The host must resolve this path through ChatSession::media_path after an explicit click.
pub fn open_media(path: &std::path::Path) -> Result<(), String> {
    if !path.is_file() { return Err("图片文件不存在".into()); }
    #[cfg(windows)] let result = std::process::Command::new("explorer.exe").arg(path).spawn();
    #[cfg(target_os = "macos")] let result = std::process::Command::new("open").arg(path).spawn();
    #[cfg(not(any(windows, target_os = "macos")))] let result = std::process::Command::new("xdg-open").arg(path).spawn();
    result.map(|_| ()).map_err(|e| format!("无法打开图片：{e}"))
}
