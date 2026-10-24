#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{self, Align, Color32, Layout, RichText, Vec2};
use lumakit_ai::{AiConnection, AiError, AiHost, Approval, ModelRef, RunRequest, ToolCall, ToolFuture, ToolOutput, ToolSpec};
use lumakit_ports::{LogLevel, LogPort};
use lumakit_ui::{UiBackend, UiSession};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

struct DemoLogs {
    level: Mutex<LogLevel>,
}

impl DemoLogs {
    fn new() -> Self {
        Self {
            level: Mutex::new(LogLevel::Info),
        }
    }
}

impl LogPort for DemoLogs {
    fn level(&self) -> LogLevel {
        self.level.lock().map(|level| *level).unwrap_or(LogLevel::Info)
    }

    fn set_level(&self, level: LogLevel) -> Result<LogLevel, lumakit_ports::PortError> {
        *self
            .level
            .lock()
            .map_err(|_| lumakit_ports::PortError::Unavailable("日志状态不可用".into()))? = level;
        Ok(level)
    }

    fn tail(&self, _limit: usize) -> Result<Vec<String>, lumakit_ports::PortError> {
        Ok(vec!["lumakit-desktop started".into()])
    }
}

struct DemoAiHost;

impl AiHost for DemoAiHost {
    fn select_model(&self, request: &RunRequest) -> Result<ModelRef, AiError> {
        request.model.clone().ok_or(AiError::ModelNotConfigured)
    }

    fn system_prompt(&self, _request: &RunRequest) -> Result<Option<String>, AiError> {
        Ok(Some("只使用明确授权的桌面工具。".into()))
    }

    fn tools(&self, _request: &RunRequest) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "desktop.read_status".into(),
            description: "读取宿主应用状态".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            read_only: true,
            requires_approval: false,
        }]
    }

    fn authorize(&self, _call: &ToolCall, default: Approval) -> Approval {
        default
    }

    fn connection(&self, model: &ModelRef) -> Result<AiConnection, AiError> {
        let base_url = std::env::var("LUMAKIT_AI_URL").unwrap_or_else(|_| "http://127.0.0.1".into());
        Ok(AiConnection {
            base_url,
            protocol: model.protocol,
            api_key: std::env::var("LUMAKIT_AI_KEY").ok().map(Zeroizing::new),
            model: model.model.clone(),
            settings: Default::default(),
        })
    }

    fn call_tool(&self, _call: ToolCall, _cancel: CancellationToken) -> ToolFuture {
        Box::pin(async {
            Ok(ToolOutput {
                text: "宿主状态正常".into(),
                is_error: false,
            })
        })
    }
}

struct DesktopBackend {
    logs: Arc<DemoLogs>,
    ai: Arc<DemoAiHost>,
}

impl UiBackend for DesktopBackend {
    fn logs(&self) -> Arc<dyn LogPort> {
        self.logs.clone()
    }

    fn ai_host(&self) -> Arc<dyn AiHost> {
        self.ai.clone()
    }

    fn select_model(&self, request: &RunRequest) -> Result<ModelRef, String> {
        self.ai.select_model(request).map_err(|error| error.to_string())
    }

    fn tools(&self, request: &RunRequest) -> Vec<ToolSpec> {
        self.ai.tools(request)
    }
}

struct DesktopApp {
    session: UiSession<DesktopBackend>,
    ai: lumakit_ai::AiRuntime<DemoAiHost>,
    active: usize,
    dark: bool,
    notice: String,
}

impl DesktopApp {
    fn new() -> Result<Self, AiError> {
        let backend = Arc::new(DesktopBackend {
            logs: Arc::new(DemoLogs::new()),
            ai: Arc::new(DemoAiHost),
        });
        let ai = backend.ai.clone();
        Ok(Self {
            session: UiSession::new(backend),
            ai: lumakit_ai::AiRuntime::new(ai)?,
            active: 0,
            dark: false,
            notice: "选择一个应用开始".into(),
        })
    }

    fn request(&self) -> RunRequest {
        RunRequest {
            input: "读取宿主状态".into(),
            model: Some(ModelRef {
                provider: "demo".into(),
                endpoint: "loopback".into(),
                model: "demo".into(),
                protocol: lumakit_ai::Protocol::OpenAiChat,
            }),
            purpose: "launcher".into(),
        }
    }

    fn launcher(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(44.0);
            ui.label(RichText::new("应用").size(18.0).color(Color32::from_rgb(70, 79, 92)));
            ui.add_space(24.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(18.0, 18.0);
                for (index, (icon, name, desc)) in [
                    ("▢", "对话", "AI 对话入口"),
                    ("✣", "工作", "任务与工具"),
                    ("✎", "笔记", "记录想法"),
                    ("▤", "知识库", "检索资料"),
                    ("文", "翻译", "文本处理"),
                ]
                .iter()
                .enumerate()
                {
                    let selected = self.active == index + 1;
                    let button = egui::Button::new(
                        RichText::new(format!("{icon}\n{name}"))
                            .size(18.0)
                            .color(if selected { Color32::WHITE } else { Color32::from_rgb(35, 42, 52) }),
                    )
                    .min_size(Vec2::new(116.0, 92.0))
                    .fill(if selected {
                        Color32::from_rgb(68, 108, 144)
                    } else {
                        Color32::from_rgb(248, 249, 251)
                    });
                    if ui.add(button).clicked() {
                        self.active = index + 1;
                        self.notice = format!("已打开{name}：{desc}");
                    }
                }
            });
            ui.add_space(28.0);
            ui.label(RichText::new(&self.notice).color(Color32::from_rgb(112, 121, 134)));
        });
    }

    fn module(&mut self, ui: &mut egui::Ui) {
        let names = ["启动台", "对话", "工作", "笔记", "知识库", "翻译"];
        let name = names[self.active];
        ui.vertical_centered(|ui| {
            ui.add_space(42.0);
            ui.heading(name);
            ui.label(RichText::new("这是 LumaKit 宿主 App 的功能页面入口。").color(Color32::from_rgb(112, 121, 134)));
            ui.add_space(28.0);
            if self.active == 1 {
                let request = self.request();
                let tools = self.ai.available_tools(&request);
                let status = self.session.log_status();
                ui.group(|ui| {
                    ui.set_min_width(430.0);
                    ui.label(format!("AI Host：已接入"));
                    ui.label(format!("可用工具：{} 个", tools.len()));
                    ui.label(format!("日志级别：{:?}", status.level));
                    if ui.button("检查宿主工具").clicked() {
                        self.notice = format!("AI runtime 已发现 {} 个只读工具", tools.len());
                    }
                });
            } else if ui.button("返回启动台").clicked() {
                self.active = 0;
            }
        });
    }
}

impl eframe::App for DesktopApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.dark {
            ui.ctx().set_visuals(egui::Visuals::dark());
        } else {
            ui.ctx().set_visuals(egui::Visuals::light());
        }

        egui::Panel::top("topbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("LumaKit");
                ui.separator();
                ui.label("启动台");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button(if self.dark { "浅色" } else { "深色" }).clicked() {
                        self.dark = !self.dark;
                    }
                });
            });
        });
        egui::Panel::left("sidebar")
            .resizable(false)
            .default_size(64.0)
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    for (index, icon) in ["⌂", "▢", "✣", "✎", "▤", "文"].iter().enumerate() {
                        if ui
                            .add_sized([40.0, 42.0], egui::Button::new(RichText::new(*icon).size(20.0)))
                            .clicked()
                        {
                            self.active = index;
                        }
                    }
                });
            });
        egui::CentralPanel::default().show(ui, |ui| {
            if self.active == 0 {
                self.launcher(ui);
            } else {
                self.module(ui);
            }
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LumaKit · 启动台")
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([860.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "lumakit-desktop",
        options,
        Box::new(|_cc| Ok(Box::new(DesktopApp::new().expect("LumaKit host initialization failed")))),
    )
}
