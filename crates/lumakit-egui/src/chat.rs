//! 固定头尾分区的纯文本聊天；业务动作一律由宿主执行。
use egui::{Align, Button, FontId, Frame, Id, Layout, Margin, Rect, RichText, ScrollArea, TextEdit, TextStyle, Ui, Vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use lumakit_ai::{ModelRef, chat::{Attempt, ChatSession}, history::TurnStatus};
use lumakit_ui::chat::{ChatAction, ModelOption};
use pulldown_cmark::{Event, Options, Parser, Tag};
use std::{collections::HashMap, hash::Hash};

const CONTROL: f32 = 30.0;
const CACHE_ENTRIES: usize = 64;
const CACHE_BYTES: usize = 4 * 1024 * 1024;

/// 当前帧真实绘制区域（逻辑点）；测试和宿主可检查遮挡。
#[derive(Clone, Copy, Debug)]
pub struct ChatGeometry {
    pub header: Rect,
    pub transcript: Rect,
    pub composer: Rect,
    pub editor: Rect,
    pub buttons: Rect,
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
pub struct ChatPanel {
    id: Id,
    conversation: Option<String>,
    draft: String,
    expanded: bool,
    composing: bool,
    history_open: bool,
    history_search: String,
    delete: Option<String>,
    model_search: String,
    retry_search: String,
    follow: bool,
    to_bottom: bool,
    last_offset: f32,
    frame: u64,
    markdown: HashMap<String, MarkdownEntry>,
    geometry: ChatGeometry,
}
impl ChatPanel {
    /// 同屏每个组件必须提供不同 ID。
    pub fn new(instance_id: impl Hash) -> Self {
        Self { id: Id::new(instance_id), conversation: None, draft: String::new(), expanded: false,
            composing: false, history_open: false, history_search: String::new(), delete: None,
            model_search: String::new(), retry_search: String::new(), follow: true, to_bottom: true,
            last_offset: 0.0, frame: 0, markdown: HashMap::new(), geometry: ChatGeometry::default() }
    }
    /// 上次 show 捕获的实际布局，不重新计算坐标。
    pub fn geometry(&self) -> &ChatGeometry { &self.geometry }

    /// 仅绘制并返回动作；调用者应在绘制后依次处理。
    pub fn show(&mut self, ui: &mut Ui, session: &ChatSession, options: &[ModelOption]) -> Vec<ChatAction> {
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
            self.geometry.header = egui::TopBottomPanel::top(self.id.with("header"))
                .exact_height(88.0).frame(Frame::new().inner_margin(Margin::symmetric(12, 8)))
                .show_inside(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_sized([ui.available_width() - 160.0, CONTROL], egui::Label::new(
                            RichText::new(selected.map_or("新对话", |c| c.title.as_str())).strong()).truncate());
                        if ui.add_enabled(unlocked, Button::new("历史")).clicked() { self.history_open = true; }
                        if ui.add_enabled(unlocked, Button::new("新对话")).clicked() { actions.push(ChatAction::New); }
                    });
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(unlocked, |ui| {
                            let picked = session.history().model.as_ref();
                            let label = picked.and_then(|model| options.iter().find(|o| same_model(&o.model, model)))
                                .map(model_label).unwrap_or_else(|| if options.is_empty() { "尚未配置模型".into() } else { "请选择有效模型".into() });
                            model_picker(ui, self.id.with("models"), &label, &mut self.model_search,
                                options, picked, false, &mut actions);
                        });
                        if ui.button("模型设置").clicked() { actions.push(ChatAction::OpenSettings); }
                    });
                }).response.rect;

            let valid_model = session.history().model.as_ref()
                .is_some_and(|model| options.iter().any(|option| same_model(&option.model, model)));
            let model_error = if options.is_empty() { "尚未配置聊天模型，请打开模型设置。" }
                else if !valid_model { "所选模型已失效，请重新选择模型。" } else { "" };
            let has_notice = !session.notice().is_empty() || !model_error.is_empty() || session.save_failed();
            let budget = (ui.available_height() - 12.0 - 54.0).max(16.0);
            let notice_height = if has_notice { 48.0_f32.min(budget * 0.45) } else { 0.0 };
            let editor_height = (if self.expanded { ui.available_height() * 0.4 } else { 80.0 })
                .min((budget - notice_height - if has_notice { 8.0 } else { 0.0 }).max(16.0));
            let composer_height = editor_height + 54.0 + notice_height + if has_notice { 8.0 } else { 0.0 };
            self.geometry.composer = egui::TopBottomPanel::bottom(self.id.with("composer"))
                .exact_height(composer_height).frame(Frame::new().inner_margin(Margin::symmetric(12, 8)))
                .show_inside(ui, |ui| {
                    if has_notice {
                        ScrollArea::vertical().id_salt(self.id.with("notice")).max_height(notice_height)
                            .min_scrolled_height(notice_height)
                            .auto_shrink([false, false]).show(ui, |ui| {
                                if session.save_failed() { ui.colored_label(ui.visuals().error_fg_color, "保存失败，恢复保存前不能修改聊天。"); }
                                if !model_error.is_empty() { ui.colored_label(ui.visuals().error_fg_color, model_error); }
                                if !session.notice().is_empty() { ui.colored_label(ui.visuals().error_fg_color, session.notice()); }
                            });
                    }
                    let shortcut = self.consume_send_shortcut(ui, unlocked && valid_model);
                    let editor = ScrollArea::vertical().id_salt(self.id.with("draft-scroll"))
                        .max_height(editor_height).min_scrolled_height(editor_height).auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add_enabled_ui(unlocked, |ui| {
                                ui.add(TextEdit::multiline(&mut self.draft).id(self.id.with("draft"))
                                    .desired_width(f32::INFINITY).desired_rows(4)
                                    .hint_text("输入消息，Ctrl+Enter 发送"))
                            }).inner
                        });
                    self.geometry.editor = editor.inner_rect;
                    if editor.inner.changed() {
                        if selected.is_none() { actions.push(ChatAction::New); }
                        actions.push(ChatAction::Draft(self.draft.clone()));
                    }
                    self.geometry.buttons = ui.horizontal(|ui| {
                        if !self.follow {
                            if ui.add_sized([94.0, CONTROL], Button::new("回到底部")).clicked() { self.follow = true; self.to_bottom = true; }
                        } else if ui.add_sized([82.0, CONTROL], Button::new(if self.expanded { "收起输入" } else { "展开输入" })).clicked() { self.expanded = !self.expanded; }
                        ui.label(RichText::new("Ctrl+Enter 发送").weak().small());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let send = if session.busy() {
                                let response = ui.add_sized([76.0, CONTROL], Button::new("停止"));
                                if response.clicked() { actions.push(ChatAction::Stop); }
                                response
                            } else {
                                let response = ui.add_enabled(unlocked && valid_model && !self.draft.trim().is_empty(),
                                    Button::new("发送").min_size(Vec2::new(76.0, CONTROL)));
                                if response.clicked() || (shortcut && !self.draft.trim().is_empty()) {
                                    actions.push(ChatAction::Send); self.follow = true; self.to_bottom = true;
                                }
                                response
                            };
                            self.geometry.send_stop = send.rect;
                            if session.save_failed() && ui.add_sized([88.0, CONTROL], Button::new("重试保存")).clicked() { actions.push(ChatAction::Save); }
                        });
                    }).response.rect;
                }).response.rect;

            let remaining = ui.available_rect_before_wrap();
            self.geometry.transcript = remaining;
            let upward = ui.input(|input| input.pointer.hover_pos().is_some_and(|p| remaining.contains(p))
                && input.raw_scroll_delta.y > 0.0);
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
                                    });
                            });
                            ui.add_space(12.0);
                            if let Some((last, previous)) = turn.attempts.split_last() {
                                if !previous.is_empty() {
                                    egui::CollapsingHeader::new(format!("之前的 {} 次回答", previous.len()))
                                        .id_salt("attempts").show(ui, |ui| {
                                            for attempt in previous { self.answer(ui, attempt, false, unlocked, options, &mut actions); }
                                        });
                                }
                                self.answer(ui, last, index + 1 == conversation.turns.len(), unlocked, options, &mut actions);
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
            if output.state.offset.y + 1.0 < self.last_offset && ui.input(|i| i.pointer.primary_down()) { self.follow = false; }
            if !upward && output.state.offset.y >= max_offset - 8.0 && output.state.offset.y > self.last_offset + 1.0 { self.follow = true; }
            self.last_offset = output.state.offset.y;
            self.history_modal(ui, session, &mut actions);
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
                self.composing = matches!(event, egui::ImeEvent::Enabled | egui::ImeEvent::Preedit(_));
            }
        });
        let can_send = enabled && !self.composing && !ime_frame && !self.history_open;
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

    fn answer(&mut self, ui: &mut Ui, attempt: &Attempt, retry: bool, unlocked: bool,
        options: &[ModelOption], actions: &mut Vec<ChatAction>) {
        ui.push_id(&attempt.id, |ui| {
            Frame::new().fill(ui.visuals().faint_bg_color).corner_radius(8).inner_margin(12)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let source = options.iter().find(|option| same_model(&option.model, &attempt.model));
                    ui.label(RichText::new(source.map(model_label).unwrap_or_else(|| format!("{} · {} / {}",
                        attempt.model.model, attempt.model.provider, attempt.model.endpoint))).color(ui.visuals().text_color()).size(12.0));
                    ui.add_space(4.0);
                    if attempt.text.is_empty() && attempt.status == TurnStatus::Running { ui.spinner(); ui.label("正在等待回复…"); }
                    if !attempt.text.is_empty() { self.markdown(ui, &attempt.id, &attempt.text); }
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
                    ui.add_space(4.0);
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        if ui.add_enabled(!attempt.text.is_empty(), Button::new("复制")).clicked() { ui.ctx().copy_text(attempt.text.clone()); }
                        if retry {
                            let source_available = options.iter().any(|option| same_model(&option.model, &attempt.model));
                            if ui.add_enabled(unlocked && source_available, Button::new(if attempt.status == TurnStatus::Completed { "重新生成" } else { "重试" }))
                                .on_hover_text("使用这次回答的原模型重试").clicked() {
                                actions.push(ChatAction::Retry); self.follow = true; self.to_bottom = true;
                            }
                            ui.add_enabled_ui(unlocked && !options.is_empty(), |ui| {
                                let count = actions.len();
                                model_picker(ui, self.id.with(("retry", &attempt.id)), "换模型重答", &mut self.retry_search,
                                    options, Some(&attempt.model), true, actions);
                                if actions.len() != count { self.follow = true; self.to_bottom = true; }
                            });
                        }
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
                }] }],
            }],
        };
        let path = root.path().join("chat-history.json");
        lumakit_support::save_versioned(&path, 1, &history).unwrap();
        let session = ChatSession::open(&path, Arc::new(|| {})).unwrap();
        let options = vec![ModelOption { model, name: "模型".into(), provider: "来源A".into(), endpoint: "端点A".into() }];
        (root, session, options)
    }

    fn draw(ctx: &egui::Context, panel: &mut ChatPanel, session: &ChatSession,
        options: &[ModelOption], size: Vec2, scale: f32, events: Vec<egui::Event>) -> Vec<ChatAction> {
        let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)), events, ..Default::default() };
        input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap().native_pixels_per_point = Some(scale);
        let mut actions = Vec::new();
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| { actions = panel.show(ui, session, options); });
        });
        actions
    }

    #[test]
    fn actual_layout_keeps_footer_reachable_for_long_content() {
        let (_root, mut session, options) = fixture();
        session.report("保存或请求错误，详细说明\n".repeat(300));
        for (width, height, scale) in [(860.0, 560.0, 1.0), (1200.0, 800.0, 1.0), (860.0, 560.0, 2.0), (860.0, 220.0, 1.0), (480.0, 480.0, 1.0)] {
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
            vec![egui::Event::Ime(egui::ImeEvent::Preedit("拼".into())), enter(false)]);
        assert!(!actions.iter().any(|a| matches!(a, ChatAction::Send)));
        let actions = draw(&ctx, &mut a, &session, &options, Vec2::new(860.0, 560.0), 1.0,
            vec![egui::Event::Ime(egui::ImeEvent::Commit("拼".into())), enter(false)]);
        assert!(!actions.iter().any(|a| matches!(a, ChatAction::Send)));
        let actions = draw(&ctx, &mut a, &session, &options, Vec2::new(860.0, 560.0), 1.0, vec![enter(true)]);
        assert!(!actions.iter().any(|a| matches!(a, ChatAction::Send)));
        draw(&ctx, &mut a, &session, &options, Vec2::new(860.0, 560.0), 1.0, vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::CTRL }]);
        let actions = draw(&ctx, &mut a, &session, &options, Vec2::new(860.0, 560.0), 1.0, vec![enter(false)]);
        assert!(actions.iter().any(|a| matches!(a, ChatAction::Send)));
        for i in 0..CACHE_ENTRIES + 10 {
            a.frame += 1;
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| a.markdown(ui, &i.to_string(), "正文"));
            });
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
            let _ = ctx.run(egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(480.0, 480.0))), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    measured = ui.scope(|ui| { panel.markdown(ui, "paragraph", &text); }).response.rect;
                });
            });
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
            egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(0.0, 120.0), modifiers: egui::Modifiers::NONE },
        ]);
        assert!(!panel.follow);
        let offset = panel.last_offset;
        draw(&ctx, &mut panel, &session, &options, Vec2::new(860.0, 560.0), 1.0, vec![]);
        assert!(!panel.follow);
        assert!(panel.last_offset <= offset);
    }
}
