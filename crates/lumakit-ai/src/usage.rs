//! Request accounting independent of conversation lifetime; input includes cached tokens.
use crate::{ModelRef, RoundUsage, chat::History};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashSet},
};

/// Known usage subtotals; missing observations are counted, never replaced with zero.
#[allow(missing_docs)]
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct UsageTotals {
    pub requests: u64,
    pub missing_usage: u64,
    pub missing_cost: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: Option<u64>,
    pub costs: BTreeMap<String, f64>,
    /// Amounts backed only by the user's request-time configured prices, excluding reference tariffs.
    #[serde(default)]
    pub configured_costs: BTreeMap<String, f64>,
    /// Requests covered by a valid user-configured price snapshot.
    #[serde(default)]
    pub configured_cost_requests: u64,
}
/// A named provider, model or UTC date subtotal.
#[allow(missing_docs)]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UsageGroup {
    pub label: String,
    pub totals: UsageTotals,
}
/// Request totals with missing-date and legacy coverage made explicit.
#[allow(missing_docs)]
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct UsageReport {
    pub total: UsageTotals,
    pub by_model: Vec<UsageGroup>,
    pub by_provider: Vec<UsageGroup>,
    pub by_day: Vec<UsageGroup>,
    pub undated_requests: u64,
    pub legacy_requests: u64,
}
/// Current prompt occupancy, not cumulative spending or a billable token estimate.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextUsage {
    pub tokens: u64,
    pub context_window: Option<u32>,
    pub anchored: bool,
}

pub(crate) fn normalized<'a>(
    round: &'a RoundUsage,
    model: &ModelRef,
    attempt_id: &str,
    index: usize,
) -> Cow<'a, RoundUsage> {
    if !round.request_id.is_empty() && round.model.is_some() {
        return Cow::Borrowed(round);
    }
    let mut round = round.clone();
    if round.request_id.is_empty() {
        // Historic copies have different attempt IDs: their shared origin cannot be recovered.
        round.request_id = format!("legacy:{attempt_id}:{index}");
        round.legacy = true;
        round.started_at_ms = None;
        round.duration_ms = None;
    }
    if round.model.is_none() {
        round.model = Some(model.clone());
    }
    Cow::Owned(round)
}

pub(crate) fn record(
    connection: &Connection,
    history: &History,
) -> Result<(), crate::history::HistoryError> {
    let mut insert = connection.prepare("INSERT INTO usage_requests(request_id,started_at_ms,legacy,round) VALUES(?1,?2,?3,?4) ON CONFLICT(request_id) DO NOTHING")?;
    for conversation in &history.conversations {
        for turn in &conversation.turns {
            for attempt in &turn.attempts {
                for (index, round) in attempt.rounds.iter().enumerate() {
                    let round = normalized(round, &attempt.model, &attempt.id, index);
                    let timestamp =
                        round
                            .started_at_ms
                            .map(i64::try_from)
                            .transpose()
                            .map_err(|_| {
                                crate::history::HistoryError::InvalidData(
                                    "请求时间超出 SQLite 范围",
                                )
                            })?;
                    insert.execute(params![
                        round.request_id,
                        timestamp,
                        round.legacy,
                        serde_json::to_string(round.as_ref())?
                    ])?;
                }
            }
        }
    }
    Ok(())
}

impl UsageTotals {
    fn add(&mut self, round: &RoundUsage) {
        self.requests = self.requests.saturating_add(1);
        if let Some(usage) = &round.usage {
            self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens);
            self.output_tokens = self.output_tokens.saturating_add(usage.output_tokens);
            self.cache_read_tokens = self
                .cache_read_tokens
                .saturating_add(usage.cache_read_tokens);
            self.cache_write_tokens = self
                .cache_write_tokens
                .saturating_add(usage.cache_write_tokens);
            if let Some(tokens) = usage.reasoning_tokens {
                self.reasoning_tokens =
                    Some(self.reasoning_tokens.unwrap_or(0).saturating_add(tokens));
            }
        } else {
            self.missing_usage = self.missing_usage.saturating_add(1);
        }
        if let Some((pricing, cost)) =
            round
                .pricing
                .as_ref()
                .zip(round.cost)
                .filter(|(pricing, cost)| {
                    !pricing.currency.trim().is_empty() && cost.is_finite() && *cost >= 0.0
                })
        {
            *self.costs.entry(pricing.currency.clone()).or_default() += cost;
            if round.cost_source == crate::model_config::CostSource::Estimate {
                *self
                    .configured_costs
                    .entry(pricing.currency.clone())
                    .or_default() += cost;
                self.configured_cost_requests = self.configured_cost_requests.saturating_add(1);
            }
        } else {
            self.missing_cost = self.missing_cost.saturating_add(1);
        }
    }
}

/// `None` conversation reads the persistent ledger; selected-conversation reports include all attempts.
/// Undated/legacy coverage counts describe the scope before the optional date filter; undated rows
/// are excluded from filtered totals, never assigned the import date.
pub(crate) fn report(
    connection: &Connection,
    history: &History,
    conversation_id: Option<&str>,
    since_ms: Option<u64>,
) -> Result<UsageReport, String> {
    let mut report = UsageReport::default();
    let mut models = BTreeMap::<String, UsageTotals>::new();
    let mut providers = BTreeMap::<String, UsageTotals>::new();
    let mut days = BTreeMap::<u64, UsageTotals>::new();
    let mut seen = HashSet::new();
    let mut add = |round: &RoundUsage| {
        if !seen.insert(round.request_id.clone()) {
            return;
        }
        if round.started_at_ms.is_none() {
            report.undated_requests += 1;
        }
        if round.legacy {
            report.legacy_requests += 1;
        }
        if since_ms.is_some_and(|since| {
            round
                .started_at_ms
                .is_none_or(|timestamp| timestamp < since)
        }) {
            return;
        }
        report.total.add(round);
        let provider = round
            .model
            .as_ref()
            .map(|model| model.provider.as_str())
            .unwrap_or("Unknown");
        let model = round
            .model
            .as_ref()
            .map(|model| format!("{} / {}", model.provider, model.model))
            .unwrap_or_else(|| "Unknown".into());
        providers.entry(provider.to_owned()).or_default().add(round);
        models.entry(model).or_default().add(round);
        if let Some(timestamp) = round.started_at_ms {
            days.entry(timestamp / 86_400_000).or_default().add(round);
        }
    };
    if let Some(id) = conversation_id {
        let conversation = history
            .conversations
            .iter()
            .find(|conversation| conversation.id == id)
            .ok_or("对话不存在")?;
        for turn in &conversation.turns {
            for attempt in &turn.attempts {
                for (index, round) in attempt.rounds.iter().enumerate() {
                    add(&normalized(round, &attempt.model, &attempt.id, index));
                }
            }
        }
    } else {
        let mut statement = connection
            .prepare("SELECT round FROM usage_requests ORDER BY rowid")
            .map_err(|error| error.to_string())?;
        let mut rows = statement.query([]).map_err(|error| error.to_string())?;
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            let value: String = row.get(0).map_err(|error| error.to_string())?;
            let round: RoundUsage =
                serde_json::from_str(&value).map_err(|error| error.to_string())?;
            add(&round);
        }
    }
    report.by_model = models
        .into_iter()
        .map(|(label, totals)| UsageGroup { label, totals })
        .collect();
    report.by_provider = providers
        .into_iter()
        .map(|(label, totals)| UsageGroup { label, totals })
        .collect();
    for (day, totals) in days {
        let seconds = i64::try_from(day)
            .ok()
            .and_then(|day| day.checked_mul(86_400))
            .ok_or("请求日期超出范围")?;
        let label: Option<String> = connection
            .query_row(
                "SELECT strftime('%Y-%m-%d',?1,'unixepoch')",
                [seconds],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        report.by_day.push(UsageGroup {
            label: label.ok_or("请求日期超出范围")?,
            totals,
        });
    }
    Ok(report)
}
