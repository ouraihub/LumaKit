//! 独立消费同一聊天组件；模型连接由环境提供，不引用产品源码。
use eframe::egui;
use lumakit_ai::{chat::ChatSession, AiConnection, ModelRef, Protocol};
use lumakit_egui::ChatPanel;
use lumakit_ui::chat::{ChatAction, ModelOption};
use std::{path::PathBuf, sync::Arc};

struct Host { session: ChatSession, panel: ChatPanel, options: Vec<ModelOption>, image_options: Vec<ModelOption>, settings: lumakit_ai::model_config::ModelSettings, base: String, picker: Option<std::sync::mpsc::Receiver<Result<Option<PathBuf>, String>>> }
impl Host {
    fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        let base = std::env::var("LUMAKIT_AI_URL").map_err(|_| "请设置 LUMAKIT_AI_URL 为真实服务或回环地址".to_owned())?;
        lumakit_ai::catalog::validate_url(&base)?;
        let root = std::env::var_os("LUMAKIT_CHAT_DATA").map(PathBuf::from).ok_or("请设置独立 LUMAKIT_CHAT_DATA 目录")?;
        let ctx = cc.egui_ctx.clone();
        let mut fonts = egui::FontDefinitions::default();
        lumakit_egui::install_icon_font(&mut fonts);
        if let Ok(data) = std::fs::read("C:/Windows/Fonts/msyh.ttc") {
            fonts.font_data.insert("chinese".into(), egui::FontData::from_owned(data).into());
            fonts.families.entry(egui::FontFamily::Proportional).or_default().push("chinese".into());
        }
        ctx.set_fonts(fonts);
        let model = ModelRef { provider: "host".into(), endpoint: "main".into(), model: std::env::var("LUMAKIT_AI_MODEL").unwrap_or_else(|_| "local-model".into()), protocol: match std::env::var("LUMAKIT_AI_PROTOCOL").as_deref() { Ok("responses") => Protocol::OpenAiResponses, Ok("messages") => Protocol::AnthropicMessages, _ => Protocol::OpenAiChat } };
        let mut session = ChatSession::open(&root.join("chat.db"), Arc::new(move || ctx.request_repaint()))?;
        if session.history().model.is_none() { session.select_model(model.clone()); }
        let settings: lumakit_ai::model_config::ModelSettings = std::env::var("LUMAKIT_AI_SETTINGS").ok().map(|json| serde_json::from_str(&json).map_err(|e| format!("LUMAKIT_AI_SETTINGS 无效：{e}"))).transpose()?.unwrap_or_default();
        settings.validate(model.protocol)?;
        let image_options = std::env::var("LUMAKIT_AI_IMAGE_MODEL").ok().filter(|m| !m.trim().is_empty()).map(|name| vec![ModelOption { model: ModelRef { model: name.clone(), protocol: Protocol::OpenAiChat, ..model.clone() }, name, provider: "独立宿主".into(), endpoint: "主线路".into() }]).unwrap_or_default();
        Ok(Self { session, panel: ChatPanel::new("independent-host"), options: vec![ModelOption { name: model.model.clone(), provider: "独立宿主".into(), endpoint: "主线路".into(), model }], image_options, settings, base, picker: None })
    }
    fn resolve(&self, model: &ModelRef) -> Result<AiConnection, String> {
        let image = self.image_options.iter().any(|o| o.model.model == model.model && o.model.endpoint == model.endpoint && o.model.protocol == model.protocol && o.model.provider == model.provider);
        if !image && !self.options.iter().any(|o| o.model.model == model.model && o.model.endpoint == model.endpoint && o.model.protocol == model.protocol && o.model.provider == model.provider) { return Err("模型已失效".into()); }
        let settings = if image { lumakit_ai::model_config::ModelSettings { model_type: lumakit_ai::model_config::ModelType::Image, ..Default::default() } } else { self.settings.clone() };
        Ok(AiConnection { base_url: self.base.clone(), protocol: model.protocol, model: model.model.clone(), api_key: std::env::var("LUMAKIT_AI_KEY").ok().map(zeroize::Zeroizing::new), settings })
    }
}
impl eframe::App for Host {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.session.poll();
        if let Some(result) = self.picker.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err("附件选择器已意外退出".into())),
        }) {
            self.picker = None;
            match result {
                Ok(Some(path)) => if let Err(error) = self.session.add_attachment(&path) { self.session.report(error); },
                Ok(None) => {},
                Err(error) => self.session.report(error),
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.session.can_close() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            for action in self.panel.show(ui, &self.session, &self.options, &self.image_options, Some(&self.settings)) {
                match action {
                    ChatAction::New => self.session.new_conversation(), ChatAction::Select(id) => self.session.select_conversation(&id), ChatAction::Delete(id) => self.session.delete_conversation(&id), ChatAction::Draft(text) => self.session.set_draft(text), ChatAction::SelectModel(model) => self.session.select_model(model), ChatAction::Stop => self.session.cancel(), ChatAction::Save => self.session.retry_save(),
                    ChatAction::SetComposer(settings) => self.session.set_composer(settings),
                    ChatAction::RemoveAttachment(id) => self.session.remove_attachment(&id),
                    ChatAction::UploadAttachment => if self.picker.is_none() {
                        let (tx, rx) = std::sync::mpsc::channel();
                        self.picker = Some(rx);
                        let ctx = ui.ctx().clone();
                        std::thread::spawn(move || {
                            let result = lumakit_egui::chat::pick_attachment();
                            let _ = tx.send(result);
                            ctx.request_repaint();
                        });
                    },
                    ChatAction::OpenMedia(id) => if let Err(error) = self.session.media_path(&id).and_then(|path| lumakit_egui::chat::open_media(&path)) { self.session.report(error); }, 
                    ChatAction::SaveNote { .. } | ChatAction::InsertMarkdown { .. } | ChatAction::ExportMarkdown { .. } => self.session.report("此验收宿主未接入笔记宿主动作".into()), ChatAction::OpenSettings => self.session.report("此验收宿主通过环境变量配置模型".into()), ChatAction::EditAttempt { .. } => {}, ChatAction::SubmitEdit { conversation_id, turn_id, attempt_id, expected_text, next_text } => { if let Err(error) = self.session.edit_attempt(&conversation_id, &turn_id, &attempt_id, &expected_text, &next_text) { self.session.report(error); } }, ChatAction::DeleteAttempt { conversation_id, turn_id, attempt_id } => { if let Err(error) = self.session.delete_attempt(&conversation_id, &turn_id, &attempt_id) { self.session.report(error); } }, ChatAction::SelectAttempt { conversation_id, turn_id, attempt_id } => { if let Err(error) = self.session.select_attempt(&conversation_id, &turn_id, &attempt_id) { self.session.report(error); } }, ChatAction::ForkAttempt { conversation_id, turn_id, attempt_id } => { if let Err(error) = self.session.fork_attempt(&conversation_id, &turn_id, &attempt_id) { self.session.report(error); } }, ChatAction::CopyConversation { conversation_id } => { if let Err(error) = self.session.copy_conversation(&conversation_id) { self.session.report(error); } },
                    ChatAction::Send | ChatAction::Retry => { let retry = matches!(action, ChatAction::Retry); match self.session.request_model(retry).and_then(|m| self.resolve(&m)) { Ok(c) => if retry { self.session.retry(c); } else { self.session.send(c); }, Err(e) => self.session.report(e) } },
                    ChatAction::RetryWith(model) => match self.resolve(&model) { Ok(c) => { self.session.select_model(model); self.session.retry_with(c); }, Err(e) => self.session.report(e) }, ChatAction::TranslateAttempt { .. } | ChatAction::SubmitTranslation { .. } => self.session.report("独立验收宿主暂未接入翻译模型选择".into()),
                }
            }
        });
    }
}
fn main() -> eframe::Result {
    eframe::run_native("LumaKit Chat Host", eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([900.0,700.0]), ..Default::default() }, Box::new(|cc| Host::new(cc).map(|app| Box::new(app) as Box<dyn eframe::App>).map_err(|e| Box::new(std::io::Error::other(e)) as _)))
}
