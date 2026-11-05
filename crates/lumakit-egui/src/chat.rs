//! 参考 AutoNote 的紧凑聊天输入区；业务动作由宿主执行，安全本地图片内联预览。
use egui::{Align, Button, FontId, Frame, Id, Layout, Margin, Rect, RichText, ScrollArea, TextEdit, TextStyle, Ui, Vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use lumakit_ai::{ModelRef, chat::{Attempt, ChatSession, ToolbarTool}, history::TurnStatus, model_config::{ModelSettings, ReasoningEffort}};
use lumakit_ai::chat::{ChatAction, ModelOption};
use pulldown_cmark::{Event, Options, Parser, Tag};
use std::{collections::HashMap, fmt::Debug, hash::Hash};

#[path = "chat_usage.rs"]
mod usage;

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
    media: crate::chat_media::MediaPreview,
    usage: usage::UsageMenu,
    geometry: ChatGeometry,
}
impl ChatPanel {
    /// 同屏每个组件必须提供不同 ID。
    pub fn new(instance_id: impl Hash + Debug) -> Self {
        Self { id: Id::unique(instance_id), conversation: None, draft: String::new(), expanded: false,
            composing: false, history_open: false, quick_open: false, history_search: String::new(), delete: None, edit: None,
            model_search: String::new(), follow: true, to_bottom: true,
            last_offset: 0.0, frame: 0, markdown: HashMap::new(), media: Default::default(), usage: Default::default(), geometry: ChatGeometry::default() }
    }
    /// 上次 show 捕获的实际布局，不重新计算坐标。
    pub fn geometry(&self) -> &ChatGeometry { &self.geometry }

    /// 仅绘制并返回动作；设置始终属于 history.model 聊天模型，生图目录仅供工具选择。
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
            self.usage.close();
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
            let unlocked = session.phase() == lumakit_ai::chat::ChatPhase::Idle && !session.save_failed();
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
            let toolbar = session.toolbar();
            let mut web_search = composer.web_search;
            let mut reasoning = composer.reasoning;
            let mut image_mode = composer.image_mode;
            let mut image_model = composer.image_model.as_ref();
            let picked = session.history().model.as_ref();
            let valid_chat = picked.is_some_and(|model| options.iter().any(|option| same_model(&option.model, model)));
            let tools_supported = model_settings.is_none_or(|settings| settings.supports_tools != Some(false));
            let valid_image = image_model.is_some_and(|model| image_options.iter().any(|option| same_model(&option.model, model)));
            let valid_model = valid_chat && (!composer.image_mode || (valid_image && tools_supported));
            let model_error = if options.is_empty() { "尚未配置聊天模型，请打开模型设置。" }
                else if !valid_chat { "请选择有效聊天模型。" }
                else if composer.image_mode && !tools_supported { "当前聊天模型不支持工具调用，请更换聊天模型或关闭生图。" }
                else if composer.image_mode && image_options.is_empty() { "尚未配置生图模型，请打开模型设置或关闭生图。" }
                else if composer.image_mode && !valid_image { "请选择有效生图模型，聊天将通过工具调用生成图片。" } else { "" };
            let has_notice = !session.notice().is_empty() || !model_error.is_empty() || session.save_failed();
            let attachment_height = if session.draft_attachments().is_empty() { 0.0 } else { 36.0 };
            let narrow_toolbar = ui.available_width() - 40.0 < 560.0;
            let toolbar_height = if narrow_toolbar { CONTROL * 2.0 + 8.0 } else { CONTROL };
            let composer_overhead = 40.0 + toolbar_height;
            let budget = (ui.available_height() - composer_overhead - attachment_height).max(16.0);
            let notice_height = if has_notice { 40.0_f32.min(budget * 0.35) } else { 0.0 };
            let editor_height = (if self.expanded { ui.available_height() * 0.4 } else { 64.0 })
                .min((budget - notice_height - if has_notice { 8.0 } else { 0.0 }).max(16.0));
            let composer_height = editor_height + composer_overhead + attachment_height + notice_height + if has_notice { 8.0 } else { 0.0 };
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
                                    .hint_text("输入消息…")).accessible_name("消息输入")
                            }).inner
                        });
                    self.geometry.editor = editor.inner_rect;
                    if editor.inner.changed() {
                        if selected.is_none() { actions.push(ChatAction::New); }
                        actions.push(ChatAction::Draft(self.draft.clone()));
                    }
                    self.geometry.buttons = ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        ui.horizontal(|ui| {
                            if toolbar.contains(&ToolbarTool::NewConversation) && crate::icons::action(ui, unlocked, "\u{E34C}", "新对话").clicked() { actions.push(ChatAction::New); }
                            ui.add_enabled_ui(unlocked, |ui| {
                                egui::containers::menu::MenuButton::from_button(crate::icons::button("\u{E3D4}")).ui(ui, |ui| {
                                    if !toolbar.contains(&ToolbarTool::NewConversation) && ui.button("新对话").clicked() { actions.push(ChatAction::New); ui.close(); }
                                    if !toolbar.contains(&ToolbarTool::WebSearch) && ui.selectable_label(web_search, "网络搜索").clicked() { web_search = !web_search; ui.close(); }
                                    if !toolbar.contains(&ToolbarTool::Attachment) && ui.button("上传附件").on_hover_text("最多 4 个附件；UTF-8 文本合计不超过 1 MiB，图片每张不超过 4 MiB").clicked() { actions.push(ChatAction::UploadAttachment); ui.close(); }
                                    let painting_notice = if !valid_chat { "请先配置并选择有效聊天模型。" }
                                        else if !tools_supported { "当前聊天模型不支持工具调用，请更换聊天模型。" }
                                        else if image_options.is_empty() { "尚未配置生图模型，请打开模型设置。" }
                                        else { "选择生图模型后，聊天可调用图片生成工具。" };
                                    if !toolbar.contains(&ToolbarTool::GenerateImage) && ui.add_enabled(image_mode || (valid_chat && tools_supported && valid_image), Button::new("生成图片").selected(image_mode)).on_hover_text(painting_notice).clicked() { image_mode = !image_mode; ui.close(); }
                                    ui.add_enabled_ui(valid_chat && tools_supported && !image_options.is_empty(), |ui| {
                                        ui.menu_button("生图模型", |ui| {
                                            ui.label(RichText::new("选择生图模型").weak().small());
                                            for option in image_options {
                                                if ui.selectable_label(image_model.is_some_and(|m| same_model(m, &option.model)), model_label(option)).clicked() {
                                                    image_model = Some(&option.model);
                                                    image_mode = true;
                                                    ui.close();
                                                }
                                            }
                                        }).response.on_hover_text(painting_notice);
                                    });
                                    if !valid_chat || !tools_supported || image_options.is_empty() { ui.label(RichText::new(painting_notice).weak()); }
                                    if ui.button("快捷面板").clicked() { self.quick_open = true; ui.close(); }
                                    if ui.button(if self.expanded { "收起输入" } else { "展开输入" }).clicked() { self.expanded = !self.expanded; ui.close(); }
                                    ui.label("Ctrl / ⌘ + Enter 发送；Enter 换行");
                                }).0.on_hover_text("添加附件与输入工具").accessible_name("添加附件与输入工具");
                                if image_mode && !toolbar.contains(&ToolbarTool::GenerateImage) {
                                    let source = image_model.and_then(|m| image_options.iter().find(|o| same_model(m, &o.model)));
                                    if ui.add(Button::new("生成图片 ×").selected(true)).on_hover_text(source.map(model_label).unwrap_or_else(|| "尚未选择有效生图模型".into()))
                                        .accessible_name("关闭生成图片").clicked() { image_mode = false; }
                                }
                            });
                            if toolbar.contains(&ToolbarTool::Attachment) && crate::icons::action(ui, unlocked, "\u{E39A}", "上传附件").clicked() { actions.push(ChatAction::UploadAttachment); }
                            if toolbar.contains(&ToolbarTool::GenerateImage) && ui.add_enabled(unlocked && (image_mode || (valid_chat && tools_supported && valid_image)), crate::icons::button("\u{E2C8}").selected(image_mode))
                                .on_hover_text("生成图片；在添加菜单中选择生图模型").accessible_name("生成图片").clicked() { image_mode = !image_mode; }
                            if toolbar.contains(&ToolbarTool::WebSearch) && ui.add_enabled(unlocked, crate::icons::button("\u{E288}").selected(web_search))
                                .on_hover_text("网络搜索").accessible_name("网络搜索").clicked() { web_search = !web_search; }
                            if !narrow_toolbar {
                                ui.add_enabled_ui(unlocked, |ui| {
                                    model_picker(ui, self.id.with("models"), 136.0, &mut self.model_search, options, picked, &mut actions);
                                });
                                self.send_tools(ui, session, unlocked, valid_model, shortcut, &mut reasoning, model_settings, &mut actions);
                            }
                        });
                        if narrow_toolbar {
                            // 窄窗主动换行，避免左侧标签挤出发送键或覆盖模型名。
                            ui.horizontal(|ui| {
                                let picker_width = (ui.available_width() - 140.0).clamp(64.0, 136.0);
                                ui.add_enabled_ui(unlocked, |ui| {
                                    model_picker(ui, self.id.with("models"), picker_width, &mut self.model_search, options, picked, &mut actions);
                                });
                                self.send_tools(ui, session, unlocked, valid_model, shortcut, &mut reasoning, model_settings, &mut actions);
                            });
                        }
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
                                        for attempt in previous { self.answer(ui, session, &conversation.id, &turn.id, attempt, false, unlocked, options, image_options, &mut actions); }
                                    });
                                }
                                self.answer(ui, session, &conversation.id, &turn.id, last, index + 1 == conversation.turns.len(), unlocked, options, image_options, &mut actions);
                            }
                            ui.add_space(12.0);
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
            self.quick_modal(ui, unlocked, session.toolbar(), &mut actions);
            self.edit_modal(ui, &mut actions);
        });
        self.markdown.retain(|_, entry| self.frame.wrapping_sub(entry.used) <= 120);
        actions
    }

    fn send_tools(&mut self, ui: &mut Ui, session: &ChatSession, unlocked: bool, valid_model: bool, shortcut: bool,
        reasoning: &mut Option<ReasoningEffort>, model_settings: Option<&ModelSettings>, actions: &mut Vec<ChatAction>) {
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
                self.usage.close();
                if crate::icons::action(ui, true, "\u{E038}", "重试保存").clicked() { actions.push(ChatAction::Save); }
            } else if !self.follow {
                self.usage.close();
                if crate::icons::action(ui, true, "\u{E03E}", "回到底部").clicked() { self.follow = true; self.to_bottom = true; }
            } else {
                self.usage.show(ui, session, model_settings);
            }
            ui.add_enabled_ui(unlocked, |ui| {
                ui.menu_button(reasoning_label(*reasoning), |ui| {
                    ui.label(RichText::new("推理强度").weak().small());
                    ui.selectable_value(reasoning, None, "默认");
                    if let Some(settings) = model_settings {
                        for &effort in settings.effective_model().supported_efforts() { ui.selectable_value(reasoning, Some(effort), reasoning_label(Some(effort))); }
                    }
                }).response.on_hover_text("推理强度：默认交由服务商处理，不发送指定档位").accessible_name("推理强度");
            });
        });
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

    fn answer(&mut self, ui: &mut Ui, session: &ChatSession, conversation_id: &str, turn_id: &str, attempt: &Attempt, retry: bool, unlocked: bool,
        options: &[ModelOption], image_options: &[ModelOption], actions: &mut Vec<ChatAction>) {
        ui.push_id(&attempt.id, |ui| {
            Frame::new().fill(ui.visuals().faint_bg_color).corner_radius(8).inner_margin(12)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let image_attempt = !options.iter().any(|option| same_model(&option.model, &attempt.model))
                        && !attempt.tool_events.iter().any(|event| event.name == "generate_image")
                        && (image_options.iter().any(|option| same_model(&option.model, &attempt.model)) || !attempt.generated.is_empty());
                    let source = options.iter().chain(image_options).find(|option| same_model(&option.model, &attempt.model));
                    ui.label(RichText::new(source.map_or(attempt.model.model.as_str(), |option| option.name.as_str())).weak().size(12.0))
                        .on_hover_text(source.map(model_label).unwrap_or_else(|| format!("{} · {} / {}", attempt.model.model, attempt.model.provider, attempt.model.endpoint)));
                    if !attempt.reasoning.is_empty() {
                        egui::CollapsingHeader::new("思考过程").id_salt("reasoning").show(ui, |ui| {
                            ScrollArea::vertical().id_salt("reasoning-text").max_height(180.0).show(ui, |ui| {
                                for block in &attempt.reasoning {
                                    ui.label(block.get("text").or_else(|| block.get("thinking")).and_then(|text| text.as_str()).map_or_else(|| block.to_string(), str::to_owned));
                                }
                            });
                        });
                    }
                    for event in &attempt.tool_events {
                        let summary = tool_summary(event);
                        let text = RichText::new(summary).small();
                        let text = if event.is_error { text.color(ui.visuals().error_fg_color) } else { text.weak() };
                        egui::CollapsingHeader::new(text).id_salt(("tool-details", &event.id)).default_open(false).show(ui, |ui| {
                            ScrollArea::vertical().id_salt(("tool-text", &event.id)).max_height(180.0).show(ui, |ui| {
                                ui.label(format!("工具：{}\n调用 ID：{}", event.name, event.id));
                                ui.label(format!("参数：{}", event.arguments));
                                if let Some(result) = &event.result { ui.label(result); }
                                if let Some(error) = &event.error { ui.colored_label(ui.visuals().error_fg_color, error); }
                            });
                        });
                    }
                    if !attempt.tool_events.is_empty() || !attempt.reasoning.is_empty() { ui.add_space(4.0); }
                    self.media.show(ui, session, &attempt.generated, actions);
                    if !attempt.generated.is_empty() { ui.add_space(4.0); }
                    if attempt.text.is_empty() && attempt.status == TurnStatus::Running { ui.spinner(); ui.label("正在等待回复…"); }
                    if !attempt.text.is_empty() { self.markdown(ui, &attempt.id, &attempt.text); }
                    match attempt.status {
                        TurnStatus::Running if !attempt.text.is_empty() => { ui.label(RichText::new("正在生成…").weak().small()); }
                        TurnStatus::Cancelled => { ui.label(RichText::new("已停止，保留部分回答").weak().small()); }
                        TurnStatus::Failed if !attempt.tool_events.iter().any(|event| event.is_error) => {
                            ui.label(RichText::new("回答失败，请重试或更换模型").color(ui.visuals().error_fg_color).small());
                        }
                        _ => {}
                    }
                    if let Some(error) = &attempt.error {
                        egui::CollapsingHeader::new("诊断详情").id_salt("attempt-error").default_open(false).show(ui, |ui| {
                            ScrollArea::vertical().id_salt("error").max_height(120.0).show(ui, |ui| {
                                ui.colored_label(ui.visuals().error_fg_color, error);
                            });
                        });
                    }
                    if attempt.status != TurnStatus::Running || !attempt.rounds.is_empty() {
                        egui::CollapsingHeader::new(RichText::new("用量详情").weak().small()).id_salt("usage").default_open(false).show(ui, |ui| {
                            usage::show_rounds(ui, &attempt.rounds);
                            ui.label(RichText::new("按请求保存的用量与价格快照；费用是 token 估算，不是账单。输入已含缓存，推理属于输出子集，均不重复相加。").weak().small());
                        });
                    }
                    ui.add_space(4.0);
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
                        // 次要写入动作收进更多菜单，避免操作行挤占窄窗正文。
                        if crate::icons::action(ui, unlocked, "\u{E2F0}", "选择上下文").clicked() { actions.push(ChatAction::SelectAttempt { conversation_id: conversation_id.into(), turn_id: turn_id.into(), attempt_id: attempt.id.clone() }); }
                        egui::containers::menu::MenuButton::from_button(crate::icons::button("\u{E1FE}")).ui(ui, |ui| {
                            if ui.add_enabled(complete && !attempt.text.is_empty(), Button::new("保存到笔记")).clicked() { ui.close(); actions.push(ChatAction::SaveNote { attempt_id: attempt.id.clone(), markdown: attempt.text.clone() }); }
                            if ui.add_enabled(complete && !attempt.text.is_empty(), Button::new("插入编辑器")).clicked() { ui.close(); actions.push(ChatAction::InsertMarkdown { attempt_id: attempt.id.clone(), markdown: attempt.text.clone() }); }
                            if ui.add_enabled(complete, Button::new("编辑")).clicked() { ui.close(); self.edit = Some((conversation_id.into(), turn_id.into(), attempt.id.clone(), attempt.text.clone(), attempt.text.clone())); }
                            if ui.add_enabled(unlocked, Button::new("分支")).clicked() { ui.close(); actions.push(ChatAction::ForkAttempt { conversation_id: conversation_id.into(), turn_id: turn_id.into(), attempt_id: attempt.id.clone() }); }
                            if ui.add_enabled(unlocked, Button::new("复制对话")).clicked() { ui.close(); actions.push(ChatAction::CopyConversation { conversation_id: conversation_id.into() }); }
                            ui.separator();
                            if ui.add_enabled(complete && !attempt.text.is_empty(), Button::new("导出 Markdown")).clicked() { ui.close(); actions.push(ChatAction::ExportMarkdown { attempt_id: attempt.id.clone(), markdown: attempt.text.clone() }); }
                            if ui.add_enabled(unlocked, Button::new("删除回答")).clicked() { ui.close(); actions.push(ChatAction::DeleteAttempt { conversation_id: conversation_id.into(), turn_id: turn_id.into(), attempt_id: attempt.id.clone() }); }
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
    fn quick_modal(&mut self, ui: &Ui, unlocked: bool, toolbar: &[ToolbarTool], actions: &mut Vec<ChatAction>) {
        if !self.quick_open { return; }
        let mut pending = toolbar.to_vec();
        let response = egui::Modal::new(self.id.with("quick-toolbar")).show(ui.ctx(), |ui| {
            ui.set_max_width((ui.ctx().content_rect().width() - 48.0).min(360.0));
            ui.heading("自定义工具栏");
            ui.add_enabled_ui(unlocked, |ui| {
                for (tool, label) in TOOLBAR_TOOLS {
                    toolbar_switch(ui, &mut pending, *tool, label);
                }
                ui.separator();
                if ui.add_enabled(pending != DEFAULT_TOOLBAR, Button::new("恢复默认")).clicked() { pending = DEFAULT_TOOLBAR.to_vec(); }
            });
            if ui.button("关闭").clicked() { self.quick_open = false; }
        });
        if pending != toolbar { actions.push(ChatAction::SetToolbar(pending)); }
        if response.should_close() { self.quick_open = false; }
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
            let unlocked = session.phase() == lumakit_ai::chat::ChatPhase::Idle && !session.save_failed();
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
const DEFAULT_TOOLBAR: &[ToolbarTool] = &[ToolbarTool::NewConversation, ToolbarTool::WebSearch];
const TOOLBAR_TOOLS: &[(ToolbarTool, &str)] = &[
    (ToolbarTool::NewConversation, "新对话"),
    (ToolbarTool::WebSearch, "网络搜索"),
    (ToolbarTool::Attachment, "上传附件"),
    (ToolbarTool::GenerateImage, "生成图片"),
];

fn toolbar_switch(ui: &mut Ui, toolbar: &mut Vec<ToolbarTool>, tool: ToolbarTool, label: &str) -> egui::Response {
    let mut pinned = toolbar.contains(&tool);
    let response = crate::switch(ui, &mut pinned, label);
    if response.changed() {
        *toolbar = TOOLBAR_TOOLS.iter().map(|(item, _)| *item)
            .filter(|item| if *item == tool { pinned } else { toolbar.contains(item) }).collect();
    }
    response
}

fn reasoning_label(effort: Option<ReasoningEffort>) -> &'static str {
    match effort {
        None => "默认", Some(ReasoningEffort::None) => "关闭", Some(ReasoningEffort::Auto) => "自动",
        Some(ReasoningEffort::Minimal) => "最小", Some(ReasoningEffort::Low) => "低",
        Some(ReasoningEffort::Medium) => "中", Some(ReasoningEffort::High) => "高",
        Some(ReasoningEffort::Xhigh) => "超高", Some(ReasoningEffort::Max) => "最大",
    }
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

fn model_picker(ui: &mut Ui, id: Id, width: f32, search: &mut String, options: &[ModelOption],
    picked: Option<&ModelRef>, actions: &mut Vec<ChatAction>) {
    let source = picked.and_then(|model| options.iter().find(|option| same_model(&option.model, model)));
    let label = source.map_or("选择模型", |option| option.name.as_str());
    egui::ComboBox::from_id_salt(id).width(width).selected_text(label)
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
                    actions.push(ChatAction::SelectModel(option.model.clone()));
                    ui.close();
                }
            }
            if !found { ui.label("没有匹配的模型"); }
        }).response.on_hover_text(source.map(model_label).unwrap_or_else(|| "选择聊天模型".into())).accessible_name("聊天模型");
}

fn tool_summary(event: &lumakit_ai::ToolEvent) -> String {
    let name = if event.name == "generate_image" { "图片生成" } else { &event.name };
    let status = if event.is_error { "失败，请重试或更换模型" }
        else if event.result.is_some() { "完成" } else { "进行中…" };
    format!("{name}{status}")
}


#[cfg(test)]
mod tests {
    use super::*;
    use lumakit_ai::{Protocol, chat::{Conversation, History, Turn}};
    use std::sync::Arc;


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
                        input_tokens: 123, output_tokens: 45, cache_read_tokens: 67, cache_write_tokens: 8, ..Default::default()
                    }), pricing: None, cost: None, ..Default::default() }], kind: lumakit_ai::chat::AttemptKind::Reply,
                    source_attempt_id: None, parent_attempt_id: None, search: Default::default(), tool_events: Vec::new(), reasoning: Vec::new(), generated: Vec::new(),
                }], selected_attempt: None }],
            }],
        };
        let path = root.path().join("chat-history.json");
        lumakit_support::save_versioned(&path, 1, &history).unwrap();
        let session = ChatSession::open(&root.path().join("chat.db"), Arc::new(|| {})).unwrap();
        let options = vec![ModelOption { model, name: "模型".into(), provider: "来源A".into(), endpoint: "端点A".into(), settings: Default::default(), has_key: false }];
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
    fn failed_generator_keeps_raw_diagnostics_and_rounds_out_of_default_view() {
        let (_root, session, options) = fixture();
        let mut attempt = session.selected().unwrap().turns[0].attempts[0].clone();
        attempt.text.clear();
        attempt.error = Some("RAW_ATTEMPT_DIAGNOSTIC".into());
        attempt.tool_events.push(lumakit_ai::ToolEvent {
            id: "private-request-id".into(), name: "generate_image".into(), arguments: Default::default(),
            approval: None, result: Some("RAW_GENERATOR_JSON".into()), is_error: true,
            error: Some("RAW_UPSTREAM_ERROR".into()),
        });
        let ctx = egui::Context::default();
        let mut panel = ChatPanel::new("diagnostics");
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                panel.answer(ui, &session, "conversation", "turn", &attempt, true, true, &options, &[], &mut Vec::new());
            });
        });
        fn visible_text(shape: &egui::Shape, text: &mut String) {
            match shape {
                egui::Shape::Text(shape) => text.push_str(shape.galley.text()),
                egui::Shape::Vec(shapes) => for shape in shapes { visible_text(shape, text); },
                _ => {}
            }
        }
        let mut text = String::new();
        for shape in &output.shapes { visible_text(&shape.shape, &mut text); }
        assert!(text.contains(&tool_summary(&attempt.tool_events[0])));
        for diagnostic in ["RAW_ATTEMPT_DIAGNOSTIC", "RAW_GENERATOR_JSON", "RAW_UPSTREAM_ERROR", "private-request-id"] {
            assert!(!text.contains(diagnostic), "{text}");
        }
        assert!(!text.contains(&usage::round_label(&attempt.rounds[0])));
        output.drop_without_applying_deltas();
    }


    #[test]
    fn actual_layout_keeps_footer_reachable_for_long_content() {
        let (_root, mut session, options) = fixture();
        session.report("保存或请求错误，详细说明\n".repeat(300));
        for (width, height, scale) in [(860.0, 560.0, 1.0), (1200.0, 800.0, 1.0), (860.0, 560.0, 2.0), (860.0, 220.0, 1.0), (768.0, 480.0, 1.0), (414.0, 480.0, 1.0), (375.0, 480.0, 1.0), (320.0, 480.0, 1.0), (320.0, 900.0, 1.0)] {
            for expanded in [false, true] {
                for toolbar in [DEFAULT_TOOLBAR.to_vec(), TOOLBAR_TOOLS.iter().map(|(tool, _)| *tool).collect()] {
                    session.set_toolbar(toolbar);
                for image_mode in [false, true] {
                    session.set_composer(lumakit_ai::chat::ComposerSettings { image_mode, ..Default::default() });
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
                    assert!(g.buttons.left() >= g.composer.left() && g.buttons.right() <= g.composer.right() + 0.5, "{g:?}");
                    assert!(g.buttons.contains_rect(g.send_stop), "{g:?}");
                    assert!(g.send_stop.height() >= CONTROL, "{g:?}");
                    assert!(g.editor.height() >= 16.0, "{g:?}");
                }
                }
                }
            }
        }
    }

    #[test]
    fn painting_controls_preserve_chat_settings_and_validate_both_models() {
        let (_root, mut session, options) = fixture();
        let ctx = egui::Context::default();
        let mut panel = ChatPanel::new("painting");
        let focus = panel.id.with("draft");
        let painting = ModelOption { model: ModelRef { model: "painting".into(), ..options[0].model.clone() }, name: "绘图".into(), provider: "来源A".into(), endpoint: "端点A".into(), settings: ModelSettings { model_type: lumakit_ai::model_config::ModelType::Image, ..Default::default() }, has_key: false };
        session.set_composer(lumakit_ai::chat::ComposerSettings { web_search: true, reasoning: Some(ReasoningEffort::High), image_mode: true, image_model: Some(painting.model.clone()) });
        let mut frame = |session: &ChatSession, options: &[ModelOption], images: &[ModelOption], settings: Option<&ModelSettings>, events| {
            let mut actions = Vec::new();
            ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(375.0, 480.0))), events, ..Default::default() }, |ui| {
                egui::CentralPanel::default().show(ui, |ui| { actions = panel.show(ui, session, options, images, settings); });
            }).drop_without_applying_deltas();
            actions
        };
        frame(&session, &options, std::slice::from_ref(&painting), None, vec![]);
        ctx.memory_mut(|memory| memory.request_focus(focus));
        let enter = || egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::CTRL };
        let actions = frame(&session, &options, std::slice::from_ref(&painting), None, vec![enter()]);
        assert!(actions.iter().any(|action| matches!(action, ChatAction::Send)));
        assert!(!actions.iter().any(|action| matches!(action, ChatAction::SetComposer(_))));
        let unsupported = ModelSettings { supports_tools: Some(false), ..Default::default() };
        for (chat, images, settings) in [(&options[..], &[][..], None), (&[][..], std::slice::from_ref(&painting), None), (&options[..], std::slice::from_ref(&painting), Some(&unsupported))] {
            let actions = frame(&session, chat, images, settings, vec![enter()]);
            assert!(!actions.iter().any(|action| matches!(action, ChatAction::Send)));
        }
        session.set_composer(lumakit_ai::chat::ComposerSettings { web_search: true, reasoning: Some(ReasoningEffort::High), image_mode: true, image_model: None });
        let actions = frame(&session, &options, std::slice::from_ref(&painting), None, vec![enter()]);
        assert!(!actions.iter().any(|action| matches!(action, ChatAction::Send | ChatAction::SetComposer(_))));
        assert!(session.composer().web_search);
        assert_eq!(session.composer().reasoning, Some(ReasoningEffort::High));
        assert!(session.history().model.as_ref().is_some_and(|model| same_model(model, &options[0].model)));
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
    fn toolbar_switch_only_changes_pinning_not_draft_or_request_settings() {
        let (_root, mut session, options) = fixture();
        session.set_composer(lumakit_ai::chat::ComposerSettings { web_search: true, image_mode: true, ..Default::default() });
        let draft = session.selected().unwrap().draft.clone();
        let ctx = egui::Context::default();
        let mut toolbar = TOOLBAR_TOOLS.iter().map(|(tool, _)| *tool).collect::<Vec<_>>();
        for (tool, label) in TOOLBAR_TOOLS {
            let mut frame = |events| {
                let mut response = None;
                ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
                    response = Some(toolbar_switch(ui, &mut toolbar, *tool, label));
                }).drop_without_applying_deltas();
                response.unwrap()
            };
            let response = frame(vec![]);
            ctx.memory_mut(|memory| memory.request_focus(response.id));
            assert!(frame(vec![egui::Event::Key { key: egui::Key::Space, physical_key: None, pressed: true,
                repeat: false, modifiers: egui::Modifiers::NONE }]).changed());
            frame(vec![egui::Event::Key { key: egui::Key::Space, physical_key: None, pressed: false,
                repeat: false, modifiers: egui::Modifiers::NONE }]);
            assert!(!toolbar.contains(tool));
        }
        session.set_toolbar(toolbar);
        let mut panel = ChatPanel::new("pin-only");
        panel.quick_open = true;
        let actions = draw(&ctx, &mut panel, &session, &options, Vec2::new(375.0, 480.0), 1.0, vec![]);
        assert!(actions.is_empty());
        assert_eq!(panel.draft, draft);
        assert_eq!(session.selected().unwrap().draft, draft);
        assert!(session.composer().web_search);
        assert!(session.composer().image_mode);
        assert!(session.toolbar().is_empty());
    }
}
