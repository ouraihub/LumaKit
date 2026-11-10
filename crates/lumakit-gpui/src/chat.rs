//! Stateful GPUI rendering over a borrowed ChatSession; emits existing business actions.
use crate::{
    markdown::{MarkdownCache, web_link},
    media::MediaPreview,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    Disableable, FocusTrapElement, Selectable, Sizable,
    button::{Button, ButtonVariants},
    input::{Enter, Input, InputEvent, InputState, Textarea, TextareaState},
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use lumakit_ai::chat::{ChatAction, ModelOption};
use lumakit_ai::{
    ModelRef,
    chat::{Attempt, ChatSession, ComposerSettings, SearchStatus, ToolbarTool},
    history::TurnStatus,
    model_config::{ModelSettings, ReasoningEffort},
};
use lumakit_design::{
    LumaTheme,
    components::{TextStyled as _, label},
    metrics::{radius, size, space, text},
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

mod diagnostics;
mod model_picker;
mod overlays;
mod toolbar;
mod usage;
pub use usage::usage_totals;
use usage::{context_summary, usage_details};

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
/// Answer editor height.
const EDITOR_HEIGHT: Pixels = px(220.);
/// Pixel distance of one wheel line when converting line-based scroll deltas.
const SCROLL_LINE: Pixels = px(20.);

#[derive(Clone)]
enum Overlay {
    History,
    Tools,
    Quick,
    ImageModels,
    Usage,
    More(String, String, String),
    RetryModels,
    Edit,
}
struct Edit {
    conversation: String,
    turn: String,
    attempt: String,
    expected: SharedString,
}
struct AnswerContext<'a> {
    session: &'a ChatSession,
    conversation: &'a str,
    turn: &'a str,
    options: &'a [ModelOption],
    image_options: &'a [ModelOption],
}
/// Last laid-out bounds of each menu trigger, written during prepaint so a menu opens beside its button.
type Anchors = Rc<RefCell<HashMap<SharedString, Bounds<Pixels>>>>;

/// Per-instance inputs, navigation layers and bounded display caches. No credentials or history copy.
pub struct ChatPanel {
    input: Entity<TextareaState>,
    search: Entity<InputState>,
    editor: Entity<TextareaState>,
    _subscriptions: Vec<Subscription>,
    conversation: Option<String>,
    draft: SharedString,
    external_busy: bool,
    unlocked: bool,
    valid_model: bool,
    expanded: bool,
    overlay: Option<Overlay>,
    image_notice: &'static str,
    delete: Option<String>,
    edit: Option<Edit>,
    details: HashSet<String>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    follow: bool,
    markdown: MarkdownCache,
    media: MediaPreview,
    anchors: Anchors,
    anchor: Option<SharedString>,
    picker: model_picker::PickerState,
    usage_status: Entity<lumakit_usage_status::ContextStatusView>,
}
impl EventEmitter<ChatAction> for ChatPanel {}
impl ChatPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("输入消息…")
                .rows(3)
        });
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索"));
        let editor = cx.new(|cx| TextareaState::new(window, cx).rows(8));
        let mut subscriptions = vec![
            cx.subscribe(&input, |this, state, event, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = state.read(cx).value();
                    if value != this.draft && this.unlocked && !this.external_busy {
                        this.draft = value.clone();
                        cx.emit(ChatAction::Draft(value.to_string()));
                        cx.notify();
                    }
                }
            }),
            cx.subscribe(&search, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        let (picker, subscription) = model_picker::PickerState::new(window, cx);
        subscriptions.push(subscription);
        let usage_status = cx.new(|cx| lumakit_usage_status::ContextStatusView::new(window, cx));
        Self {
            input,
            search,
            editor,
            _subscriptions: subscriptions,
            conversation: None,
            draft: "".into(),
            external_busy: false,
            unlocked: true,
            valid_model: false,
            expanded: false,
            overlay: None,
            delete: None,
            edit: None,
            details: HashSet::new(),
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            follow: true,
            markdown: MarkdownCache::default(),
            media: MediaPreview::default(),
            anchors: Anchors::default(),
            anchor: None,
            image_notice: "请先选择生图模型",
            picker,
            usage_status,
        }
    }
    /// Disable writes while the host owns an asynchronous file picker.
    pub fn set_external_busy(&mut self, busy: bool) {
        self.external_busy = busy;
    }
    /// Update the host-cached usage snapshot without database access during rendering.
    pub fn set_usage_status(
        &mut self,
        status: lumakit_usage_status::ContextStatus,
        cx: &mut Context<Self>,
    ) {
        self.usage_status
            .update(cx, |view, cx| view.set_status(status, cx));
    }
    /// `trigger` names the button that opened the layer; a layer opened from inside a menu keeps that menu's anchor.
    fn open(
        &mut self,
        overlay: Overlay,
        trigger: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.anchors.borrow().contains_key(&trigger) {
            self.anchor = Some(trigger);
        }
        if matches!(
            overlay,
            Overlay::History | Overlay::ImageModels | Overlay::RetryModels
        ) {
            self.search.update(cx, |state, cx| {
                state.set_value("", window, cx);
                state.focus(window, cx);
            });
        } else {
            self.focus.focus(window, cx);
        }
        self.overlay = Some(overlay);
        cx.notify();
    }
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = None;
        self.delete = None;
        self.edit = None;
        self.input.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }
    fn emit(&mut self, action: ChatAction, cx: &mut Context<Self>) {
        if matches!(
            action,
            ChatAction::Send | ChatAction::Retry | ChatAction::RetryWith(_)
        ) {
            self.follow = true;
            self.scroll.scroll_to_bottom();
        }
        if matches!(action, ChatAction::Delete(_)) {
            self.delete = None;
        }
        if matches!(
            action,
            ChatAction::New
                | ChatAction::Select(_)
                | ChatAction::DeleteAttempt { .. }
                | ChatAction::ForkAttempt { .. }
                | ChatAction::CopyConversation { .. }
                | ChatAction::OpenSettings
                | ChatAction::UploadAttachment
        ) {
            self.overlay = None;
        }
        cx.emit(action);
        cx.notify();
    }
    fn action(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        enabled: bool,
        action: ChatAction,
        cx: &Context<Self>,
    ) -> Button {
        Button::new(id)
            .small()
            .ghost()
            .label(label)
            .disabled(!enabled)
            .on_click(cx.listener(move |this, _, _, cx| this.emit(action.clone(), cx)))
    }
    fn layer_button(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        enabled: bool,
        layer: Overlay,
        cx: &Context<Self>,
    ) -> Button {
        let key = id.into();
        Button::new(key.clone())
            .small()
            .ghost()
            .label(label)
            .disabled(!enabled)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open(layer.clone(), key.clone(), window, cx)
            }))
    }
    fn icon_action(
        &self,
        id: impl Into<ElementId>,
        icon: IconName,
        label: &'static str,
        enabled: bool,
        action: ChatAction,
        cx: &Context<Self>,
    ) -> Button {
        icon_button(id, icon, label)
            .disabled(!enabled)
            .on_click(cx.listener(move |this, _, _, cx| this.emit(action.clone(), cx)))
    }
    fn icon_layer(
        &self,
        id: impl Into<SharedString>,
        icon: IconName,
        label: &'static str,
        enabled: bool,
        layer: Overlay,
        cx: &Context<Self>,
    ) -> Button {
        let key = id.into();
        icon_button(key.clone(), icon, label)
            .disabled(!enabled)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open(layer.clone(), key.clone(), window, cx)
            }))
    }
    /// Wrap a menu trigger so its bounds are known when the menu opens. `key` MUST equal the trigger's id.
    fn tracked(&self, key: impl Into<SharedString>, trigger: Button) -> Div {
        let (key, anchors) = (key.into(), self.anchors.clone());
        div()
            .on_children_prepainted(move |bounds, _, _| {
                if let Some(bounds) = bounds.first() {
                    anchors.borrow_mut().insert(key.clone(), *bounds);
                }
            })
            .child(trigger)
    }
    /// A left-aligned toggle row; the chevron, not a filled background, shows whether it is open.
    fn disclosure(&self, key: String, label: impl Into<SharedString>, cx: &Context<Self>) -> Div {
        let open = self.details.contains(&key);
        row().child(
            Button::new(SharedString::from(key.clone()))
                .small()
                .ghost()
                .label(label)
                .toggled(open)
                .icon(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.details.remove(&key) {
                        this.details.insert(key.clone());
                    }
                    cx.notify();
                })),
        )
    }

    /// Host calls `panel.update(cx, |panel, cx| panel.render_session(...))` and observes this entity.
    pub fn render_session(
        &mut self,
        session: &ChatSession,
        options: &[ModelOption],
        image_options: &[ModelOption],
        model_settings: Option<&ModelSettings>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = session.selected();
        let id = selected.map(|c| c.id.as_str());
        if self.conversation.as_deref() != id {
            self.conversation = id.map(str::to_owned);
            self.details.clear();
            self.markdown.clear();
            self.anchors.borrow_mut().clear();
            self.edit = None;
            self.overlay = None;
            self.delete = None;
            self.follow = true;
        }
        let source_draft = selected.map_or("", |c| c.draft.as_str());
        if self.draft.as_ref() != source_draft {
            self.draft = source_draft.to_owned().into();
            self.input
                .update(cx, |state, cx| state.set_value(source_draft, window, cx));
        }
        self.unlocked = session.phase() == lumakit_ai::chat::ChatPhase::Idle
            && !session.save_failed()
            && !self.external_busy;
        let picked = session.history().model.as_ref();
        let valid_chat = picked.is_some_and(|m| options.iter().any(|o| same_model(m, &o.model)));
        let tools_supported = model_settings.is_none_or(|s| s.supports_tools != Some(false));
        let composer = session.composer();
        let valid_image = composer
            .image_model
            .as_ref()
            .is_some_and(|m| image_options.iter().any(|o| same_model(m, &o.model)));
        self.image_notice = if !valid_chat {
            "请先选择有效聊天模型；点击配置生图模型"
        } else if !tools_supported {
            "当前聊天模型不支持工具调用；请更换聊天模型"
        } else if !valid_image {
            "请先选择生图模型；点击打开选择器"
        } else {
            "生成图片"
        };
        self.valid_model = valid_chat && (!composer.image_mode || (valid_image && tools_supported));
        let model_error = if options.is_empty() {
            "尚未配置聊天模型，请打开模型设置。"
        } else if !valid_chat {
            "请选择有效聊天模型。"
        } else if composer.image_mode && !tools_supported {
            "当前聊天模型不支持工具调用，请更换聊天模型或关闭生图。"
        } else if composer.image_mode && !valid_image {
            "请选择有效生图模型，或关闭生图。"
        } else {
            ""
        };
        let r = LumaTheme::global(cx).roles;
        let header = row()
            .flex_nowrap()
            .justify_between()
            .px(space::S12)
            .py(space::S8)
            .bg(r.container_header_bg)
            .border_b_1()
            .border_color(r.border_muted)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_style(text::TITLE_SECTION)
                    .child(selected.map_or("新对话", |c| c.title.as_str()).to_owned()),
            )
            .child(self.tracked(
                "history",
                self.icon_layer(
                    "history",
                    IconName::Menu,
                    "历史对话",
                    self.unlocked,
                    Overlay::History,
                    cx,
                ),
            ))
            .child(self.icon_action(
                "settings",
                IconName::Settings,
                "模型设置",
                true,
                ChatAction::OpenSettings,
                cx,
            ));
        // The transcript and composer share one centered column so wide windows keep a readable line length.
        let mut transcript = col().w_full().max_w(size::CONTENT_MAX).gap(space::S16);
        if let Some(conversation) = selected {
            for (index, turn) in conversation.turns.iter().enumerate() {
                let mut question = col()
                    .gap(space::S4)
                    .px(space::S12)
                    .py(space::S8)
                    .rounded(radius::LG)
                    .bg(r.bubble_user_bg)
                    .border_1()
                    .border_color(r.bubble_user_border)
                    .max_w(relative(0.85))
                    .text_style(text::READING)
                    .text_color(r.text_default)
                    .child(turn.input.clone());
                for attachment in session.turn_attachments(&turn.id) {
                    question = question.child(
                        div()
                            .text_style(text::METADATA)
                            .text_color(r.text_muted)
                            .child(format!("附件 · {}", attachment.name)),
                    );
                }
                let mut group = col()
                    .gap(space::S12)
                    .child(row().justify_end().child(question));
                if let Some((last, previous)) = turn.attempts.split_last() {
                    if !previous.is_empty() {
                        let key = format!("previous:{}", turn.id);
                        group = group.child(self.disclosure(
                            key.clone(),
                            format!("之前的 {} 次回答", previous.len()),
                            cx,
                        ));
                        if self.details.contains(&key) {
                            for attempt in previous {
                                group = group.child(self.answer(
                                    AnswerContext {
                                        session,
                                        conversation: &conversation.id,
                                        turn: &turn.id,
                                        options,
                                        image_options,
                                    },
                                    attempt,
                                    turn.selected_attempt.as_deref() == Some(attempt.id.as_str()),
                                    false,
                                    cx,
                                ));
                            }
                        }
                    }
                    group = group.child(self.answer(
                        AnswerContext {
                            session,
                            conversation: &conversation.id,
                            turn: &turn.id,
                            options,
                            image_options,
                        },
                        last,
                        turn.selected_attempt.as_deref().unwrap_or(&last.id) == last.id,
                        index + 1 == conversation.turns.len(),
                        cx,
                    ));
                }
                transcript = transcript.child(group);
            }
            if conversation.turns.is_empty() {
                transcript = transcript.child(
                    div()
                        .text_style(text::METADATA)
                        .text_color(r.text_muted)
                        .child("开始一个新对话"),
                );
            }
        } else {
            transcript = transcript.child(
                div()
                    .text_style(text::METADATA)
                    .text_color(r.text_muted)
                    .child("输入消息，开始一个新对话"),
            );
        }
        if self.follow {
            self.scroll.scroll_to_bottom();
        }

        let mut footer = col().w_full().max_w(size::CONTENT_MAX).gap(space::S8);
        if session.save_failed() || !session.notice().is_empty() || !model_error.is_empty() {
            let mut notice = div()
                .id("chat-notice")
                .max_h(NOTICE_MAX)
                .overflow_y_scroll()
                .text_style(text::METADATA)
                .text_color(r.feedback_error);
            if session.save_failed() {
                notice = notice.child(div().child("保存失败，恢复保存前不能修改聊天。"));
            }
            if !model_error.is_empty() {
                notice = notice.child(div().child(model_error));
            }
            if !session.notice().is_empty() {
                notice = notice.child(div().child(session.notice().to_owned()));
            }
            footer = footer.child(notice);
        }
        let mut attachments = row();
        for attachment in session.draft_attachments() {
            attachments = attachments.child(
                self.action(
                    SharedString::from(format!("attachment:{}", attachment.id)),
                    attachment.name.clone(),
                    self.unlocked,
                    ChatAction::RemoveAttachment(attachment.id.clone()),
                    cx,
                )
                .icon(IconName::X)
                .accessibility_label(format!("移除附件 {}", attachment.name))
                .tooltip(format!("移除附件 · {} 字节", attachment.bytes)),
            );
        }
        if !session.draft_attachments().is_empty() {
            footer = footer.child(attachments);
        }
        let height = if self.expanded {
            (f32::from(window.viewport_size().height) * 0.35)
                .clamp(COMPOSER_EXPANDED.0, COMPOSER_EXPANDED.1)
        } else {
            COMPOSER_HEIGHT
        };
        // Kit's Textarea binds Ctrl/⌘+Enter to `Enter { secondary: true }`, and GPUI dispatches bindings
        // before raw key listeners, so the shortcut is intercepted as that action in the capture phase.
        let editor = div()
            .capture_action(cx.listener(|this, action: &Enter, window, cx| {
                if !action.secondary || action.shift {
                    return;
                }
                // The input exposes the platform's real IME marked range, not a guessed key sequence.
                let composing = this.input.update(cx, |state, cx| {
                    state.marked_text_range(window, cx).is_some()
                });
                if composing {
                    return;
                }
                cx.stop_propagation();
                if this.unlocked
                    && this.valid_model
                    && this.overlay.is_none()
                    && !this.draft.trim().is_empty()
                {
                    this.emit(ChatAction::Send, cx);
                }
            }))
            .child(
                Textarea::new(&self.input)
                    .aria_label("消息输入；Enter 换行，Ctrl+Enter 发送")
                    .disabled(!self.unlocked)
                    .appearance(false)
                    .h(px(height)),
            );
        let mut toolbar = row().gap(space::S4);
        for &tool in session.toolbar() {
            toolbar = toolbar.child(self.tracked(
                format!("tool-{tool:?}"),
                self.tool_button(
                    tool,
                    composer,
                    valid_chat && tools_supported && valid_image,
                    cx,
                ),
            ));
        }
        toolbar = toolbar.child(self.tracked(
            "tools",
            self.icon_layer(
                "tools",
                IconName::Plus,
                "添加附件与输入工具",
                self.unlocked,
                Overlay::Tools,
                cx,
            ),
        ));
        if composer.image_mode && !session.toolbar().contains(&ToolbarTool::GenerateImage) {
            let mut next = composer.clone();
            next.image_mode = false;
            toolbar = toolbar.child(
                self.action(
                    "remove-image-mode",
                    "生成图片",
                    self.unlocked,
                    ChatAction::SetComposer(next),
                    cx,
                )
                .icon(IconName::X)
                .accessibility_label("关闭生成图片"),
            );
        }
        toolbar = toolbar
            .child(self.model_pill(session, options, cx))
            .child(self.tracked(
                "usage",
                self.layer_button("usage", "用量", true, Overlay::Usage, cx),
            ));
        if !self.follow {
            toolbar = toolbar.child(
                icon_button("bottom", IconName::ArrowDown, "回到底部").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.follow = true;
                        this.scroll.scroll_to_bottom();
                        cx.notify();
                    },
                )),
            );
        }
        if session.save_failed() {
            toolbar = toolbar.child(self.icon_action(
                "save",
                IconName::RefreshCw,
                "重试保存",
                !session.busy(),
                ChatAction::Save,
                cx,
            ));
        }
        toolbar = toolbar.child(
            div()
                .flex_1()
                .min_w(px(160.))
                .flex()
                .items_center()
                .justify_end()
                .gap(space::S4)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .justify_end()
                        .overflow_hidden()
                        .child(self.usage_status.clone()),
                )
                .child(if session.busy() {
                    self.icon_action(
                        "stop",
                        IconName::Square,
                        "停止生成",
                        true,
                        ChatAction::Stop,
                        cx,
                    )
                } else {
                    self.icon_action(
                        "send",
                        IconName::ArrowUp,
                        "发送消息 · Ctrl+Enter",
                        self.unlocked && self.valid_model && !self.draft.trim().is_empty(),
                        ChatAction::Send,
                        cx,
                    )
                    .primary()
                }),
        );
        // Text area and toolbar share one raised card, as in the reference composer.
        footer = footer.child(
            col()
                .gap(space::S4)
                .p(space::S8)
                .rounded(radius::LG)
                .bg(r.control_bg)
                .border_1()
                .border_color(r.border_default)
                .child(editor)
                .child(toolbar),
        );
        let transcript = div()
            .id("transcript")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .items_center()
            .px(space::S16)
            .py(space::S16)
            .child(transcript)
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                let delta = event.delta.pixel_delta(SCROLL_LINE);
                if delta.y > px(0.) {
                    this.follow = false;
                } else if this.scroll.offset().y.abs() + FOLLOW_SLOP >= this.scroll.max_offset().y {
                    this.follow = true;
                }
                cx.notify();
            }));
        let footer = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .p(space::S12)
            .border_t_1()
            .border_color(r.border_muted)
            .child(footer);
        let mut root = col()
            .id(("chat-panel", cx.entity_id()))
            .relative()
            .size_full()
            .min_h_0()
            .min_w_0()
            .text_style(text::UI)
            .text_color(r.text_default)
            .bg(r.container_workspace_bg)
            .child(header)
            .child(transcript)
            .child(footer);
        if let Some(overlay) = self.overlay.clone() {
            root = root.child(self.render_overlay(
                overlay,
                overlays::OverlayContext {
                    session,
                    options,
                    image_options,
                    settings: model_settings,
                    can_image: valid_chat && tools_supported,
                },
                window,
                cx,
            ));
        }
        root.into_any_element()
    }

    fn answer(
        &mut self,
        context: AnswerContext<'_>,
        attempt: &Attempt,
        chosen: bool,
        retry: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let AnswerContext {
            session,
            conversation,
            turn,
            options,
            image_options,
        } = context;
        let r = LumaTheme::global(cx).roles;
        // The chosen attempt keeps a stronger outline; the selected-context button carries the state.
        // READING on the card is what the Markdown answer inherits; other children set their own style.
        let mut body = col()
            .id(SharedString::from(attempt.id.clone()))
            .gap(space::S8)
            .p(space::S12)
            .rounded(radius::LG)
            .text_style(text::READING)
            .bg(r.container_elevated_bg)
            .border_1()
            .border_color(if chosen {
                r.border_default
            } else {
                r.border_muted
            });
        let source = options
            .iter()
            .chain(image_options)
            .find(|o| same_model(&o.model, &attempt.model));
        let source_detail = source.map(model_label).unwrap_or_else(|| {
            format!(
                "{} · {} / {}",
                attempt.model.model, attempt.model.provider, attempt.model.endpoint
            )
        });
        body = body.child(
            div()
                .id(SharedString::from(format!("source:{}", attempt.id)))
                .text_style(text::METADATA)
                .text_color(r.text_muted)
                .tooltip(move |window, cx| Tooltip::new(source_detail.clone()).build(window, cx))
                .child(
                    source
                        .map_or(&attempt.model.model, |option| &option.name)
                        .clone(),
                ),
        );
        if attempt.search.status != SearchStatus::Idle {
            let label = match attempt.search.status {
                SearchStatus::Searching => "正在搜索…",
                SearchStatus::Complete => "搜索来源",
                SearchStatus::Failed => "搜索失败",
                SearchStatus::Cancelled => "搜索已取消",
                SearchStatus::Idle => "",
            };
            let key = format!("sources:{}", attempt.id);
            body = body.child(self.disclosure(key.clone(), label, cx));
            if self.details.contains(&key) {
                for (index, hit) in attempt.search.hits.iter().enumerate() {
                    let url = hit.url.clone();
                    body = body.child(
                        row().child(
                            Button::new(SharedString::from(format!(
                                "source:{}:{index}",
                                attempt.id
                            )))
                            .small()
                            .link()
                            .icon(IconName::ExternalLink)
                            .label(hit.title.clone())
                            .tooltip(hit.snippet.clone())
                            .disabled(!web_link(&url))
                            .on_click(move |_, _, cx| {
                                if web_link(&url) {
                                    cx.open_url(&url);
                                }
                            }),
                        ),
                    );
                }
                if let Some(error) = &attempt.search.error {
                    body = body.child(
                        div()
                            .text_style(text::METADATA)
                            .text_color(r.feedback_error)
                            .child(error.clone()),
                    );
                }
            }
        }
        if !attempt.reasoning.is_empty() {
            let key = format!("reasoning:{}", attempt.id);
            body = body.child(self.disclosure(key.clone(), "思考过程", cx));
            if self.details.contains(&key) {
                let mut reasoning = div()
                    .id(SharedString::from(key))
                    .max_h(DETAIL_MAX)
                    .overflow_y_scroll()
                    .text_style(text::METADATA)
                    .text_color(r.text_muted);
                for block in &attempt.reasoning {
                    reasoning = reasoning.child(
                        div().child(
                            block
                                .get("text")
                                .or_else(|| block.get("thinking"))
                                .and_then(|v| v.as_str())
                                .map_or_else(|| block.to_string(), str::to_owned),
                        ),
                    );
                }
                body = body.child(reasoning);
            }
        }
        for event in &attempt.tool_events {
            let key = format!("tool:{}:{}", attempt.id, event.id);
            let name = if event.name == "generate_image" {
                "图片生成"
            } else {
                &event.name
            };
            let status = if event.is_error {
                "失败，请重试或更换模型"
            } else if event.result.is_some() {
                "完成"
            } else {
                "进行中…"
            };
            body = body.child(self.disclosure(key.clone(), format!("{name}{status}"), cx));
            if self.details.contains(&key) {
                let mut details = div()
                    .id(SharedString::from(key))
                    .max_h(DETAIL_MAX)
                    .overflow_y_scroll()
                    .text_style(text::METADATA)
                    .child(format!(
                        "工具：{}\n调用 ID：{}\n参数：{}",
                        event.name, event.id, event.arguments
                    ));
                if let Some(result) = &event.result {
                    details = details.child(div().child(result.clone()));
                }
                if let Some(error) = &event.error {
                    details =
                        details.child(div().text_color(r.feedback_error).child(error.clone()));
                }
                body = body.child(details);
            }
        }
        let mut images = row().gap(space::S8);
        for image in &attempt.generated {
            images = images.child(match self.media.get(session, image) {
                Ok(preview) => {
                    let id = image.id.clone();
                    Button::new(SharedString::from(format!("image:{}", id)))
                        .ghost()
                        .p_0()
                        .w(IMAGE_W)
                        .max_w_full()
                        .h(IMAGE_H)
                        .flex_none()
                        .overflow_hidden()
                        .accessibility_label("生成图片，点击打开")
                        .tooltip("打开生成图片")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.emit(ChatAction::OpenMedia(id.clone()), cx)
                        }))
                        .child(
                            img(preview)
                                .w(IMAGE_W)
                                .max_w_full()
                                .h(IMAGE_H)
                                .object_fit(ObjectFit::Contain),
                        )
                        .into_any_element()
                }
                Err(error) => div()
                    .text_style(text::METADATA)
                    .text_color(r.feedback_error)
                    .child(error)
                    .into_any_element(),
            });
        }
        if !attempt.generated.is_empty() {
            body = body.child(images);
        }
        let (text_view, raw) = self.markdown.view(&attempt.id, &attempt.text);
        if !attempt.text.is_empty() {
            body = body.child(text_view);
        }
        body = body.child(self.diagnostics(attempt, cx));
        let key = format!("usage:{}", attempt.id);
        body = body.child(self.disclosure(key.clone(), "用量详情", cx));
        if self.details.contains(&key) {
            body = body.child(usage_details(attempt));
        }
        let mut actions = row().gap(space::S4).child(
            icon_button(
                SharedString::from(format!("copy:{}", attempt.id)),
                IconName::Copy,
                "复制原文",
            )
            .disabled(raw.is_empty())
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(raw.to_string()))
            }),
        );
        if retry {
            let available = options
                .iter()
                .chain(image_options)
                .any(|o| same_model(&o.model, &attempt.model));
            actions = actions.child(self.icon_action(
                SharedString::from(format!("retry:{}", attempt.id)),
                IconName::RefreshCw,
                "重试 / 重新生成",
                self.unlocked && available,
                ChatAction::Retry,
                cx,
            ));
            let image_attempt = !options.iter().any(|o| same_model(&o.model, &attempt.model))
                && !attempt
                    .tool_events
                    .iter()
                    .any(|e| e.name == "generate_image")
                && (!attempt.generated.is_empty()
                    || image_options
                        .iter()
                        .any(|o| same_model(&o.model, &attempt.model)));
            if !image_attempt {
                actions = actions.child(self.tracked(
                    "retry-model",
                    self.icon_layer(
                        "retry-model",
                        IconName::Bot,
                        "换模型重答",
                        self.unlocked && !options.is_empty(),
                        Overlay::RetryModels,
                        cx,
                    ),
                ));
            }
        }
        actions = actions
            .child(
                self.icon_action(
                    SharedString::from(format!("select:{}", attempt.id)),
                    if chosen {
                        IconName::Check
                    } else {
                        IconName::Menu
                    },
                    if chosen {
                        "已选上下文"
                    } else {
                        "选择上下文"
                    },
                    self.unlocked && attempt.status == TurnStatus::Completed,
                    ChatAction::SelectAttempt {
                        conversation_id: conversation.into(),
                        turn_id: turn.into(),
                        attempt_id: attempt.id.clone(),
                    },
                    cx,
                )
                .selected(chosen),
            )
            .child(self.tracked(
                format!("more:{}", attempt.id),
                self.icon_layer(
                    format!("more:{}", attempt.id),
                    IconName::Ellipsis,
                    "更多操作",
                    true,
                    Overlay::More(conversation.into(), turn.into(), attempt.id.clone()),
                    cx,
                ),
            ));
        body.child(actions).into_any_element()
    }
}
fn row() -> Div {
    div()
        .flex()
        .items_center()
        .flex_wrap()
        .gap(space::S8)
        .min_w_0()
}
fn col() -> Div {
    div().flex().flex_col().min_w_0()
}
fn icon_button(id: impl Into<ElementId>, icon: IconName, label: &'static str) -> Button {
    Button::new(id)
        .small()
        .ghost()
        .icon(icon)
        .accessibility_label(label)
        .tooltip(label)
}
fn same_model(a: &ModelRef, b: &ModelRef) -> bool {
    a.provider == b.provider
        && a.endpoint == b.endpoint
        && a.model == b.model
        && a.protocol == b.protocol
}
fn model_label(option: &ModelOption) -> String {
    format!(
        "{} · {} / {}",
        option.name, option.provider, option.endpoint
    )
}

/// Call on the host's file-picker worker, never during render.
pub fn pick_attachment() -> Result<Option<std::path::PathBuf>, String> {
    #[cfg(windows)]
    {
        let script = "[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); Add-Type -AssemblyName System.Windows.Forms; $d=[System.Windows.Forms.OpenFileDialog]::new(); $d.Title='上传附件'; $d.Multiselect=$false; if($d.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK){[Console]::Write($d.FileName)}; $d.Dispose()";
        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-STA",
                "-WindowStyle",
                "Hidden",
                "-Command",
                script,
            ])
            .output()
            .map_err(|e| format!("无法打开附件选择器：{e}"))?;
        if !output.status.success() {
            return Err(format!(
                "附件选择器失败：{}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let path = String::from_utf8(output.stdout)
            .map_err(|_| "附件选择器返回了无效路径编码".to_owned())?;
        Ok((!path.is_empty()).then(|| std::path::PathBuf::from(path)))
    }
    #[cfg(not(windows))]
    {
        Err("当前宿主的附件选择器仅支持 Windows".into())
    }
}
/// The host must resolve this path through ChatSession::media_path after an explicit click.
pub fn open_media(path: &std::path::Path) -> Result<(), String> {
    if !path.is_file() {
        return Err("图片文件不存在".into());
    }
    #[cfg(windows)]
    let result = std::process::Command::new("explorer.exe").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(path).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(path).spawn();
    result.map(|_| ()).map_err(|e| format!("无法打开图片：{e}"))
}
