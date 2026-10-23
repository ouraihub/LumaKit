//! 已保存的服务商目录；界面只读描述，执行时才解密所选凭据。
use crate::{AiConnection, ModelRef, Protocol, providers::ModelCapabilities};
use lumakit_support::secrets::SavedKey;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct ModelEntry { pub id: String, pub capabilities: ModelCapabilities, pub manual: bool }
#[derive(Clone, Serialize, Deserialize)]
pub struct EndpointEntry {
    pub id: String, pub name: String, pub base_url: String, pub protocol: Protocol,
    pub keys: Vec<SavedKey>, pub models: Vec<ModelEntry>, pub selected_key: Option<String>, pub selected_model: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct ProviderEntry { pub id: String, pub name: String, pub enabled: bool, pub endpoints: Vec<EndpointEntry> }

/// 只接收已保存快照；草稿从不参与请求。
pub struct ModelCatalog { providers: Vec<ProviderEntry> }
impl ModelCatalog {
    pub fn new(providers: Vec<ProviderEntry>) -> Self { Self { providers } }
    pub fn providers(&self) -> &[ProviderEntry] { &self.providers }
    pub fn resolve(&self, model: &ModelRef) -> Result<AiConnection, String> {
        let provider = self.providers.iter().find(|p| p.enabled && p.id == model.provider).ok_or("所选服务商已停用或删除，请重新选择模型")?;
        let endpoint = provider.endpoints.iter().find(|e| e.id == model.endpoint && e.protocol == model.protocol).ok_or("所选线路已更改或删除，请重新选择模型")?;
        if !endpoint.models.iter().any(|m| m.id == model.model) { return Err("所选模型已删除，请重新选择模型".into()); }
        let key = endpoint.selected_key.as_ref().and_then(|id| endpoint.keys.iter().find(|k| &k.id == id)).or_else(|| endpoint.keys.first());
        Ok(AiConnection { base_url: validate_url(&endpoint.base_url)?, protocol: endpoint.protocol, model: model.model.clone(), api_key: key.map(SavedKey::open).transpose()? })
    }
}

pub fn validate_url(raw: &str) -> Result<String, String> {
    if raw.len() > 4096 { return Err("基础地址过长，最多 4096 字节".into()); }
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| "基础地址无效，请填写完整 HTTPS 地址".to_owned())?;
    let host = url.host_str().ok_or_else(|| "地址缺少主机名".to_owned())?;
    let loopback = host.eq_ignore_ascii_case("localhost") || host.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() { return Err("地址不得包含用户名、密码、查询参数或片段".into()); }
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) { return Err("仅允许 HTTPS；HTTP 仅限回环主机".into()); }
    Ok(url.to_string().trim_end_matches('/').to_owned())
}
