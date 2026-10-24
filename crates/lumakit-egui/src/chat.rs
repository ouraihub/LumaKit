//! 固定头尾分区的紧凑聊天输入区；业务动作由宿主执行，附件与图片显式打开。
use egui::{Align, Button, FontId, Frame, Id, Layout, Margin, Rect, RichText, ScrollArea, TextEdit, TextStyle, Ui, Vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use lumakit_ai::{ModelRef, chat::{Attempt, ChatSession}, history::TurnStatus, model_config::{ModelSettings, ReasoningEffort}};
use lumakit_ui::chat::{ChatAction, ModelOption};
use pulldown_cmark::{Event, Options, Parser, Tag};
use std::{collections::HashMap, fmt::Debug, hash::Hash};

const CONTROL: f32 = 30.0;
const CACHE_ENTRIES: usize = 64;
const CACHE_BYTES: usize = 4 * 1024 * 1024;

/// 当前帧真实绘制区域（逻辑点）；测试和宿主可检查遮挡。
#[derive(Clone, Copy, Debug)]
pub struct ChatGeometry {
    /// 顶部标题与历史入口区域。
    pub header: Rect,
    /// 聊天记录滚动区域。
    pub transcript: Rect,
    /// 底部输入、提示和按钮的整体区域。
    pub composer: Rect,
    /// 草稿编辑器的滚动视口。
    pub editor: Rect,
    /// 底部操作按钮行。
    pub buttons: Rect,
    /// 发送或停止按钮的区域。
    pub send_stop: Rect,
}
impl Default for ChatGeometry {
    fn default() -> Self {
        Self { header: Rect::NOTHING, transcript: Rect::NOTHING, composer: Rect::NOTHING,
            editor: Rect::NOTHING, buttons: Rect::NOTHING, send_stop: Rect::NOTHING }
    }
}

struct MarkdownEntry {
    source: String,
    display: String,
    cache: CommonMarkCache,
    used: u64,
}
impl MarkdownEntry {
    fn new(source: &str, used: u64) -> Self {
        // 用成熟解析器的源范围移除图片：即使宿主安装了网络/文件加载器，也不会调用它们。
        let mut images = Parser::new_ext(source, markdown_options()).into_offset_iter()
            .filter_map(|(event, range)| matches!(event, Event::Start(Tag::Image { .. })).then_some(range))
            .collect::<Vec<_>>();
        images.sort_by_key(|range| range.start);
        let mut display = String::with_capacity(source.len());
        let mut end = 0;
        for range in images {
            if range.start < end { continue; }
            display.push_str(&source[end..range.start]);
            display.push_str("〔图片未加载〕");
            end = range.end;
        }
        display.push_str(&source[end..]);
        let mut cache = CommonMarkCache::default();
        for event in Parser::new_ext(&display, markdown_options()) {
            if let Event::Start(Tag::Link { dest_url, .. }) = event {
                if !web_link(&dest_url) { cache.add_link_hook(dest_url.into_string()); }
            }
        }
        Self { source: source.to_owned(), display, cache, used }
    }
    fn bytes(&self) -> usize { self.source.len() + self.display.len() }
}
fn markdown_options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES | Options::ENABLE_DEFINITION_LIST
}
fn web_link(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else { return false; };
    (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        && !rest.is_empty() && !url.chars().any(char::is_control)
}
fn same_model(a: &ModelRef, b: &ModelRef) -> bool {
    a.provider == b.provider && a.endpoint == b.endpoint && a.model == b.model && a.protocol == b.protocol
}
fn model_label(option: &ModelOption) -> String {
    format!("{} · {} / {}", option.name, option.provider, option.endpoint)
}

/// 每个实例独占输入、弹窗、滚动和 Markdown 缓存；不持有连接或产品设置。
#[allow(dead_code)]
pub struct ChatPanel {
    id: Id,
    conversation: Option<String>,
    draft: String,
    expanded: bool,
    composing: bool,
    history_open: bool,
    quick_open: bool,
    history_search: String,
    delete: Option<String>,
    edit: Option<(String, String, String, String, String)>,
    model_search: String,
    follow: bool,
    to_bottom: bool,
    last_offset: f32,
    frame: u64,
    markdown: HashMap<String, MarkdownEntry>,
    geometry: ChatGeometry,
}
impl ChatPanel {
    /// 同屏每个组件必须提供不同 ID。
    pub fn new(instance_id: impl Hash + Debug) -> Self {
        Self { id: Id::unique(instance_id), conversation: None, draft: String::new(), expanded: false,
            composing: false, history_open: false, quick_open: false, history_search: String::new(), delete: None, edit: None,
            model_search: String::new(), follow: true, to_bottom: true,
            last_offset: 0.0, frame: 0, markdown: HashMap::new(), geometry: ChatGeometry::default() }
    }
    /// 上次 show 捕获的实际布局，不重新计算坐标。
    pub fn geometry(&self) -> &ChatGeometry { &self.geometry }

    /// 仅绘制并返回动作；模型目录与当前模型设置按引用传入，不复制连接配置。
    pub fn show(&mut self, ui: &mut Ui, session: &ChatSession, options: &[ModelOption], image_options: &[ModelOption], model_settings: Option<&ModelSettings>) -> Vec<ChatAction> {
        self.frame = self.frame.wrapping_add(1);
        self.geometry = ChatGeometry::default();
        let selected = session.selected();
        let selected_id = selected.map(|conversation| conversation.id.as_str());
        if self.conversation.as_deref() != selected_id {
            self.conversation = selected_id.map(str::to_owned);
            self.draft = selected.map_or_else(String::new, |conversation| conversation.draft.clone());
            self.composing = false;
            self.follow = true;
            self.to_bottom = true;
            self.last_offset = 0.0;
            self.markdown.clear();
        } else if let Some(conversation) = selected {
            // 宿主执行 Send 后清空草稿、或保存失败恢复草稿时同步；不是每帧复制全文。
            if self.draft != conversation.draft { self.draft.clone_from(&conversation.draft); }
        }
        let mut actions = Vec::new();
        ui.push_id(self.id, |ui| {
            ui.style_mut().text_styles.insert(TextStyle::Body, FontId::proportional(13.0));
            ui.style_mut().text_styles.insert(TextStyle::Button, FontId::proportional(13.0));
            ui.style_mut().interaction.selectable_labels = true;
            ui.style_mut().interaction.multi_widget_text_select = true;
            ui.spacing_mut().interact_size.y = CONTROL;
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            let unlocked = !session.busy() && !session.save_failed();
            self.geometry.header = egui::Panel::top(self.id.with("header"))
                .exact_size(46.0).frame(Frame::new().inner_margin(Margin::symmetric(12, 8)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_sized([(ui.available_width() - 84.0).max(24.0), CONTROL], egui::Label::new(
                            RichText::new(selected.map_or("新对话", |c| c.title.as_str())).strong()).truncate());
                        if crate::icons::action(ui, unlocked, "\u{E2F0}", "历史对话").clicked() { self.history_open = true; }
                        if crate::icons::action(ui, true, "\u{E270}", "模型设置").clicked() { actions.push(ChatAction::OpenSettings); }
                    });
                }).response.rect;

            let composer = session.composer();
            let mut web_search = composer.web_search;
            let mut reasoning = composer.reasoning;
            let mut image_mode = composer.image_mode;
            let mut image_model = composer.image_model.as_ref();
            let active_options = if composer.image_mode { image_options } else { options };
            let picked = if composer.image_mode { composer.image_model.as_ref() } else { session.history().model.as_ref() };
            let valid_model = picked.is_some_and(|model| active_options.iter().any(|option| same_model(&option.model, model)));
            let model_error = if active_options.is_empty() { if composer.image_mode { "尚未配置图片模型，请打开模型设置。" } else { "尚未配置聊天模型，请打开模型设置。" } }
                else if !valid_model { "请选择有效模型。" } else { "" };
            let has_notice = !session.notice().is_empty() || !model_error.is_empty() || session.save_failed();
            let attachment_height = if session.draft_attachments().is_empty() { 0.0 } else { 36.0 };
            let budget = (ui.available_height() - 106.0 - attachment_height).max(16.0);
            let notice_height = if has_notice { 40.0_f32.min(budget * 0.35) } else { 0.0 };
            let editor_height = (if self.expanded { ui.available_height() * 0.4 } else { 64.0 })
                .min((budget - notice_height - if has_notice { 8.0 } else { 0.0 }).max(16.0));
            let composer_height = editor_height + 106.0 + attachment_height + notice_height + if has_notice { 8.0 } else { 0.0 };
            self.geometry.composer = egui::Panel::bottom(self.id.with("composer"))
                .exact_size(composer_height).frame(Frame::new().inner_margin(Margin::symmetric(12, 8)))
                .show(ui, |ui| {
                    if has_notice {
                        ScrollArea::vertical().id_salt(self.id.with("notice")).max_height(notice_height)
                            .min_scrolled_height(notice_height)
                            .auto_shrink([false, false]).show(ui, |ui| {
                                if session.save_failed() { ui.colored_label(ui.visuals().error_fg_color, "保存失败，恢复保存前不能修改聊天。"); }
                                if !model_error.is_empty() { ui.colored_label(ui.visuals().error_fg_color, model_error); }
                                if !session.notice().is_empty() { ui.colored_label(ui.visuals().error_fg_color, session.notice()); }
                            });
                    }
                    Frame::new().fill(ui.visuals().extreme_bg_color).stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                        .corner_radius(12).inner_margin(8).show(ui, |ui| {
                    if !session.draft_attachments().is_empty() {
                        ScrollArea::horizontal().id_salt(self.id.with("attachments")).max_height(28.0).show(ui, |ui| {
                            ui.horizontal(|ui| for attachment in session.draft_attachments() {
                                if ui.add_enabled(unlocked, Button::new(format!("{} ×", attachment.name))).on_hover_text(format!("移除附件 · {} 字节", attachment.bytes)).accessible_name(format!("移除附件 {}", attachment.name)).clicked() {
                                    actions.push(ChatAction::RemoveAttachment(attachment.id.clone()));
                                }
                            });
                        });
                    }
                    let shortcut = self.consume_send_shortcut(ui, unlocked && valid_model);
                    let editor = ScrollArea::vertical().id_salt(self.id.with("draft-scroll"))
                        .max_height(editor_height).min_scrolled_height(editor_height).auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add_enabled_ui(unlocked, |ui| {
                                ui.add(TextEdit::multiline(&mut self.draft).id(self.id.with("draft"))
                                    .frame(Frame::NONE).desired_width(f32::INFINITY).desired_rows(3)
                                    .hint_text(if composer.image_mode { "描述要生成的图片…" } else { "输入消息…" })).accessible_name("消息输入")
                            }).inner
                        });
                    self.geometry.editor = editor.inner_rect;
                    if editor.inner.changed() {
                        if selected.is_none() { actions.push(ChatAction::New); }
                        actions.push(ChatAction::Draft(self.draft.clone()));
                        if self.draft == "/" { self.quick_open = true; }
                    }
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(unlocked, |ui| {
                            if composer.image_mode {
                                egui::ComboBox::from_id_salt(self.id.with("image-model")).width((ui.available_width() - 78.0).max(60.0).min(300.0))
                                    .selected_text(picked.and_then(|m| image_options.iter().find(|o| same_model(m, &o.model))).map(model_label).unwrap_or_else(|| "选择图片模型".into())).truncate().show_ui(ui, |ui| {
                                        for option in image_options {
                                            if ui.selectable_label(picked.is_some_and(|m| same_model(m, &option.model)), model_label(option)).clicked() { image_model = Some(&option.model); }
                                        }
                                    });
                                if ui.button("退出生图").clicked() { image_mode = false; }
                            } else {
                                let label = picked.and_then(|m| options.iter().find(|o| same_model(&o.model, m))).map(model_label).unwrap_or_else(|| "选择模型".into());
                                model_picker(ui, self.id.with("models"), &label, &mut self.model_search, options, picked, false, &mut actions);
                            }
                        });
                    });
                    self.geometry.buttons = ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if crate::icons::action(ui, unlocked, "\u{E34C}", "新对话").clicked() { actions.push(ChatAction::New); }
                        let search = ui.add_enabled(unlocked && !composer.image_mode, crate::icons::button("\u{E288}").selected(web_search))
                            .on_hover_text("联网搜索").accessible_name("联网搜索");
                        if search.clicked() { web_search = !web_search; }
                        ui.add_enabled_ui(unlocked, |ui| {
                            egui::containers::menu::MenuButton::from_button(crate::icons::button("\u{E3D4}")).ui(ui, |ui| {
                                if ui.button("上传附件").on_hover_text("最多 4 个附件；UTF-8 文本合计不超过 1 MiB，图片每张不超过 4 MiB").clicked() { actions.push(ChatAction::UploadAttachment); ui.close(); }
                                if ui.button("生成图片").clicked() { image_mode = true; web_search = false; reasoning = None; ui.close(); }
                                if ui.button("输入快捷面板").clicked() { self.quick_open = true; ui.close(); }
                                if ui.button(if self.expanded { "收起输入" } else { "展开输入" }).clicked() { self.expanded = !self.expanded; ui.close(); }
                                ui.label("Ctrl / ⌘ + Enter 发送；Enter 换行");
                            }).0.on_hover_text("添加附件与输入工具").accessible_name("添加附件与输入工具");
                        });
                        ui.add_enabled_ui(unlocked && !composer.image_mode, |ui| {
                            egui::ComboBox::from_id_salt(self.id.with("reasoning")).width(64.0)
                                .selected_text(reasoning_label(reasoning)).show_ui(ui, |ui| {
                                    ui.selectable_value(&mut reasoning, None, "默认");
                                    if let Some(config) = model_settings.and_then(|s| s.reasoning.as_ref()) {
                                        for &effort in &config.efforts { ui.selectable_value(&mut reasoning, Some(effort), reasoning_label(Some(effort))); }
                                    }
                                }).response.on_hover_text("推理强度：默认沿用模型配置").accessible_name("推理强度");
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let send = if session.busy() {
                                let response = crate::icons::action(ui, true, "\u{E46C}", "停止生成");
                                if response.clicked() { actions.push(ChatAction::Stop); }
                                response
                            } else {
                                let response = crate::icons::action(ui, unlocked && valid_model && !self.draft.trim().is_empty(), "\u{E08E}", "发送消息 · Ctrl+Enter");
                                if response.clicked() || (shortcut && !self.draft.trim().is_empty()) {
                                    actions.push(ChatAction::Send); self.follow = true; self.to_bottom = true;
                                }
                                response
                            };
                            self.geometry.send_stop = send.rect;
                            if session.save_failed() {
                                if crate::icons::action(ui, true, "\u{E038}", "重试保存").clicked() { actions.push(ChatAction::Save); }
                            } else if !self.follow {
                                if crate::icons::action(ui, true, "\u{E03E}", "回到底部").clicked() { self.follow = true; self.to_bottom = true; }
                            } else if ui.available_width() > 36.0 {
                                ui.add_sized([ui.available_width(), CONTROL], egui::Label::new(RichText::new(composer_usage(session)).weak().small()).truncate()).on_hover_text("上一回答的服务商实际 token 用量；不是草稿估算");
                            }
                        });
                    }).response.rect;
                    });
                }).response.rect;
            if web_search != composer.web_search || reasoning != composer.reasoning || image_mode != composer.image_mode
                || !match (image_model, composer.image_model.as_ref()) { (Some(a), Some(b)) => same_model(a, b), (None, None) => true, _ => false } {
                actions.insert(0, ChatAction::SetComposer(lumakit_ai::chat::ComposerSettings { web_search, reasoning, image_mode, image_model: image_model.cloned() }));
            }

            let remaining = ui.available_rect_before_wrap();
            self.geometry.transcript = remaining;
            let upward = ui.input(|input| input.pointer.hover_pos().is_some_and(|p| remaining.contains(p))
                && input.smooth_scroll_delta().y > 0.0);
            if upward { self.follow = false; }
            let scroll = ScrollArea::vertical().id_salt(self.id.with(("transcript", selected_id)))
                .auto_shrink([false, false]).stick_to_bottom(self.follow).animated(false);
            let output = scroll.show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                if let Some(conversation) = selected {
                    for (index, turn) in conversation.turns.iter().enumerate() {
                        ui.push_id(&turn.id, |ui| {
                            ui.with_layout(Layout::top_down(Align::Max), |ui| {
                                Frame::new().fill(ui.visuals().selection.bg_fill).corner_radius(8)
                                    .inner_margin(12).show(ui, |ui| {
                                        ui.set_max_width((remaining.width() * 0.78 - 24.0).max(100.0));
                                        ui.label(RichText::new(&turn.input).color(ui.visuals().selection.stroke.color).line_height(Some(20.8)));
                                        for attachment in session.turn_attachments(&turn.id) { ui.label(RichText::new(format!("附件 · {}", attachment.name)).small()); }
                                    });
                            });
                            ui.add_space(12.0);
                            if let Some((last, previous)) = turn.attempts.split_last() {
                                if !previous.is_empty() {
                                    egui::CollapsingHeader::new(format!("之前的 {} 次回答", previous.len())).id_salt("attempts").show(ui, |ui| {
                                        for attempt in previous { self.answer(ui, &conversation.id, &turn.id, attempt, false, unlocked, options, image_options, &mut actions); }
                                    });
                                }
                                self.answer(ui, &conversation.id, &turn.id, last, index + 1 == conversation.turns.len(), unlocked, options, image_options, &mut actions);
                            }
                            ui.add_space(16.0);
                        });
                    }
                    if conversation.turns.is_empty() { ui.add_space(24.0); ui.label(RichText::new("开始一个新对话").weak()); }
                } else {
                    ui.add_space(24.0);
                    ui.label(RichText::new("输入消息，开始一个新对话").weak());
                }
                if self.to_bottom || self.follow { ui.scroll_to_cursor(Some(Align::BOTTOM)); self.to_bottom = false; }
            });
            // egui 的 sticky 状态处理滚轮与拖动；本地状态还负责回到底部入口。
            let max_offset = (output.content_size.y - output.inner_rect.height()).max(0.0);
            let offset = output.state.clamped_offset().y;
            if offset + 1.0 < self.last_offset && ui.input(|i| i.pointer.primary_down()) { self.follow = false; }
            if !upward && offset >= max_offset - 8.0 && offset > self.last_offset + 1.0 { self.follow = true; }
            self.last_offset = offset;
            self.history_modal(ui, session, &mut actions);
            self.quick_modal(ui, unlocked, &mut actions);
            self.edit_modal(ui, &mut actions);
        });
        self.markdown.retain(|_, entry| self.frame.wrapping_sub(entry.used) <= 120);
        actions
    }

    fn consume_send_shortcut(&mut self, ui: &Ui, enabled: bool) -> bool {
        if !ui.memory(|memory| memory.has_focus(self.id.with("draft"))) {
            self.composing = false;
            return false;
        }
        let mut ime_frame = false;
        ui.input(|input| for event in &input.events {
            if let egui::Event::Ime(event) = event {
                ime_frame = true;
                if let egui::ImeEvent::Preedit { text, .. } = event {
                    self.composing = !text.is_empty();
                } else if matches!(event, egui::ImeEvent::Commit(_)) {
                    self.composing = false;
                }
            }
        });
        let can_send = enabled && !self.composing && !ime_frame && !self.history_open && !self.quick_open;
        ui.input_mut(|input| {
            let mut send = false;
            input.events.retain(|event| {
                if let egui::Event::Key { key: egui::Key::Enter, pressed: true, repeat, modifiers, .. } = event {
                    if (modifiers.ctrl || modifiers.command) && !modifiers.alt && !modifiers.shift {
                        if !repeat && can_send { send = true; }
                        return false;
                    }
                }
                true
            });
            send
        })
    }

    fn answer(&mut self, ui: &mut Ui, conversation_id: &str, turn_id: &str, attempt: &Attempt, retry: bool, unlocked: bool,
        options: &[ModelOption], image_options: &[ModelOption], actions: &mut Vec<ChatAction>) {
        ui.push_id(&attempt.id, |ui| {
            Frame::new().fill(ui.visuals().faint_bg_color).corner_radius(8).inner_margin(12)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let image_attempt = image_options.iter().any(|option| same_model(&option.model, &attempt.model)) || !attempt.generated.is_empty();
                    let source = options.iter().chain(image_options).find(|option| same_model(&option.model, &attempt.model));
                    ui.label(RichText::new(source.map(model_label).unwrap_or_else(|| format!("{} · {} / {}",
                        attempt.model.model, attempt.model.provider, attempt.model.endpoint))).color(ui.visuals().text_color()).size(12.0));
                    ui.add_space(4.0);
                    if attempt.text.is_empty() && attempt.status == TurnStatus::Running { ui.spinner(); ui.label("正在等待回复…"); }
                    if !attempt.text.is_empty() { self.markdown(ui, &attempt.id, &attempt.text); }
                    for image in &attempt.generated {
                        if ui.button("查看生成图片").clicked() { actions.push(ChatAction::OpenMedia(image.id.clone())); }
                    }
                    if let Some(error) = &attempt.error {
                        ScrollArea::vertical().id_salt("error").max_height(88.0).show(ui, |ui| {
                            ui.colored_label(ui.visuals().error_fg_color, error);
                        });
                    }
                    match attempt.status {
                        TurnStatus::Running => { ui.label(RichText::new("正在生成…").weak().small()); }
                        TurnStatus::Cancelled => { ui.label(RichText::new("已停止，保留部分回答").weak().small()); }
                        TurnStatus::Failed => { ui.label(RichText::new("回答失败").color(ui.visuals().error_fg_color).small()); }
                        TurnStatus::Completed => {}
                    }
                    if attempt.rounds.is_empty() && attempt.status != TurnStatus::Running {
                        ui.label(RichText::new("用量未提供 · 费用未知").weak().small());
                    }
                    for (index, round) in attempt.rounds.iter().enumerate() {
                        ui.label(RichText::new(format!("第 {} 轮 · {}", index + 1, usage_label(round))).weak().small())
                            .on_hover_text("使用本次请求保存的用量和价格快照；不同币种不换算，费用不含搜索等额外服务费。缓存读取与写入已包含在输入token中。");
                    }
                    if !attempt.reasoning.is_empty() {
                        ui.label(RichText::new(format!("推理 · {} 块", attempt.reasoning.len())).weak().small());
                    }
                    for event in &attempt.tool_events {
                        ui.label(RichText::new(format!("工具 · {} · {}", event.name, if event.is_error { "失败" } else if event.result.is_some() { "完成" } else { "等待" })).weak().small());
                    }
                    ui.add_space(4.0);
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let complete = unlocked && attempt.status == TurnStatus::Completed;
                        if crate::icons::action(ui, !attempt.text.is_empty(), "\u{E1CA}", "复制原文").clicked() { ui.ctx().copy_text(attempt.text.clone()); }
                        if retry {
                            let source_available = options.iter().chain(image_options).any(|option| same_model(&option.model, &attempt.model));
                            if crate::icons::action(ui, unlocked && source_available, "\u{E038}", if attempt.status == TurnStatus::Completed { "重新生成" } else { "重试" }).clicked() { actions.push(ChatAction::Retry); self.follow = true; self.to_bottom = true; }
                            if !image_attempt {
                            ui.add_enabled_ui(unlocked && !options.is_empty(), |ui| {
                                egui::containers::menu::MenuButton::from_button(crate::icons::button("\u{E0AC}")).ui(ui, |ui| {
                                    for option in options {
                                        if ui.button(model_label(option)).clicked() { ui.close(); actions.push(ChatAction::RetryWith(option.model.clone())); self.follow = true; self.to_bottom = true; }
                                    }
                                }).0.on_hover_text("换模型重答").accessible_name("换模型重答");
                            });
                            }
                        }
                        if crate::icons::action(ui, unlocked, "\u{E4A6}", "删除回答").clicked() { actions.push(ChatAction::DeleteAttempt { conversation_id: conversation_id.into(), turn_id: turn_id.into(), attempt_id: attempt.id.clone() }); }
                        if crate::icons::action(ui, complete && !attempt.text.is_empty(), "\u{E236}", "保存到笔记").clicked() { actions.push(ChatAction::SaveNote { attempt_id: attempt.id.clone(), markdown: attempt.text.clone() }); }
                        if crate::icons::action(ui, complete && !attempt.text.is_empty(), "\u{E428}", "插入编辑器").clicked() { actions.push(ChatAction::InsertMarkdown { attempt_id: attempt.id.clone(), markdown: attempt.text.clone() }); }
                        if crate::icons::action(ui, unlocked, "\u{E2F0}", "选择上下文").clicked() { actions.push(ChatAction::SelectAttempt { conversation_id: conversation_id.into(), turn_id: turn_id.into(), attempt_id: attempt.id.clone() }); }
                        egui::containers::menu::MenuButton::from_button(crate::icons::button("\u{E1FE}")).ui(ui, |ui| {
                            if ui.add_enabled(complete, Button::new("编辑")).clicked() { ui.close(); self.edit = Some((conversation_id.into(), turn_id.into(), attempt.id.clone(), attempt.text.clone(), attempt.text.clone())); }
                            if ui.add_enabled(unlocked, Button::new("分支")).clicked() { ui.close(); actions.push(ChatAction::ForkAttempt { conversation_id: conversation_id.into(), turn_id: turn_id.into(), attempt_id: attempt.id.clone() }); }
                            if ui.add_enabled(unlocked, Button::new("复制对话")).clicked() { ui.close(); actions.push(ChatAction::CopyConversation { conversation_id: conversation_id.into() }); }
                            ui.separator();
                            if ui.add_enabled(complete && !attempt.text.is_empty(), Button::new("导出 Markdown")).clicked() { ui.close(); actions.push(ChatAction::ExportMarkdown { attempt_id: attempt.id.clone(), markdown: attempt.text.clone() }); }
                        }).0.on_hover_text("更多操作").accessible_name("更多操作");
                    });
                });
        });
    }

    fn markdown(&mut self, ui: &mut Ui, id: &str, text: &str) {
        let changed = self.markdown.get(id).is_none_or(|entry| entry.source != text);
        if changed {
            self.markdown.remove(id);
            while self.markdown.len() >= CACHE_ENTRIES
                || (!self.markdown.is_empty() && self.markdown.values().map(MarkdownEntry::bytes).sum::<usize>() + text.len() * 2 > CACHE_BYTES) {
                let oldest = self.markdown.iter().min_by_key(|(_, entry)| entry.used).map(|(key, _)| key.clone());
                if let Some(oldest) = oldest { self.markdown.remove(&oldest); }
            }
            self.markdown.insert(id.to_owned(), MarkdownEntry::new(text, self.frame));
        }
        if let Some(entry) = self.markdown.get_mut(id) {
            entry.used = self.frame;
            ui.scope(|ui| {
                // 固定可用宽度，横向滚动仅接住表格溢出；不能把无限宽度交给正文换行。
                ui.spacing_mut().item_spacing.y = 6.0;
                let width = ui.available_width();
                ScrollArea::horizontal().id_salt(("markdown", id)).show(ui, |ui| {
                    ui.set_width(width);
                    CommonMarkViewer::new().explicit_image_uri_scheme(true).show(ui, &mut entry.cache, &entry.display);
                });
            });
        }
    }
    fn quick_modal(&mut self, ui: &Ui, unlocked: bool, actions: &mut Vec<ChatAction>) {
        if !self.quick_open { return; }
        let response = egui::Modal::new(self.id.with("quick-input")).show(ui.ctx(), |ui| {
            ui.set_max_width((ui.ctx().content_rect().width() - 48.0).min(360.0));
            ui.heading("输入快捷面板");
            for (label, prompt) in QUICK_PROMPTS {
                if ui.add_enabled(unlocked, Button::new(*label)).clicked() {
                    self.insert_prompt(prompt, actions);
                    self.quick_open = false;
                }
            }
            ui.separator();
            if ui.button(if self.expanded { "收起输入" } else { "展开输入" }).clicked() { self.expanded = !self.expanded; }
            ui.label("Ctrl / ⌘ + Enter 发送；Enter 换行");
            if ui.button("关闭").clicked() { self.quick_open = false; }
        });
        if response.should_close() { self.quick_open = false; }
    }

    fn insert_prompt(&mut self, prompt: &str, actions: &mut Vec<ChatAction>) {
        if self.draft == "/" { self.draft.clear(); }
        if !self.draft.is_empty() { self.draft.push_str("\n\n"); }
        self.draft.push_str(prompt);
        if self.conversation.is_none() { actions.push(ChatAction::New); }
        actions.push(ChatAction::Draft(self.draft.clone()));
    }

    fn history_modal(&mut self, ui: &Ui, session: &ChatSession, actions: &mut Vec<ChatAction>) {
        if !self.history_open { return; }
        let response = egui::Modal::new(self.id.with("history")).show(ui.ctx(), |ui| {
            ui.spacing_mut().interact_size.y = CONTROL;
            ui.style_mut().text_styles.insert(TextStyle::Body, FontId::proportional(13.0));
            ui.style_mut().text_styles.insert(TextStyle::Button, FontId::proportional(13.0));
            ui.set_width(420.0_f32.min(ui.ctx().content_rect().width() - 48.0));
            ui.heading("历史对话");
            ui.add(TextEdit::singleline(&mut self.history_search).hint_text("搜索对话"));
            let unlocked = !session.busy() && !session.save_failed();
            if let Some(id) = self.delete.clone() {
                ui.label("确定删除这段对话？删除后无法恢复。");
                ui.horizontal(|ui| {
                    if ui.add_enabled(unlocked, Button::new("确定删除")).clicked() {
                        actions.push(ChatAction::Delete(id.to_owned()));
                        self.delete = None;
                    }
                    if ui.button("取消").clicked() { self.delete = None; }
                });
                ui.separator();
            }
            let query = self.history_search.to_lowercase();
            ScrollArea::vertical().id_salt(self.id.with("history-list"))
                .max_height((ui.ctx().content_rect().height() * 0.55).min(420.0)).show(ui, |ui| {
                    let mut found = false;
                    for conversation in session.history().conversations.iter().rev() {
                        if !conversation.title.to_lowercase().contains(&query) { continue; }
                        found = true;
                        ui.horizontal(|ui| {
                            ui.add_enabled_ui(unlocked, |ui| {
                                if ui.add_sized([ui.available_width() - 64.0, CONTROL], egui::Button::new(
                                    egui::RichText::new(&conversation.title)).selected(Some(&conversation.id) == session.history().selected.as_ref())
                                    .truncate()).clicked() {
                                    actions.push(ChatAction::Select(conversation.id.clone())); self.history_open = false;
                                }
                                if ui.button("删除").clicked() { self.delete = Some(conversation.id.clone()); }
                            });
                        });
                    }
                    if !found { ui.label("没有匹配的历史对话"); }
                });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.add_enabled(unlocked, Button::new("新对话")).clicked() { actions.push(ChatAction::New); self.history_open = false; }
                if ui.button("关闭").clicked() { self.history_open = false; }
            });
        });
        if response.should_close() { self.history_open = false; self.delete = None; }
    }
    fn edit_modal(&mut self, ui: &Ui, actions: &mut Vec<ChatAction>) {
        let Some((conversation_id, turn_id, attempt_id, expected, draft)) = self.edit.as_mut() else { return; };
        let mut close = false;
        egui::Modal::new(self.id.with("edit-attempt")).show(ui.ctx(), |ui| {
            ui.heading("编辑回答");
            ui.add(TextEdit::multiline(draft).desired_width(420.0).desired_rows(8));
            ui.horizontal(|ui| {
                if ui.button("取消").clicked() { close = true; }
                if ui.button("保存").clicked() {
                    actions.push(ChatAction::SubmitEdit { conversation_id: conversation_id.clone(), turn_id: turn_id.clone(), attempt_id: attempt_id.clone(), expected_text: expected.clone(), next_text: draft.clone() });
                    close = true;
                }
            });
        });
        if close { self.edit = None; }
    }
}
const QUICK_PROMPTS: &[(&str, &str)] = &[
    ("总结要点", "请总结以下内容，提炼关键结论和待办事项：\n"),
    ("润色文字", "请在保留原意和事实的前提下润色以下文字：\n"),
    ("解释代码", "请解释以下代码的作用、关键步骤及潜在问题：\n"),
];

fn reasoning_label(effort: Option<ReasoningEffort>) -> &'static str {
    match effort {
        None => "默认", Some(ReasoningEffort::None) => "关闭", Some(ReasoningEffort::Auto) => "自动",
        Some(ReasoningEffort::Minimal) => "最小", Some(ReasoningEffort::Low) => "低",
        Some(ReasoningEffort::Medium) => "中", Some(ReasoningEffort::High) => "高",
        Some(ReasoningEffort::Xhigh) => "超高", Some(ReasoningEffort::Max) => "最大",
    }
}

fn composer_usage(session: &ChatSession) -> String {
    let Some(attempt) = session.selected().and_then(|c| c.turns.last()).and_then(|t| t.attempts.last()) else { return "用量未知".into(); };
    if attempt.rounds.is_empty() || attempt.rounds.iter().any(|r| r.usage.is_none()) { return "用量未知".into(); }
    let total = attempt.rounds.iter().filter_map(|r| r.usage.as_ref()).fold(0u64, |n, u| n.saturating_add(u.input_tokens).saturating_add(u.output_tokens));
    format!("{total} token")
}

/// 使用系统文件对话框；取消不会生成错误或附件。
pub fn pick_attachment() -> Result<Option<std::path::PathBuf>, String> {
    #[cfg(windows)]
    {
        // 固定脚本不拼入文件名；Command 按 Unicode 参数传递，返回路径统一用 UTF-8。
        let script = "[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); Add-Type -AssemblyName System.Windows.Forms; $d=[System.Windows.Forms.OpenFileDialog]::new(); $d.Title='上传附件'; $d.Multiselect=$false; if($d.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK){[Console]::Write($d.FileName)}; $d.Dispose()";
        let output = std::process::Command::new("powershell.exe").args(["-NoProfile", "-STA", "-WindowStyle", "Hidden", "-Command", script]).output().map_err(|e| format!("无法打开附件选择器：{e}"))?;
        if !output.status.success() { return Err(format!("附件选择器失败：{}", String::from_utf8_lossy(&output.stderr))); }
        let path = String::from_utf8(output.stdout).map_err(|_| "附件选择器返回了无效路径编码".to_owned())?;
        Ok((!path.is_empty()).then(|| std::path::PathBuf::from(path)))
    }
    #[cfg(not(windows))]
    { Err("当前宿主的附件选择器仅支持 Windows".into()) }
}

/// 仅在用户明确点击后由系统打开本地媒体；文件名作为独立参数传递。
pub fn open_media(path: &std::path::Path) -> Result<(), String> {
    if !path.is_file() { return Err("图片文件不存在".into()); }
    #[cfg(windows)]
    let result = std::process::Command::new("explorer.exe").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(path).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(path).spawn();
    result.map(|_| ()).map_err(|e| format!("无法打开图片：{e}"))
}

fn model_picker(ui: &mut Ui, id: Id, label: &str, search: &mut String, options: &[ModelOption],
    picked: Option<&ModelRef>, retry: bool, actions: &mut Vec<ChatAction>) {
    egui::ComboBox::from_id_salt(id).width(ui.available_width().min(320.0)).selected_text(label)
        .truncate().show_ui(ui, |ui| {
            ui.add(TextEdit::singleline(search).hint_text("搜索模型、服务商、端点"));
            let query = search.to_lowercase();
            let mut found = false;
            let mut group = None;
            for option in options {
                let label = model_label(option);
                if !label.to_lowercase().contains(&query) { continue; }
                found = true;
                let source = (option.model.provider.as_str(), option.model.endpoint.as_str());
                if group != Some(source) {
                    if group.is_some() { ui.separator(); }
                    ui.label(RichText::new(format!("{} / {}", option.provider, option.endpoint)).weak().small());
                    group = Some(source);
                }
                if ui.selectable_label(picked.is_some_and(|picked| same_model(picked, &option.model)), label).clicked() {
                    actions.push(if retry { ChatAction::RetryWith(option.model.clone()) }
                        else { ChatAction::SelectModel(option.model.clone()) });
                    ui.close();
                }
            }
            if !found { ui.label("没有匹配的模型"); }
        });
}

fn usage_label(round: &lumakit_ai::RoundUsage) -> String {
    let Some(usage) = &round.usage else { return "用量未提供 · 费用未知".into(); };
    let tokens = format!("输入 {} · 输出 {} · 缓存读取 {} · 缓存写入 {} token",
        usage.input_tokens, usage.output_tokens, usage.cache_read_tokens, usage.cache_write_tokens);
    match (&round.pricing, round.cost) {
        (Some(pricing), Some(cost)) if cost.is_finite() && cost >= 0.0 && !pricing.currency.is_empty() =>
            format!("{tokens} · 费用 {} {cost}", pricing.currency),
        _ => format!("{tokens} · 费用未知"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumakit_ai::{Protocol, chat::{Conversation, History, Turn}};
    use std::sync::Arc;

    #[test]
    fn usage_display_preserves_actual_counts_snapshot_currency_and_unknown_cost() {
        use lumakit_ai::{RoundUsage, TokenUsage, model_config::{ModelPricing, PriceTier}};
        let pricing = |currency: &str| ModelPricing { currency: currency.into(), tiers: vec![PriceTier {
            min_input_tokens: 0, input: 1.0, output: 2.0, cache_read: None, cache_write: None,
        }] };
        let mut round = RoundUsage { usage: Some(TokenUsage { input_tokens: 123, output_tokens: 45,
            cache_read_tokens: 67, cache_write_tokens: 8 }), pricing: Some(pricing("USD")), cost: Some(0.000000001) };
        assert_eq!(usage_label(&round), "输入 123 · 输出 45 · 缓存读取 67 · 缓存写入 8 token · 费用 USD 0.000000001");
        round.pricing = Some(pricing("CNY"));
        round.cost = Some(0.25);
        assert!(usage_label(&round).ends_with("费用 CNY 0.25"));
        round.cost = Some(0.0);
        assert!(usage_label(&round).ends_with("费用 CNY 0"));
        round.cost = None;
        assert!(usage_label(&round).ends_with("费用未知"));
        round.pricing = None;
        round.cost = Some(5.0);
        assert!(usage_label(&round).ends_with("费用未知"));
        round.usage = None;
        assert_eq!(usage_label(&round), "用量未提供 · 费用未知");
    }

    fn fixture() -> (tempfile::TempDir, ChatSession, Vec<ModelOption>) {
        let root = tempfile::tempdir().unwrap();
        let model = ModelRef { provider: "来源A".into(), endpoint: "端点A".into(), model: "model".into(), protocol: Protocol::OpenAiChat };
        let markdown = "# 中文回答\n\n这是一段长中文内容，用于检查真实布局和滚动。\n\n- 列表一\n- 列表二\n\n```rust\nlet value = 123;\n```\n\n| 列一 | 列二 |\n| --- | --- |\n| 数据 | 内容 |\n\n".repeat(24);
        let history = History {
            selected: Some("conversation".into()), model: Some(model.clone()),
            conversations: vec![Conversation { id: "conversation".into(), title: "布局验收".into(), draft: "长草稿\n".repeat(300),
                turns: vec![Turn { id: "turn".into(), input: "问题中文".repeat(120), attempts: vec![Attempt {
                    id: "attempt".into(), model: model.clone(), text: markdown, status: TurnStatus::Failed,
                    error: Some("很长的错误信息\n".repeat(300)),
                    rounds: vec![lumakit_ai::RoundUsage { usage: Some(lumakit_ai::TokenUsage {
                        input_tokens: 123, output_tokens: 45, cache_read_tokens: 67, cache_write_tokens: 8,
                    }), pricing: None, cost: None }], kind: lumakit_ai::chat::AttemptKind::Reply,
                    source_attempt_id: None, parent_attempt_id: None, search: Default::default(), tool_events: Vec::new(), reasoning: Vec::new(), generated: Vec::new(),
                }], selected_attempt: None }],
            }],
        };
        let path = root.path().join("chat-history.json");
        lumakit_support::save_versioned(&path, 1, &history).unwrap();
        let session = ChatSession::open(&root.path().join("chat.db"), Arc::new(|| {})).unwrap();
        let options = vec![ModelOption { model, name: "模型".into(), provider: "来源A".into(), endpoint: "端点A".into() }];
        (root, session, options)
    }

    fn draw(ctx: &egui::Context, panel: &mut ChatPanel, session: &ChatSession,
        options: &[ModelOption], size: Vec2, scale: f32, events: Vec<egui::Event>) -> Vec<ChatAction> {
        let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)), events, ..Default::default() };
        input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap().native_pixels_per_point = Some(scale);
        let mut actions = Vec::new();
        // 无渲染器的 headless 帧显式丢弃纹理增量；正式宿主由 eframe 应用。
        ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| { actions = panel.show(ui, session, options, &[], None); });
        }).drop_without_applying_deltas();
        actions
    }

    #[test]
    fn actual_layout_keeps_footer_reachable_for_long_content() {
        let (_root, mut session, options) = fixture();
        session.report("保存或请求错误，详细说明\n".repeat(300));
        for (width, height, scale) in [(860.0, 560.0, 1.0), (1200.0, 800.0, 1.0), (860.0, 560.0, 2.0), (860.0, 220.0, 1.0), (480.0, 480.0, 1.0), (375.0, 480.0, 1.0), (320.0, 480.0, 1.0)] {
            for expanded in [false, true] {
                for options in [&options[..], &[][..]] {
                    let ctx = egui::Context::default();
                    let mut panel = ChatPanel::new("layout");
                    panel.expanded = expanded;
                    for _ in 0..3 { draw(&ctx, &mut panel, &session, options, Vec2::new(width, height), scale, vec![]); }
                    let g = panel.geometry();
                    assert!(g.header.bottom() <= g.transcript.top() + 0.5, "{g:?}");
                    assert!(g.transcript.bottom() <= g.composer.top() + 0.5, "{g:?}");
                    assert!(g.editor.bottom() <= g.buttons.top() + 0.5, "{g:?}");
                    assert!(g.composer.contains_rect(g.send_stop), "{g:?}");
                    assert!(g.send_stop.bottom() <= height && g.send_stop.right() <= width, "{g:?}");
                    assert!(g.send_stop.height() >= CONTROL, "{g:?}");
                    assert!(g.editor.height() >= 16.0, "{g:?}");
                }
            }
        }
    }

    #[test]
    fn markdown_keeps_formatting_but_never_requests_images_or_unsafe_links() {
        let input = "# 标题\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n```rust\nlet x = 1;\n```\n\n![远程](https://example.com/a.png) ![本地](file:///private.png) ![引用][image]\n\n[image]: https://example.com/b.png\n\n[安全](https://example.com) [危险](javascript:alert) [文件](file:///private) [相对](private.txt)\n<script>alert(1)</script>";
        let entry = MarkdownEntry::new(input, 0);
        assert_eq!(entry.source, input);
        assert!(!Parser::new_ext(&entry.display, markdown_options()).any(|e| matches!(e, Event::Start(Tag::Image { .. }))));
        assert!(entry.cache.link_hooks().contains_key("javascript:alert"));
        assert!(entry.cache.link_hooks().contains_key("file:///private"));
        assert!(entry.cache.link_hooks().contains_key("private.txt"));
        assert!(!entry.cache.link_hooks().contains_key("https://example.com"));
        assert!(entry.display.contains("```rust") && entry.display.contains("| a | b |"));
        assert!(!web_link("javascript:alert(1)"));
        assert!(!web_link("file:///private"));
        assert!(web_link("HTTPS://example.com"));
    }

    #[test]
    fn instance_state_shortcuts_and_cache_are_isolated() {
        let (_root, session, options) = fixture();
        let ctx = egui::Context::default();
        let mut a = ChatPanel::new("first");
        let mut b = ChatPanel::new("second");
        assert_ne!(a.id.with("draft"), b.id.with("draft"));
        a.history_search = "仅实例A".into();
        a.expanded = true;
        draw(&ctx, &mut a, &session, &options, Vec2::new(860.0, 560.0), 1.0, vec![]);
        draw(&ctx, &mut b, &session, &options, Vec2::new(860.0, 560.0), 1.0, vec![]);
        assert!(!b.expanded && b.history_search.is_empty());
        let focus = a.id.with("draft");
        ctx.memory_mut(|m| m.request_focus(focus));
        let enter = |repeat| egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat, modifiers: egui::Modifiers::CTRL };
        let actions = draw(&ctx, &mut a, &session, &options, Vec2::new(860.0, 560.0), 1.0,
            vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "拼".into(), active_range_chars: None }), enter(false)]);
        assert!(!actions.iter().any(|a| matches!(a, ChatAction::Send)));
        let actions = draw(&ctx, &mut a, &session, &options, Vec2::new(860.0, 560.0), 1.0,
            vec![egui::Event::Ime(egui::ImeEvent::Commit("拼".into())), enter(false)]);
        assert!(!actions.iter().any(|a| matches!(a, ChatAction::Send)));
        let actions = draw(&ctx, &mut a, &session, &options, Vec2::new(860.0, 560.0), 1.0,
            vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "候选".into(), active_range_chars: Some(0..2) }), enter(false)]);
        assert!(!actions.iter().any(|a| matches!(a, ChatAction::Send)));
        let actions = draw(&ctx, &mut a, &session, &options, Vec2::new(860.0, 560.0), 1.0,
            vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: String::new(), active_range_chars: None }), enter(false)]);
        assert!(!actions.iter().any(|a| matches!(a, ChatAction::Send)));
        let actions = draw(&ctx, &mut a, &session, &options, Vec2::new(860.0, 560.0), 1.0, vec![enter(true)]);
        assert!(!actions.iter().any(|a| matches!(a, ChatAction::Send)));
        draw(&ctx, &mut a, &session, &options, Vec2::new(860.0, 560.0), 1.0, vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::CTRL }]);
        let actions = draw(&ctx, &mut a, &session, &options, Vec2::new(860.0, 560.0), 1.0, vec![enter(false)]);
        assert!(actions.iter().any(|a| matches!(a, ChatAction::Send)));
        for i in 0..CACHE_ENTRIES + 10 {
            a.frame += 1;
            ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| a.markdown(ui, &i.to_string(), "正文"));
            }).drop_without_applying_deltas();
        }
        assert!(a.markdown.len() <= CACHE_ENTRIES);
        assert!(!a.markdown.contains_key("0"));
    }

    #[test]
    fn markdown_paragraph_wraps_in_actual_available_width() {
        let ctx = egui::Context::default();
        let mut panel = ChatPanel::new("wrap");
        let text = "这是一段没有空格的长中文正文，必须按照当前窗口宽度折行，不能无限向右延伸。".repeat(100);
        let mut measured = Rect::NOTHING;
        for _ in 0..3 {
            ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(480.0, 480.0))), ..Default::default() }, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    measured = ui.scope(|ui| { panel.markdown(ui, "paragraph", &text); }).response.rect;
                });
            }).drop_without_applying_deltas();
        }
        assert!(measured.width() <= 464.5, "{measured:?}");
        assert!(measured.height() > 100.0, "{measured:?}");
    }
    #[test]
    fn streaming_growth_follows_until_reader_scrolls_up() {
        let (_root, session, options) = fixture();
        let ctx = egui::Context::default();
        let mut panel = ChatPanel::new("follow");
        for _ in 0..3 { draw(&ctx, &mut panel, &session, &options, Vec2::new(860.0, 560.0), 1.0, vec![]); }
        assert!(panel.follow);
        assert!(panel.last_offset > 100.0);
        let pos = panel.geometry.transcript.center();
        draw(&ctx, &mut panel, &session, &options, Vec2::new(860.0, 560.0), 1.0, vec![
            egui::Event::PointerMoved(pos),
            egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(0.0, 120.0),
                phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Unknown, modifiers: egui::Modifiers::NONE },
        ]);
        assert!(!panel.follow);
        let offset = panel.last_offset;
        draw(&ctx, &mut panel, &session, &options, Vec2::new(860.0, 560.0), 1.0, vec![]);
        assert!(!panel.follow);
        assert!(panel.last_offset <= offset);
    }
    #[test]
    fn quick_prompt_appends_real_draft_and_creates_conversation_first() {
        let mut panel = ChatPanel::new("quick");
        panel.draft = "原文".into();
        let mut actions = Vec::new();
        panel.insert_prompt(QUICK_PROMPTS[0].1, &mut actions);
        assert!(matches!(actions.first(), Some(ChatAction::New)));
        assert!(matches!(actions.last(), Some(ChatAction::Draft(text)) if text == &format!("原文\n\n{}", QUICK_PROMPTS[0].1)));
        panel.draft = "/".into();
        panel.insert_prompt(QUICK_PROMPTS[1].1, &mut actions);
        assert_eq!(panel.draft, QUICK_PROMPTS[1].1);
        assert_eq!(reasoning_label(None), "默认");
        assert_eq!(reasoning_label(Some(ReasoningEffort::None)), "关闭");
    }
}
