//! 按需打开的上下文、对话与账本统计；全局查询仅在展开或切换时间范围时执行。
use egui::{RichText, ScrollArea, Ui};
use lumakit_ai::{
    RoundUsage,
    chat::ChatSession,
    model_config::{CostSource, ModelSettings},
    usage::{UsageGroup, UsageReport, UsageTotals},
};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Default)]
pub(super) struct UsageMenu {
    visible: bool,
    days: u64,
    conversation: Option<Result<UsageReport, String>>,
    global: Option<Result<UsageReport, String>>,
}

impl UsageMenu {
    pub(super) fn close(&mut self) {
        self.visible = false;
    }
    pub(super) fn show(
        &mut self,
        ui: &mut Ui,
        session: &ChatSession,
        settings: Option<&ModelSettings>,
    ) {
        let response = egui::containers::menu::MenuButton::new("用量")
            .config(egui::containers::menu::MenuConfig::new()
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside))
            .ui(ui, |ui| {
                if !self.visible {
                    self.conversation = session.selected().map(|c| session.usage_report(Some(&c.id), None));
                    self.global = None;
                }
                ui.set_width((ui.ctx().content_rect().width() - 48.0).clamp(180.0, 340.0));
                ScrollArea::vertical().id_salt("usage-scroll")
                    .max_height((ui.ctx().content_rect().height() - 80.0).max(80.0))
                    .show(ui, |ui| {
                        ui.strong("上下文占用");
                        let defaults = ModelSettings::default();
                        match session.context_usage(settings.unwrap_or(&defaults)) {
                            Ok(context) => {
                                if let Some(window) = context.context_window.filter(|&n| n > 0) {
                                    let ratio = context.tokens as f64 / f64::from(window);
                                    ui.add(egui::ProgressBar::new(ratio.clamp(0.0, 1.0) as f32)
                                        .text(format!("{} / {} token · {:.1}%", context.tokens, window, ratio * 100.0)));
                                } else {
                                    ui.label(format!("{} token / 上下文窗口未知", context.tokens));
                                }
                                ui.label(RichText::new(if context.anchored {
                                    "估算 · 以上游用量为锚点"
                                } else { "估算 · 本地上下文估算" }).weak().small());
                            }
                            Err(error) => { ui.colored_label(ui.visuals().error_fg_color, format!("上下文未知：{error}")); }
                        }
                        if let Some(model) = session.history().model.as_ref() { ui.label(&model.model); }
                        ui.separator();
                        ui.strong("当前对话累计");
                        match self.conversation.as_ref() {
                            Some(report) => show_report(ui, report),
                            None => { ui.label("尚未选择对话"); }
                        }
                        egui::CollapsingHeader::new("上一回答详情").id_salt("latest-usage")
                            .default_open(false).show(ui, |ui| {
                                if let Some(attempt) = session.selected().and_then(|c| c.turns.last())
                                    .and_then(|t| t.attempts.last()) {
                                    show_rounds(ui, &attempt.rounds);
                                } else { ui.label("暂无回答"); }
                            });
                        ui.separator();
                        egui::CollapsingHeader::new("全部使用统计").id_salt("global-usage")
                            .default_open(false).show(ui, |ui| {
                                let previous = self.days;
                                ui.horizontal_wrapped(|ui| {
                                    ui.selectable_value(&mut self.days, 0, "全部");
                                    ui.selectable_value(&mut self.days, 7, "近 7 天");
                                    ui.selectable_value(&mut self.days, 30, "近 30 天");
                                });
                                if self.global.is_none() || previous != self.days {
                                    let since = if self.days == 0 { Ok(None) } else {
                                        SystemTime::now().duration_since(UNIX_EPOCH)
                                            .map(|now| Some((now.as_millis().min(u128::from(u64::MAX)) as u64)
                                                .saturating_sub(self.days * 86_400_000)))
                                            .map_err(|error| format!("无法确定统计时间：{error}"))
                                    };
                                    self.global = Some(since.and_then(|since| session.usage_report(None, since)));
                                }
                                if let Some(report) = &self.global {
                                    show_report(ui, report);
                                    if let Ok(report) = report {
                                        show_groups(ui, "按模型", &report.by_model);
                                        show_groups(ui, "按服务商", &report.by_provider);
                                        show_groups(ui, "按日期（UTC）", &report.by_day);
                                    }
                                }
                            });
                        ui.label(RichText::new("输入已含缓存；推理属于输出子集，不重复相加。费用仅为 token 估算，不是服务商账单，不含搜索、生图等额外收费；不同币种不换算。").weak().small());
                    });
            });
        self.visible = response.1.is_some();
        response
            .0
            .on_hover_text("上下文估算、当前对话和全部使用统计")
            .accessible_name("用量详情");
    }
}

fn show_report(ui: &mut Ui, report: &Result<UsageReport, String>) {
    match report {
        Ok(report) => {
            show_totals(ui, &report.total);
            ui.label(
                RichText::new(format!(
                    "无日期请求 {} · 旧记录 {}",
                    report.undated_requests, report.legacy_requests
                ))
                .weak()
                .small(),
            );
        }
        Err(error) => {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("用量读取失败：{error}"),
            );
        }
    }
}

fn show_totals(ui: &mut Ui, totals: &UsageTotals) {
    ui.label(format!(
        "请求 {} · 缺失用量 {} · 无可用单价 {}",
        totals.requests, totals.missing_usage, totals.missing_cost
    ));
    if totals.requests > 0 && totals.missing_usage == totals.requests {
        ui.label("Token 用量未知");
    } else {
        ui.label(format!(
            "已知 token：输入（含缓存）{} · 输出 {} · 合计 {}",
            totals.input_tokens,
            totals.output_tokens,
            totals.input_tokens.saturating_add(totals.output_tokens)
        ));
        ui.label(format!(
            "输入子项：缓存读取 {} · 缓存写入 {}",
            totals.cache_read_tokens, totals.cache_write_tokens
        ));
        ui.label(format!(
            "输出子项：已报告推理 {}",
            totals
                .reasoning_tokens
                .map_or_else(|| "未知".into(), |n| n.to_string())
        ));
    }
    if totals.configured_costs.is_empty() {
        ui.label("未配置完整用户单价，显示 token 消耗，不套用目录参考价");
    } else {
        for (currency, cost) in &totals.configured_costs {
            ui.label(format!("用户单价估算：{currency} {cost}"));
        }
    }
}

fn show_groups(ui: &mut Ui, title: &str, groups: &[UsageGroup]) {
    egui::CollapsingHeader::new(title)
        .default_open(false)
        .show(ui, |ui| {
            if groups.is_empty() {
                ui.label("无记录");
            }
            for group in groups {
                ui.push_id(&group.label, |ui| {
                    egui::CollapsingHeader::new(&group.label)
                        .default_open(false)
                        .show(ui, |ui| show_totals(ui, &group.totals));
                });
            }
        });
}

pub(super) fn show_rounds(ui: &mut Ui, rounds: &[RoundUsage]) {
    if rounds.is_empty() {
        ui.label("用量未提供 · 未配置用户单价");
    }
    for (index, round) in rounds.iter().enumerate() {
        ui.label(
            RichText::new(format!("第 {} 轮 · {}", index + 1, round_label(round)))
                .weak()
                .small(),
        );
    }
}

pub(super) fn round_label(round: &RoundUsage) -> String {
    let Some(usage) = &round.usage else {
        return "用量未提供 · 未配置用户单价".into();
    };
    let tokens = format!(
        "输入（含缓存）{} · 输出 {} · 缓存读取 {} · 缓存写入 {} · 推理（输出子集）{} token",
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_read_tokens,
        usage.cache_write_tokens,
        usage
            .reasoning_tokens
            .map_or_else(|| "未知".into(), |n| n.to_string())
    );
    match (&round.pricing, round.cost, round.cost_source) {
        (Some(pricing), Some(cost), CostSource::Estimate)
            if cost.is_finite() && cost >= 0.0 && !pricing.currency.is_empty() =>
        {
            format!(
                "{tokens} · 用户单价 token 估算：{} {cost}",
                pricing.currency
            )
        }
        _ => format!("{tokens} · 未配置用户单价，不估算金额"),
    }
}
