//! Provider-anchored display estimates; admission still uses the conservative runtime budget.
use super::{ChatMessage, ChatSession, composer};
use crate::{
    ModelRef,
    model_config::{ContextAnchor, ModelSettings},
    usage::{ContextUsage, UsageReport},
};
use sha2::{Digest, Sha256};

impl ChatSession {
    /// Aggregates recorded requests; global totals survive history deletion.
    ///
    /// # Errors
    /// Returns database errors or a missing conversation instead of an empty report.
    pub fn usage_report(
        &self,
        conversation_id: Option<&str>,
        since_ms: Option<u64>,
    ) -> Result<UsageReport, String> {
        crate::usage::report(&self.connection, &self.history, conversation_id, since_ms)
    }

    /// Estimates the next prompt from its unchanged provider-reported prefix and local tail.
    /// A model, selected reply, attachment or history edit invalidates a mismatched anchor.
    /// This is not a tokenizer result and is never used as an admission guarantee.
    ///
    /// # Errors
    /// Returns attachment and history validation failures rather than reporting zero context.
    pub fn context_usage(&self, settings: &ModelSettings) -> Result<ContextUsage, String> {
        let model = self.history.model.as_ref().ok_or("请先选择聊天模型。")?;
        let settings =
            crate::model_reference::effective_settings(&model.model, model.protocol, settings);
        let Some(conversation) = self.selected() else {
            return Ok(ContextUsage {
                tokens: 0,
                context_window: settings.context_window,
                anchored: false,
            });
        };
        let attachments = self.draft_attachments();
        let empty = conversation.draft.trim().is_empty() && attachments.is_empty();
        let mut messages = composer::context(
            &conversation.turns,
            if empty {
                "x"
            } else {
                conversation.draft.trim()
            },
            attachments,
            &self.composer_data,
            settings.supports_vision,
            &self.media_root,
        )?;
        if empty {
            messages.pop();
        }
        let overhead = context_overhead(
            model,
            self.composer().web_search,
            self.composer().image_mode,
        );
        let mut prefix_len = 0;
        let mut newest = None;
        for turn in &conversation.turns {
            let selected = turn
                .selected_attempt
                .as_ref()
                .and_then(|id| turn.attempts.iter().find(|a| &a.id == id));
            let Some(attempt) = selected.or_else(|| turn.attempts.last()).filter(|a| {
                a.status == crate::history::TurnStatus::Completed
                    && a.kind == super::AttemptKind::Reply
                    && !a.text.trim().is_empty()
            }) else {
                continue;
            };
            prefix_len += 2;
            if attempt.model != *model
                || attempt.rounds.len() != 1
                || !attempt.tool_events.is_empty()
            {
                continue;
            }
            let round = &attempt.rounds[0];
            let Some((anchor, usage)) = round.context.as_ref().zip(round.usage.as_ref()) else {
                continue;
            };
            let request_len = prefix_len - 1;
            if request_len <= messages.len()
                && anchor.overhead == overhead
                && anchor.prefix == context_prefix(&messages[..request_len])
            {
                newest = Some((request_len, usage.input_tokens));
            }
        }
        let (tokens, anchored) = if let Some((tail, base)) = newest {
            (
                messages[tail..].iter().fold(base, |sum, message| {
                    sum.saturating_add(message_tokens(message))
                }),
                true,
            )
        } else {
            let tools = super::generation::painting_tools(self.composer().image_mode);
            let tool_tokens = tools.iter().fold(0u64, |sum, tool| {
                sum.saturating_add(
                    (tool.name.chars().count() as u64
                        + tool.description.chars().count() as u64
                        + tool.parameters.to_string().chars().count() as u64)
                        .saturating_mul(4)
                        .saturating_add(32),
                )
            });
            (
                messages.iter().fold(
                    tool_tokens.saturating_add(if self.composer().web_search { 256 } else { 0 }),
                    |sum, message| sum.saturating_add(message_tokens(message)),
                ),
                false,
            )
        };
        Ok(ContextUsage {
            tokens,
            context_window: settings.context_window,
            anchored,
        })
    }
}

pub(super) fn context_anchor(
    messages: &[ChatMessage],
    model: &ModelRef,
    web_search: bool,
) -> ContextAnchor {
    ContextAnchor {
        prefix: context_prefix(messages),
        overhead: context_overhead(model, web_search, false),
    }
}

fn context_overhead(model: &ModelRef, web_search: bool, image_mode: bool) -> String {
    // Model identity includes the saved endpoint; a relay switch cannot reuse another route's anchor.
    format!("{:?}:{web_search}:{image_mode}", model)
}

fn context_prefix(messages: &[ChatMessage]) -> String {
    let mut hash = Sha256::new();
    fn part(hash: &mut Sha256, text: &str) {
        hash.update((text.len() as u64).to_le_bytes());
        hash.update(text.as_bytes());
    }
    for message in messages {
        match message {
            ChatMessage::User(text) => {
                hash.update([0]);
                part(&mut hash, text);
            }
            ChatMessage::Assistant(text) => {
                hash.update([1]);
                part(&mut hash, text);
            }
            ChatMessage::UserImages { text, images } => {
                hash.update([2]);
                part(&mut hash, text);
                hash.update((images.len() as u64).to_le_bytes());
                for image in images {
                    part(&mut hash, &image.mime);
                    part(&mut hash, &image.data);
                }
            }
        }
    }
    format!("{:x}", hash.finalize())
}

fn message_tokens(message: &ChatMessage) -> u64 {
    let (text, images) = match message {
        ChatMessage::User(text) | ChatMessage::Assistant(text) => (text, 0),
        ChatMessage::UserImages { text, images } => (text, images.len() as u64),
    };
    // ponytail: same conservative Unicode/image bound as admission, not model tokenization.
    (text.chars().count() as u64)
        .saturating_mul(4)
        .saturating_add(32)
        .saturating_add(images.saturating_mul(4096))
}
