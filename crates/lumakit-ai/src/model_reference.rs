//! Offline public model metadata, projected from the MIT-licensed OMP catalog.
//!
//! `model_reference.json` embeds the upstream revision, source SHA-256, projection
//! rules and full license notice. Canonical vendor rows take precedence over
//! relay copies; conflicting exact identities remain unknown. Only exact,
//! case-sensitive IDs and bundled canonical `provider/id` keys are recognized:
//! no fuzzy matching, suffix removal, version inference, discovery or network I/O.
//! Scheduled or otherwise unrepresentable tariffs remain unknown, not flattened.
use crate::{
    Protocol,
    model_config::{
        ModelPricing, ModelSettings, ModelType, ReasoningConfig, ReasoningDialect, ReasoningEffort,
    },
};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::LazyLock};

/// Advertised model limits and public USD-per-million-token reference prices.
/// Reference prices are estimates, never a relay provider's actual bill.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ReferenceModel {
    /// Advertised context capacity, if known in the source snapshot.
    pub context_window: Option<u32>,
    /// Advertised maximum output, not the requested output reservation.
    pub max_output: Option<u32>,
    /// Public token tariff, including representable long-context tiers.
    pub pricing: Option<ModelPricing>,
    /// Source-supported reasoning dialect and ordered efforts, if representable.
    pub reasoning: Option<ReasoningConfig>,
    /// Exact source input modalities include images; missing evidence stays unknown.
    pub supports_vision: Option<bool>,
    /// Explicit native tool support or a declared supported tool API.
    pub supports_tools: Option<bool>,
}

#[derive(Deserialize)]
struct ReferenceCatalog {
    // A null entry records an ambiguous exact identity, rather than picking a
    // relay's limits or prices arbitrarily.
    models: BTreeMap<String, Option<ReferenceModel>>,
    // Matching reasoning remains usable even when limits/prices conflict.
    ambiguous_reasoning: BTreeMap<String, ReasoningConfig>,
}

static CATALOG: LazyLock<Option<ReferenceCatalog>> =
    LazyLock::new(|| serde_json::from_str(include_str!("model_reference.json")).ok());

/// Resolves a unique exact bundled identity without allocating per lookup.
/// Unknown/ambiguous IDs or unreadable bundled data return `None`.
/// The shared immutable result is decoded only once for all hosts.
pub fn resolve(id: &str) -> Option<&'static ReferenceModel> {
    CATALOG.as_ref()?.models.get(id)?.as_ref()
}

/// Returns transient runtime/display settings, preserving explicit overrides.
///
/// Callers must keep saving their original settings, not this derived copy.
/// Missing context, chat pricing, capabilities and compatible reasoning are filled.
/// The advertised output max is not the user's requested output reservation.
/// `pricing_reference` tracks derived prices in memory and is never serialized.
pub fn effective_settings(id: &str, protocol: Protocol, settings: &ModelSettings) -> ModelSettings {
    let mut effective = settings.clone();
    effective.supports_vision = effective
        .vision_override
        .unwrap_or(effective.supports_vision);
    if let Some(reference) = resolve(id) {
        effective.context_window = effective.context_window.or(reference.context_window);
        if effective.vision_override.is_none() {
            effective.supports_vision |= reference.supports_vision == Some(true);
        }
        if effective.model_type == ModelType::Chat {
            effective.supports_tools = effective.supports_tools.or(reference.supports_tools);
        }
        if effective.model_type == ModelType::Chat && effective.pricing.is_none() {
            effective.pricing = reference.pricing.clone();
            effective.pricing_reference = effective.pricing.is_some();
        }
    }
    if effective.model_type == ModelType::Chat
        && effective.reasoning.is_none()
        && !effective.reasoning_disabled
    {
        let reference = resolve(id)
            .and_then(|model| model.reasoning.as_ref())
            .or_else(|| CATALOG.as_ref()?.ambiguous_reasoning.get(id));
        effective.reasoning = reference
            .filter(|reasoning| match reasoning.dialect {
                ReasoningDialect::OpenAi => {
                    matches!(protocol, Protocol::OpenAiChat | Protocol::OpenAiResponses)
                }
                ReasoningDialect::AnthropicAdaptive => protocol == Protocol::AnthropicMessages,
                _ => false,
            })
            .cloned();
        // ThinkRail/Cherry expose this relay model as the five-step control:
        // off, minimal, low, medium, high. Keep the request vocabulary aligned
        // with that host contract instead of exposing the public catalog's
        // xhigh/max tiers on this route.
        if id == "gpt-6.1-sol" && effective.reasoning.is_some() {
            if let Some(reasoning) = &mut effective.reasoning {
                reasoning.efforts = vec![
                    ReasoningEffort::None,
                    ReasoningEffort::Minimal,
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ];
            }
        }
    }
    if effective.reasoning_override == Some(false) {
        effective.reasoning = None;
        effective.reasoning_effort = None;
    }
    effective
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_config::{CostSource, PriceTier, RoundUsage, TokenUsage};

    #[test]
    fn exact_canonical_identity_and_prefix_share_public_metadata() {
        let reference = resolve("gpt-5.4").unwrap();
        assert_eq!(reference.context_window, Some(1_000_000));
        assert_eq!(reference.max_output, Some(128_000));
        assert_eq!(reference.pricing.as_ref().unwrap().tiers[0].input, 2.5);
        assert_eq!(resolve("openai/gpt-5.4"), Some(reference));
        // Canonical API metadata wins over a subscription relay's smaller cap.
        assert_ne!(reference.context_window, Some(272_000));
    }

    #[test]
    fn ambiguous_unknown_and_affixed_identities_stay_unknown() {
        for id in [
            "Gryphe/MythoMax-L2-13b",
            "not-a-catalog-model",
            "relay/gpt-5.4",
            "GPT-5.4",
            "gpt-5.4-thinking",
            "gpt-5.4:cloud",
            "@openai/gpt-5.4",
        ] {
            assert!(resolve(id).is_none(), "unexpected reference for {id}");
        }
    }

    #[test]
    fn overrides_win_and_derived_settings_do_not_mutate_saved_settings() {
        let saved = ModelSettings::default();
        let effective = effective_settings("gpt-5.4", Protocol::OpenAiChat, &saved);
        assert_eq!(effective.context_window, Some(1_000_000));
        assert_eq!(effective.max_output, None);
        assert_eq!(effective.output_limit(), saved.output_limit());
        assert!(effective.pricing_reference);
        assert_eq!(
            effective_settings("gpt-5.4", Protocol::OpenAiChat, &effective),
            effective
        );
        assert_eq!(saved, ModelSettings::default());
        let custom_price = ModelPricing {
            currency: "CNY".into(),
            tiers: vec![PriceTier {
                min_input_tokens: 0,
                input: 1.0,
                output: 2.0,
                cache_read: None,
                cache_write: None,
            }],
        };
        let explicit = ReasoningConfig {
            dialect: ReasoningDialect::OpenAi,
            efforts: vec![crate::model_config::ReasoningEffort::High],
            budget_tokens: None,
        };
        let custom = ModelSettings {
            context_window: Some(32_768),
            max_output: Some(2_048),
            pricing: Some(custom_price),
            reasoning: Some(explicit),
            supports_vision: true,
            supports_tools: Some(false),
            ..Default::default()
        };
        assert_eq!(
            effective_settings("gpt-5.4", Protocol::OpenAiChat, &custom),
            custom
        );
        assert_eq!(
            effective_settings("unknown", Protocol::OpenAiChat, &saved),
            saved
        );
        let non_chat = ModelSettings {
            model_type: ModelType::Embedding,
            ..Default::default()
        };
        let effective = effective_settings("gpt-5.4", Protocol::OpenAiChat, &non_chat);
        assert!(effective.pricing.is_none());
        assert!(effective.reasoning.is_none());
        assert!(effective.supports_tools.is_none());
    }

    #[test]
    fn exact_capabilities_fill_unknown_without_overriding_manual_choices() {
        let saved = ModelSettings::default();
        for id in [
            "gpt-6-sol",
            "gpt-6.1-sol",
            "gpt-6-astra",
            "openai/gpt-6.1-sol",
        ] {
            let reference = resolve(id).unwrap();
            assert_eq!(reference.supports_vision, Some(true));
            assert_eq!(reference.supports_tools, Some(true));
            let effective = effective_settings(id, Protocol::OpenAiChat, &saved);
            assert!(effective.supports_vision);
            assert_eq!(effective.supports_tools, Some(true));
            assert!(effective.reasoning.is_some());
            assert_eq!(
                effective_settings(id, Protocol::OpenAiChat, &effective),
                effective
            );
        }
        for id in [
            "not-a-catalog-model",
            "relay/gpt-6.1-sol",
            "GPT-6.1-SOL",
            "Gryphe/MythoMax-L2-13b",
        ] {
            let effective = effective_settings(id, Protocol::OpenAiChat, &saved);
            assert!(!effective.supports_vision, "unexpected vision for {id}");
            assert!(
                effective.supports_tools.is_none(),
                "unexpected tools for {id}"
            );
        }
        let disabled = ModelSettings {
            supports_vision: true,
            vision_override: Some(false),
            supports_tools: Some(false),
            ..Default::default()
        };
        let effective = effective_settings("gpt-6.1-sol", Protocol::OpenAiChat, &disabled);
        assert!(!effective.supports_vision);
        assert_eq!(effective.supports_tools, Some(false));
        assert!(disabled.supports_vision);
        let legacy: ModelSettings = serde_json::from_str(r#"{"supports_vision":true}"#).unwrap();
        assert_eq!(legacy.vision_override, None);
        assert!(effective_settings("unknown", Protocol::OpenAiChat, &legacy).supports_vision);
        let explicit = ModelSettings {
            vision_override: Some(true),
            ..Default::default()
        };
        assert!(effective_settings("unknown", Protocol::OpenAiChat, &explicit).supports_vision);
        let restored: ModelSettings =
            serde_json::from_str(&serde_json::to_string(&disabled).unwrap()).unwrap();
        assert_eq!(restored.vision_override, Some(false));
        assert_eq!(saved, ModelSettings::default());
    }

    #[test]
    fn reasoning_preserves_source_efforts_and_respects_protocol_and_explicit_disable() {
        use crate::model_config::ReasoningEffort::{
            High, Low, Max, Medium, Minimal, None as Off, Xhigh,
        };
        let _ = Off;
        let saved = ModelSettings::default();
        let reference = resolve("gpt-6.1-sol").unwrap().reasoning.as_ref().unwrap();
        assert_eq!(reference.dialect, ReasoningDialect::OpenAi);
        assert_eq!(reference.efforts, vec![Low, Medium, High, Xhigh, Max]);
        assert_eq!(
            resolve("openai/gpt-6.1-sol").unwrap().reasoning.as_ref(),
            Some(reference)
        );
        for protocol in [Protocol::OpenAiChat, Protocol::OpenAiResponses] {
            let effective = effective_settings("gpt-6.1-sol", protocol, &saved);
            assert_eq!(
                effective.reasoning.as_ref().unwrap().efforts,
                vec![Off, Minimal, Low, Medium, High]
            );
            assert!(effective.validate(protocol).is_ok());
        }
        assert!(
            effective_settings("gpt-6.1-sol", Protocol::AnthropicMessages, &saved)
                .reasoning
                .is_none()
        );
        assert!(
            effective_settings("claude-opus-5-5", Protocol::OpenAiChat, &saved)
                .reasoning
                .is_none()
        );
        let adaptive = effective_settings("claude-opus-5-5", Protocol::AnthropicMessages, &saved);
        assert_eq!(
            adaptive.reasoning.as_ref().unwrap().dialect,
            ReasoningDialect::AnthropicAdaptive
        );
        assert_eq!(
            adaptive.reasoning.as_ref().unwrap().efforts,
            reference.efforts
        );
        assert!(
            effective_settings("claude-haiku-4-5", Protocol::AnthropicMessages, &saved)
                .reasoning
                .is_none()
        );
        let independently_known =
            effective_settings("Gryphe/MythoMax-L2-13b", Protocol::OpenAiChat, &saved);
        assert!(independently_known.context_window.is_none());
        assert!(independently_known.pricing.is_none());
        assert!(independently_known.reasoning.is_some());
        let disabled = ModelSettings {
            reasoning_disabled: true,
            ..Default::default()
        };
        assert!(
            effective_settings("gpt-6.1-sol", Protocol::OpenAiChat, &disabled)
                .reasoning
                .is_none()
        );
        let explicit = ModelSettings {
            reasoning: Some(ReasoningConfig {
                dialect: ReasoningDialect::OpenAi,
                efforts: vec![Max, High],
                budget_tokens: None,
            }),
            reasoning_disabled: true,
            ..Default::default()
        };
        assert_eq!(
            effective_settings("gpt-6.1-sol", Protocol::OpenAiChat, &explicit).reasoning,
            explicit.reasoning
        );
        assert_eq!(
            effective_settings("gpt-6.1-sol", Protocol::AnthropicMessages, &explicit).reasoning,
            explicit.reasoning
        );
        assert_eq!(saved, ModelSettings::default());
    }

    #[test]
    fn scheduled_and_unpriced_tariffs_remain_unknown() {
        let scheduled = resolve("deepseek-flash").unwrap();
        assert!(scheduled.context_window.is_some());
        assert!(scheduled.pricing.is_none());
        assert!(resolve("gpt-image-1").unwrap().pricing.is_none());
    }

    #[test]
    fn long_context_threshold_preserves_exclusive_boundary_and_cache_prices() {
        let pricing = resolve("claude-haiku-5-5")
            .unwrap()
            .pricing
            .as_ref()
            .unwrap();
        assert_eq!(pricing.tiers[1].min_input_tokens, 100_001);
        assert_eq!(pricing.tiers[1].cache_read, Some(0.05));
        assert_eq!(pricing.tiers[1].cache_write, Some(0.625));
        let base = TokenUsage {
            input_tokens: 100_000,
            ..Default::default()
        };
        let premium = TokenUsage {
            input_tokens: 100_001,
            ..Default::default()
        };
        assert_eq!(pricing.cost(&base), Some(0.01));
        assert_eq!(pricing.cost(&premium), Some(0.0500005));
    }

    #[test]
    fn reference_costs_retain_reference_not_billing_provenance() {
        let settings =
            effective_settings("gpt-5.4", Protocol::OpenAiChat, &ModelSettings::default());
        let round = RoundUsage::default().finish(
            Some(TokenUsage {
                input_tokens: 1_000_000,
                output_tokens: 1_000_000,
                ..Default::default()
            }),
            settings.pricing,
            settings.pricing_reference,
            std::time::Instant::now(),
        );
        assert_eq!(round.cost_source, CostSource::Reference);
        assert_eq!(round.cost, Some(17.5));
    }
}
