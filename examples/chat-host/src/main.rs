//! 独立消费同一聊天组件；模型连接由环境提供，不引用产品源码。
use eframe::egui;
use lumakit_ai::{chat::ChatSession, AiConnection, ModelRef, Protocol};
use lumakit_egui::ChatPanel;
use lumakit_ui::chat::{ChatAction, ModelOption};
use std::{path::PathBuf, sync::Arc};

struct Host { session: ChatSession, panel: ChatPanel, options: Vec<ModelOption>, base: String }
impl Host {
    fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        let base = std::env::var("LUMAKIT_AI_URL").map_err(|_| "请设置 LUMAKIT_AI_URL 为真实服务或回环地址".to_owned())?;
        lumakit_ai::catalog::validate_url(&base)?;
        let root = std::env::var_os("LUMAKIT_CHAT_DATA").map(PathBuf::from).ok_or("请设置独立 LUMAKIT_CHAT_DATA 目录")?;
        let ctx = cc.egui_ctx.clone();
        let mut fonts = egui::FontDefinitions::default();
        if let Ok(data) = std::fs::read("C:/Windows/Fonts/msyh.ttc") {
            fonts.font_data.insert("chinese".into(), egui::FontData::from_owned(data).into());
            fonts.families.entry(egui::FontFamily::Proportional).or_default().push("chinese".into());
        }
        ctx.set_fonts(fonts);
        let model = ModelRef { provider: "host".into(), endpoint: "main".into(), model: std::env::var("LUMAKIT_AI_MODEL").unwrap_or_else(|_| "local-model".into()), protocol: match std::env::var("LUMAKIT_AI_PROTOCOL").as_deref() { Ok("responses") => Protocol::OpenAiResponses, Ok("messages") => Protocol::AnthropicMessages, _ => Protocol::OpenAiChat } };
        let mut session = ChatSession::open(&root.join("chat-history.json"), Arc::new(move || ctx.request_repaint()))?;
        if session.history().model.is_none() { session.select_model(model.clone()); }
        Ok(Self { session, panel: ChatPanel::new("independent-host"), options: vec![ModelOption { name: model.model.clone(), provider: "独立宿主".into(), endpoint: "主线路".into(), model }], base })
    }
    fn resolve(&self, model: &ModelRef) -> Result<AiConnection, String> {
        if !self.options.iter().any(|o| o.model.model == model.model && o.model.endpoint == model.endpoint && o.model.protocol == model.protocol && o.model.provider == model.provider) { return Err("模型已失效".into()); }
        Ok(AiConnection { base_url: self.base.clone(), protocol: model.protocol, model: model.model.clone(), api_key: std::env::var("LUMAKIT_AI_KEY").ok().map(zeroize::Zeroizing::new) })
    }
}
impl eframe::App for Host {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.session.poll();
        if ctx.input(|i| i.viewport().close_requested()) && !self.session.can_close() { ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose); }
        egui::CentralPanel::default().show(ctx, |ui| {
            for action in self.panel.show(ui, &self.session, &self.options) {
                match action {
                    ChatAction::New => self.session.new_conversation(),
                    ChatAction::Select(id) => self.session.select_conversation(&id),
                    ChatAction::Delete(id) => self.session.delete_conversation(&id),
                    ChatAction::Draft(text) => self.session.set_draft(text),
                    ChatAction::SelectModel(model) => self.session.select_model(model),
                    ChatAction::Stop => self.session.cancel(),
                    ChatAction::Save => self.session.retry_save(),
                    ChatAction::OpenSettings => self.session.report("此验收宿主通过环境变量配置模型".into()),
                    ChatAction::Send | ChatAction::Retry => {
                        let retry = matches!(action, ChatAction::Retry);
                        match self.session.request_model(retry).and_then(|m| self.resolve(&m)) {
                            Ok(c) => if retry { self.session.retry(c); } else { self.session.send(c); },
                            Err(e) => self.session.report(e),
                        }
                    }
                    ChatAction::RetryWith(model) => match self.resolve(&model) {
                        Ok(c) => { self.session.select_model(model); self.session.retry_with(c); }
                        Err(e) => self.session.report(e),
                    }
                }
            }
        });
    }
}
fn main() -> eframe::Result {
    eframe::run_native("LumaKit Chat Host", eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([900.0,700.0]), ..Default::default() }, Box::new(|cc| Host::new(cc).map(|app| Box::new(app) as Box<dyn eframe::App>).map_err(|e| Box::new(std::io::Error::other(e)) as _)))
}
