//! 独立消费同一聊天组件；模型连接由环境提供，不引用产品源码。
use eframe::egui;
use lumakit_ai::{chat::ChatSession, AiConnection, ModelRef, Protocol};
use lumakit_egui::ChatPanel;
use lumakit_ai::chat::{ChatAction, ModelOption};
use std::{path::PathBuf, sync::Arc};

struct Host { session: ChatSession, panel: ChatPanel, options: Vec<ModelOption>, image_options: Vec<ModelOption>, settings: lumakit_ai::model_config::ModelSettings, base: String, picker: Option<std::sync::mpsc::Receiver<Result<Option<PathBuf>, String>>> }
impl Host {
    fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        let (base, options, image_options, settings) = Self::configured_models()?;
        let root = std::env::var_os("LUMAKIT_CHAT_DATA").map(PathBuf::from).ok_or("请设置独立 LUMAKIT_CHAT_DATA 目录")?;
        let ctx = cc.egui_ctx.clone();
        let mut fonts = egui::FontDefinitions::default();
        lumakit_egui::install_icon_font(&mut fonts);
        if let Ok(data) = std::fs::read("C:/Windows/Fonts/msyh.ttc") {
            fonts.font_data.insert("chinese".into(), egui::FontData::from_owned(data).into());
            fonts.families.entry(egui::FontFamily::Proportional).or_default().push("chinese".into());
        }
        ctx.set_fonts(fonts);
        let mut session = ChatSession::open(&root.join("chat.db"), Arc::new(move || ctx.request_repaint()))?;
        if session.history().model.is_none() { session.select_model_pair(&options[0], None)?; }
        session.reconcile_model_catalog(&options)?;
        Ok(Self { session, panel: ChatPanel::new("independent-host"), options, image_options, settings, base, picker: None })
    }
    fn configured_models() -> Result<(String, Vec<ModelOption>, Vec<ModelOption>, lumakit_ai::model_config::ModelSettings), String> {
        let base = std::env::var("LUMAKIT_AI_URL").map_err(|_| "请设置 LUMAKIT_AI_URL 为真实服务或回环地址".to_owned())?;
        lumakit_ai::catalog::validate_url(&base)?;
        let model = ModelRef { provider: "host".into(), endpoint: "main".into(), model: std::env::var("LUMAKIT_AI_MODEL").unwrap_or_else(|_| "local-model".into()), protocol: match std::env::var("LUMAKIT_AI_PROTOCOL").as_deref() { Ok("responses") => Protocol::OpenAiResponses, Ok("messages") => Protocol::AnthropicMessages, _ => Protocol::OpenAiChat } };
        let settings: lumakit_ai::model_config::ModelSettings = std::env::var("LUMAKIT_AI_SETTINGS").ok().map(|json| serde_json::from_str(&json).map_err(|e| format!("LUMAKIT_AI_SETTINGS 无效：{e}"))).transpose()?.unwrap_or_default();
        let settings = lumakit_ai::model_reference::effective_settings(&model.model, model.protocol, &settings);
        settings.validate(model.protocol)?;
        // Environment credentials are not stored credentials; never imply a subscription or saved key.
        let image_options = std::env::var("LUMAKIT_AI_IMAGE_MODEL").ok().filter(|m| !m.trim().is_empty()).map(|name| vec![ModelOption {
            model: ModelRef { model: name.clone(), protocol: Protocol::OpenAiChat, ..model.clone() }, name, provider: "独立宿主".into(), endpoint: "主线路".into(),
            settings: lumakit_ai::model_config::ModelSettings { model_type: lumakit_ai::model_config::ModelType::Image, ..Default::default() }, has_key: false,
        }]).unwrap_or_default();
        let options = vec![ModelOption { name: settings.display_name(&model.model).to_owned(), provider: "独立宿主".into(), endpoint: "主线路".into(), model, settings: settings.clone(), has_key: false }];
        Ok((base, options, image_options, settings))
    }

    fn refresh_models(&mut self, force: bool) {
        match Self::configured_models() {
            Ok((base, options, image_options, settings)) => {
                self.base = base; self.options = options; self.image_options = image_options; self.settings = settings;
                if let Err(error) = self.session.reconcile_model_catalog(&self.options) { self.session.report(error); return; }
                self.session.report(if force { "已重新载入本地环境配置；独立宿主未接入 HTTP 目录发现，未请求网络。" } else { "已重新载入本地环境模型配置。" }.into());
            }
            Err(error) => self.session.report(error),
        }
    }

    fn select_model_pair(&mut self, model: ModelRef, effort: Option<lumakit_ai::model_config::ReasoningEffort>, default: bool) {
        let result = self.options.iter().find(|option| option.model == model).ok_or_else(|| "聊天模型已失效".to_owned()).and_then(|option| {
            if default { self.session.set_default_model_pair(option, effort) } else { self.session.select_model_pair(option, effort) }
        });
        if let Err(error) = result { self.session.report(error); }
    }
    fn resolve(&self, model: &ModelRef, image: bool) -> Result<AiConnection, String> {
        let options = if image { &self.image_options } else { &self.options };
        if !options.iter().any(|o| same_model(&o.model, model)) { return Err(if image { "生图模型已失效" } else { "聊天模型已失效" }.into()); }
        let settings = if image { lumakit_ai::model_config::ModelSettings { model_type: lumakit_ai::model_config::ModelType::Image, ..Default::default() } } else { self.settings.clone() };
        Ok(AiConnection { base_url: self.base.clone(), protocol: model.protocol, model: model.model.clone(), api_key: std::env::var("LUMAKIT_AI_KEY").ok().map(zeroize::Zeroizing::new), settings })
    }
    /// 聊天连接始终负责对话；生图连接仅作为本轮工具使用，重试沿用原轮快照。
    fn request(&mut self, retry: bool, override_model: Option<ModelRef>) {
        let override_selected = override_model.is_some();
        let prepared = (|| {
            let requested = match override_model { Some(model) => model, None => self.session.request_model(retry)? };
            let legacy_image = retry && !override_selected
                && !self.options.iter().any(|option| same_model(&option.model, &requested))
                && self.image_options.iter().any(|option| same_model(&option.model, &requested));
            let chat_model = if legacy_image {
                self.session.history().model.clone().ok_or("旧版生图重试需要先选择聊天模型。")?
            } else { requested };
            let chat = self.resolve(&chat_model, false)?;
            let painting = self.session.request_painting_model(retry)?.map(|model| self.resolve(&model, true)).transpose()?;
            Ok::<_, String>((chat_model, chat, painting, override_selected || legacy_image))
        })();
        match prepared {
            Ok((model, chat, painting, explicit_model)) => {
                if explicit_model { self.session.select_model(model); }
                match (retry, explicit_model, painting) {
                    (false, _, Some(painting)) => self.session.send_with_painting(chat, painting),
                    (false, _, None) => self.session.send(chat),
                    (true, true, Some(painting)) => self.session.retry_with_model_and_painting(chat, painting),
                    (true, false, Some(painting)) => self.session.retry_with_painting(chat, painting),
                    (true, true, None) => self.session.retry_with(chat),
                    (true, false, None) => self.session.retry(chat),
                }
            }
            Err(error) => self.session.report(error),
        }
    }
}
fn same_model(a: &ModelRef, b: &ModelRef) -> bool {
    a.provider == b.provider && a.endpoint == b.endpoint && a.model == b.model && a.protocol == b.protocol
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
                    ChatAction::New => self.session.new_conversation(), ChatAction::Select(id) => self.session.select_conversation(&id), ChatAction::Delete(id) => self.session.delete_conversation(&id), ChatAction::Draft(text) => self.session.set_draft(text), ChatAction::SelectModel(model) => self.select_model_pair(model, None, false), ChatAction::Stop => self.session.cancel(), ChatAction::Save => self.session.retry_save(),
                    ChatAction::SelectModelPair { model, effort } => self.select_model_pair(model, effort, false),
                    ChatAction::SetDefaultModelPair { model, effort } => self.select_model_pair(model, effort, true),
                    ChatAction::ToggleFavorite(model) => {
                        if self.options.iter().any(|option| option.model == model) { self.session.toggle_model_favorite(model); }
                        else { self.session.report("聊天模型已失效，无法收藏".into()); }
                    }
                    ChatAction::RefreshModels { force } => self.refresh_models(force),
                    ChatAction::SetComposer(settings) => self.session.set_composer(settings),
                    ChatAction::SetToolbar(tools) => self.session.set_toolbar(tools),
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
                    ChatAction::Send => self.request(false, None),
                    ChatAction::Retry => self.request(true, None),
                    ChatAction::RetryWith(model) => self.request(true, Some(model)),
                    ChatAction::TranslateAttempt { .. } | ChatAction::SubmitTranslation { .. } => self.session.report("独立验收宿主暂未接入翻译模型选择".into()),
                }
            }
        });
    }
}
fn main() -> eframe::Result {
    eframe::run_native("LumaKit Chat Host", eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([900.0,700.0]), ..Default::default() }, Box::new(|cc| Host::new(cc).map(|app| Box::new(app) as Box<dyn eframe::App>).map_err(|e| Box::new(std::io::Error::other(e)) as _)))
}
