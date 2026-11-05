//! Adapts chat model identities and persisted preferences to the shared controlled picker.
use super::*;
use lumakit_model_picker::{Effort, Model, Picker, PickerData, PickerEvent, Selection};

pub(super) struct PickerState {
    pub view: Entity<Picker>,
    models: HashMap<String, ModelRef>,
    refreshing: bool,
    notice: String,
}

impl PickerState {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ChatPanel>) -> (Self, Subscription) {
        let view = cx.new(|cx| Picker::new(window, cx));
        let subscription = cx.subscribe(&view, |this, _, event, cx| {
            if this.unlocked {
                if let Some(action) = this.picker.action(event) {
                    this.emit(action, cx);
                }
            }
        });
        (
            Self {
                view,
                models: HashMap::new(),
                refreshing: false,
                notice: String::new(),
            },
            subscription,
        )
    }

    fn action(&self, event: &PickerEvent) -> Option<ChatAction> {
        match event {
            PickerEvent::Select(selection) | PickerEvent::SetDefault(selection) => {
                let model = self.models.get(&selection.model)?.clone();
                let effort = match selection.effort.as_deref() {
                    None => None,
                    Some(id) => Some(effort_from_id(id)?),
                };
                Some(if matches!(event, PickerEvent::Select(_)) {
                    ChatAction::SelectModelPair { model, effort }
                } else {
                    ChatAction::SetDefaultModelPair { model, effort }
                })
            }
            PickerEvent::Refresh { force } => Some(ChatAction::RefreshModels { force: *force }),
        }
    }
}

impl ChatPanel {
    /// Reports the host's actual catalog refresh, including failures.
    pub fn set_model_refresh_state(&mut self, refreshing: bool, notice: String) {
        self.picker.refreshing = refreshing;
        self.picker.notice = notice;
    }

    pub(super) fn model_pill(
        &mut self,
        session: &ChatSession,
        options: &[ModelOption],
        cx: &mut Context<Self>,
    ) -> Entity<Picker> {
        let prefs = session.model_picker_preferences();
        self.picker.models.clear();
        let models = options
            .iter()
            .map(|option| {
                let id = model_key(&option.model);
                self.picker.models.insert(id.clone(), option.model.clone());
                Model {
                    id,
                    provider_id: option.model.provider.clone(),
                    provider: option.provider.clone(),
                    name: option.name.clone(),
                    detail: model_detail(option),
                    has_key: option.has_key,
                    efforts: effort_options(option),
                }
            })
            .collect();
        let data = PickerData {
            models,
            selected: session
                .history()
                .model
                .as_ref()
                .map(|model| selection(model, session.composer().reasoning)),
            default: prefs
                .default_pair
                .as_ref()
                .map(|pair| selection(&pair.model, pair.effort)),
            enabled: self.unlocked,
            refreshing: self.picker.refreshing,
            notice: self.picker.notice.clone(),
        };
        self.picker
            .view
            .update(cx, |picker, cx| picker.set_data(data, cx));
        self.picker.view.clone()
    }
}

// Debug tuple escaping preserves all four identity fields without delimiter collisions.
fn model_key(model: &ModelRef) -> String {
    format!(
        "{:?}",
        (
            &model.provider,
            &model.endpoint,
            &model.model,
            model.protocol
        )
    )
}

fn selection(model: &ModelRef, effort: Option<ReasoningEffort>) -> Selection {
    Selection {
        model: model_key(model),
        effort: effort.map(|level| effort_id(level).to_owned()),
    }
}

fn effort_id(level: ReasoningEffort) -> &'static str {
    use ReasoningEffort::*;
    match level {
        None => "off",
        Auto => "auto",
        Minimal => "minimal",
        Low => "low",
        Medium => "medium",
        High => "high",
        Xhigh => "xhigh",
        Max => "max",
    }
}

fn effort_from_id(id: &str) -> Option<ReasoningEffort> {
    Some(match id {
        "off" => ReasoningEffort::None,
        "auto" => ReasoningEffort::Auto,
        "minimal" => ReasoningEffort::Minimal,
        "low" => ReasoningEffort::Low,
        "medium" => ReasoningEffort::Medium,
        "high" => ReasoningEffort::High,
        "xhigh" => ReasoningEffort::Xhigh,
        "max" => ReasoningEffort::Max,
        _ => return None,
    })
}

fn effort_options(option: &ModelOption) -> Vec<Effort> {
    use ReasoningEffort::*;
    let model = option.settings.effective_model();
    [None, Minimal, Low, Medium, High, Xhigh, Max, Auto]
        .into_iter()
        .filter(|level| model.supports_effort(*level))
        .map(|level| Effort {
            id: effort_id(level).to_owned(),
            label: effort_label(level).to_owned(),
            hint: effort_hint(level).to_owned(),
        })
        .collect()
}

fn effort_label(level: ReasoningEffort) -> &'static str {
    use ReasoningEffort::*;
    match level {
        None => "关闭",
        Auto => "自动",
        Minimal => "最低",
        Low => "低",
        Medium => "中",
        High => "高",
        Xhigh => "极高",
        Max => "最高",
    }
}

fn effort_hint(level: ReasoningEffort) -> &'static str {
    use ReasoningEffort::*;
    match level {
        None => "无推理 — 最快·最省",
        Auto => "由服务商自动决定推理深度",
        Minimal => "最简推理",
        Low => "轻量推理 — 最快",
        Medium => "均衡推理",
        High => "深度推理",
        Xhigh => "超深度推理",
        Max => "最高推理",
    }
}

fn model_detail(option: &ModelOption) -> String {
    let mut parts = vec![option.endpoint.clone()];
    if option.name != option.model.model {
        parts.push(option.model.model.clone());
    }
    if let Some(price) = &option.settings.pricing {
        if let Some(tier) = price.tiers.first() {
            parts.push(format!(
                "{} {} / {} 每百万 token 起",
                price.currency, tier.input, tier.output
            ));
        }
    }
    if let Some(context) = option.settings.context_window {
        parts.push(if context >= 1_000_000 {
            format!("{:.1}M 上下文", context as f64 / 1_000_000.)
        } else if context >= 1000 {
            format!("{}K 上下文", context / 1000)
        } else {
            format!("{context} 上下文")
        });
    }
    parts.join(" · ")
}
