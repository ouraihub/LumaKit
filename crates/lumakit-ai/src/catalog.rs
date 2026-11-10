//! 已保存的服务商目录；界面只读描述，执行时才解密所选凭据。
use crate::{AiConnection, ModelRef, Protocol, providers::ModelCapabilities};
use lumakit_support::secrets::SavedKey;
use serde::{Deserialize, Serialize};

/// 已保存模型的标识、能力与录入来源。
#[derive(Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    /// 模型标识。
    pub id: String,
    /// 模型声明支持的功能。
    pub capabilities: ModelCapabilities,
    /// 是否为手动录入的模型。
    pub manual: bool,
    /// 已保存的模型元数据和实际请求控制。
    #[serde(default)]
    pub settings: crate::model_config::ModelSettings,
}
impl ModelEntry {
    /// Runtime/display metadata; explicit settings win over legacy true flags and exact references.
    /// Keep persisting `settings`, not this derived copy.
    pub fn effective_settings(&self, protocol: Protocol) -> crate::model_config::ModelSettings {
        let mut settings =
            crate::model_reference::effective_settings(&self.id, protocol, &self.settings);
        settings.supports_streaming = self.capabilities.streaming;
        if self.settings.vision_override.is_none() {
            settings.supports_vision |= self.capabilities.vision;
        }
        if settings.model_type == crate::model_config::ModelType::Chat
            && self.settings.supports_tools.is_none()
            && self.capabilities.tools
        {
            settings.supports_tools = Some(true);
        }
        settings
    }
}
/// 已保存线路的地址、协议、模型和凭据选择。
#[derive(Clone, Serialize, Deserialize)]
pub struct EndpointEntry {
    /// 线路标识。
    pub id: String,
    /// 线路显示名称。
    pub name: String,
    /// 请求基础地址，解析连接时会再次校验。
    pub base_url: String,
    /// 请求使用的协议。
    pub protocol: Protocol,
    /// 已保存的封装凭据。
    pub keys: Vec<SavedKey>,
    /// 已保存的模型列表。
    pub models: Vec<ModelEntry>,
    /// 首选凭据标识；不存在或未匹配时解析连接使用首个凭据。
    pub selected_key: Option<String>,
    /// 已保存的模型选择标识；解析连接仍以传入的模型引用为准。
    pub selected_model: Option<String>,
}
/// 已保存的服务商及其启用状态和线路。
#[derive(Clone, Serialize, Deserialize)]
pub struct ProviderEntry {
    /// 服务商标识。
    pub id: String,
    /// 服务商显示名称。
    pub name: String,
    /// 是否允许从该服务商解析连接。
    pub enabled: bool,
    /// 服务商的已保存线路。
    pub endpoints: Vec<EndpointEntry>,
}

/// 只接收已保存快照；草稿从不参与请求。
pub struct ModelCatalog {
    providers: Vec<ProviderEntry>,
}
impl ModelCatalog {
    /// 使用已保存的服务商快照创建目录。
    pub fn new(providers: Vec<ProviderEntry>) -> Self {
        Self { providers }
    }
    /// 返回目录中的服务商只读切片。
    pub fn providers(&self) -> &[ProviderEntry] {
        &self.providers
    }
    /// 校验模型引用和线路地址，并解封选中凭据以构造连接。
    ///
    /// # Errors
    /// 服务商停用、线路或模型不匹配、地址无效或凭据解封失败时返回错误。
    pub fn resolve(&self, model: &ModelRef) -> Result<AiConnection, String> {
        let provider = self
            .providers
            .iter()
            .find(|p| p.enabled && p.id == model.provider)
            .ok_or("所选服务商已停用或删除，请重新选择模型")?;
        let endpoint = provider
            .endpoints
            .iter()
            .find(|e| e.id == model.endpoint && e.protocol == model.protocol)
            .ok_or("所选线路已更改或删除，请重新选择模型")?;
        let entry = endpoint
            .models
            .iter()
            .find(|m| m.id == model.model)
            .ok_or("所选模型已删除，请重新选择模型")?;
        entry.settings.validate(endpoint.protocol)?;
        if entry.settings.model_type != crate::model_config::ModelType::Chat {
            return Err("所选模型不是聊天类型，请使用对应业务接口".into());
        }
        let key = endpoint
            .selected_key
            .as_ref()
            .and_then(|id| endpoint.keys.iter().find(|k| &k.id == id))
            .or_else(|| endpoint.keys.first());
        let settings = entry.effective_settings(endpoint.protocol);
        Ok(AiConnection {
            base_url: validate_url(&endpoint.base_url)?,
            protocol: endpoint.protocol,
            model: model.model.clone(),
            api_key: key.map(SavedKey::open).transpose()?,
            settings,
        })
    }
    /// Resolves a saved model while requiring a specific non-chat consumer type.
    ///
    /// # Errors
    /// Returns an error for a disabled/missing provider, changed endpoint or model,
    /// invalid settings or URL, a mismatched model type, or credentials that cannot be opened.
    pub fn resolve_for(
        &self,
        model: &ModelRef,
        expected: crate::model_config::ModelType,
    ) -> Result<AiConnection, String> {
        let provider = self
            .providers
            .iter()
            .find(|p| p.enabled && p.id == model.provider)
            .ok_or("所选服务商已停用或删除，请重新选择模型")?;
        let endpoint = provider
            .endpoints
            .iter()
            .find(|e| e.id == model.endpoint && e.protocol == model.protocol)
            .ok_or("所选线路已更改或删除，请重新选择模型")?;
        let entry = endpoint
            .models
            .iter()
            .find(|m| m.id == model.model)
            .ok_or("所选模型已删除，请重新选择模型")?;
        entry.settings.validate(endpoint.protocol)?;
        if entry.settings.model_type != expected {
            return Err("所选模型类型与当前业务不匹配".into());
        }
        let key = endpoint
            .selected_key
            .as_ref()
            .and_then(|id| endpoint.keys.iter().find(|k| &k.id == id))
            .or_else(|| endpoint.keys.first());
        let settings = entry.effective_settings(endpoint.protocol);
        Ok(AiConnection {
            base_url: validate_url(&endpoint.base_url)?,
            protocol: endpoint.protocol,
            model: model.model.clone(),
            api_key: key.map(SavedKey::open).transpose()?,
            settings,
        })
    }
}

/// 校验基础地址并移除末尾斜杠；仅允许 HTTPS 或回环主机的 HTTP。
///
/// 拒绝超过 4096 字节、缺少主机名或含凭据、查询参数、片段的地址。
///
/// # Errors
/// Returns an error if parsing fails or the URL violates the length, host,
/// credential, query, fragment, or HTTPS/loopback policy.
pub fn validate_url(raw: &str) -> Result<String, String> {
    if raw.len() > 4096 {
        return Err("基础地址过长，最多 4096 字节".into());
    }
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|_| "基础地址无效，请填写完整 HTTPS 地址".to_owned())?;
    let host = url.host_str().ok_or_else(|| "地址缺少主机名".to_owned())?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("地址不得包含用户名、密码、查询参数或片段".into());
    }
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err("仅允许 HTTPS；HTTP 仅限回环主机".into());
    }
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod model_settings_tests {
    use super::*;
    #[test]
    fn resolver_preserves_unknown_tools_and_rejects_non_chat()
    -> Result<(), Box<dyn std::error::Error>> {
        let model = ModelRef {
            provider: "p".into(),
            endpoint: "e".into(),
            model: "m".into(),
            protocol: Protocol::OpenAiChat,
        };
        let entry = ModelEntry {
            id: "m".into(),
            capabilities: ModelCapabilities::default(),
            manual: true,
            settings: Default::default(),
        };
        let mut providers = vec![ProviderEntry {
            id: "p".into(),
            name: "p".into(),
            enabled: true,
            endpoints: vec![EndpointEntry {
                id: "e".into(),
                name: "e".into(),
                base_url: "https://example.com/v1".into(),
                protocol: Protocol::OpenAiChat,
                keys: Vec::new(),
                models: vec![entry],
                selected_key: None,
                selected_model: None,
            }],
        }];
        assert_eq!(
            ModelCatalog::new(providers.clone())
                .resolve(&model)?
                .settings
                .supports_tools,
            None
        );
        providers[0].endpoints[0].models[0].capabilities.tools = true;
        providers[0].endpoints[0].models[0].capabilities.vision = true;
        assert!(
            ModelCatalog::new(providers.clone())
                .resolve(&model)?
                .settings
                .supports_vision
        );
        assert_eq!(
            ModelCatalog::new(providers.clone())
                .resolve(&model)?
                .settings
                .supports_tools,
            Some(true)
        );
        providers[0].endpoints[0].models[0].settings.supports_tools = Some(false);
        assert_eq!(
            ModelCatalog::new(providers.clone())
                .resolve(&model)?
                .settings
                .supports_tools,
            Some(false)
        );
        providers[0].endpoints[0].models[0].settings.vision_override = Some(false);
        assert!(
            !ModelCatalog::new(providers.clone())
                .resolve(&model)?
                .settings
                .supports_vision
        );
        providers[0].endpoints[0].models[0].settings.supports_tools = None;
        providers[0].endpoints[0].models[0].settings.model_type =
            crate::model_config::ModelType::Embedding;
        assert!(
            ModelCatalog::new(providers.clone())
                .resolve(&model)
                .err()
                .ok_or("non-chat model unexpectedly resolved")?
                .contains("不是聊天类型")
        );
        let non_chat = ModelCatalog::new(providers)
            .resolve_for(&model, crate::model_config::ModelType::Embedding)?;
        assert!(non_chat.settings.supports_tools.is_none());
        Ok(())
    }

    #[test]
    fn resolution_inherits_reference_without_saving_overrides()
    -> Result<(), Box<dyn std::error::Error>> {
        let model = ModelRef {
            provider: "p".into(),
            endpoint: "e".into(),
            model: "gpt-5.4".into(),
            protocol: Protocol::OpenAiChat,
        };
        let entry = ModelEntry {
            id: model.model.clone(),
            capabilities: ModelCapabilities::default(),
            manual: true,
            settings: Default::default(),
        };
        let catalog = ModelCatalog::new(vec![ProviderEntry {
            id: "p".into(),
            name: "p".into(),
            enabled: true,
            endpoints: vec![EndpointEntry {
                id: "e".into(),
                name: "e".into(),
                base_url: "https://example.com/v1".into(),
                protocol: Protocol::OpenAiChat,
                keys: Vec::new(),
                models: vec![entry],
                selected_key: None,
                selected_model: None,
            }],
        }]);
        let settings = catalog.resolve(&model)?.settings;
        assert_eq!(settings.context_window, Some(1_000_000));
        assert!(settings.supports_vision);
        assert_eq!(settings.supports_tools, Some(true));
        assert_eq!(settings.max_output, None);
        assert!(settings.pricing_reference);
        assert_eq!(
            catalog.providers()[0].endpoints[0].models[0].settings,
            crate::model_config::ModelSettings::default()
        );
        assert_eq!(
            catalog
                .resolve_for(&model, crate::model_config::ModelType::Chat)?
                .settings,
            settings
        );
        Ok(())
    }

    #[test]
    fn exact_reference_and_legacy_capabilities_respect_explicit_false_at_runtime()
    -> Result<(), Box<dyn std::error::Error>> {
        let model = ModelRef {
            provider: "p".into(),
            endpoint: "e".into(),
            model: "gpt-6.1-sol".into(),
            protocol: Protocol::OpenAiResponses,
        };
        let entry = ModelEntry {
            id: model.model.clone(),
            capabilities: ModelCapabilities {
                streaming: true,
                tools: true,
                vision: true,
            },
            manual: true,
            settings: crate::model_config::ModelSettings {
                supports_tools: Some(false),
                vision_override: Some(false),
                ..Default::default()
            },
        };
        let catalog = ModelCatalog::new(vec![ProviderEntry {
            id: "p".into(),
            name: "p".into(),
            enabled: true,
            endpoints: vec![EndpointEntry {
                id: "e".into(),
                name: "e".into(),
                base_url: "https://example.com/v1".into(),
                protocol: model.protocol,
                keys: Vec::new(),
                models: vec![entry],
                selected_key: None,
                selected_model: None,
            }],
        }]);
        let connection = catalog.resolve(&model)?;
        assert!(!connection.settings.supports_vision);
        assert_eq!(connection.settings.supports_tools, Some(false));
        assert_eq!(
            connection.settings,
            catalog.providers()[0].endpoints[0].models[0].effective_settings(model.protocol)
        );
        Ok(())
    }
}
