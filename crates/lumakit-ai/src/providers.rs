//! Provider registry, URL validation, and deliberately non-plaintext persistence.
use crate::{AiConnection, Protocol};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use thiserror::Error;
use zeroize::Zeroizing;

/// 服务商配置解析、查找或文件读写失败的原因。
#[derive(Debug, Error)]
pub enum ProviderError {
    /// 基础地址无效及其原因。
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    /// 未找到指定标识的服务商。
    #[error("provider not found: {0}")]
    ProviderNotFound(String),
    /// 未找到指定标识的模型。
    #[error("model not found: {0}")]
    ModelNotFound(String),
    /// 配置文件读写失败。
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// 配置 JSON 解析或序列化失败。
    #[error("invalid provider data: {0}")]
    Json(#[from] serde_json::Error),
}

/// 模型声明支持的功能；默认仅启用流式输出。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelCapabilities {
    /// 是否支持流式输出。
    pub streaming: bool,
    /// 是否支持工具调用。
    pub tools: bool,
    /// 是否支持图片输入。
    pub vision: bool,
}
impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            streaming: true,
            tools: false,
            vision: false,
        }
    }
}

/// 服务商线路下的模型及其能力声明。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    /// 模型标识。
    pub id: String,
    /// 模型支持的功能。
    #[serde(default)]
    pub capabilities: ModelCapabilities,
}
/// 服务商的一条请求线路及其模型列表。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoint {
    /// 线路名称。
    pub name: String,
    /// 请求基础地址。
    pub base_url: String,
    /// 线路使用的协议。
    pub protocol: Protocol,
    /// 线路提供的模型。
    #[serde(default)]
    pub models: Vec<Model>,
}
/// 服务商标识、线路和凭据配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    /// 服务商标识。
    pub id: String,
    /// 服务商显示名称。
    pub name: String,
    /// 服务商可用的请求线路。
    #[serde(default)]
    pub endpoints: Vec<Endpoint>,
    /// 服务商凭据状态。
    #[serde(default)]
    pub key: KeyState,
}

/// Secret states are explicit. `Present` is memory-only and never serializes;
/// `Sealed` is an opaque OS/keyring envelope, not plaintext (DPAPI is layered on later).
#[derive(Clone, Default)]
pub enum KeyState {
    /// 未配置凭据。
    #[default]
    None,
    /// 由外部密钥设施封装的字节；此类型不负责加解密。
    Sealed(Vec<u8>),
    /// 仅驻留内存、释放时清零且不序列化的明文凭据。
    Present(Zeroizing<String>),
}
impl std::fmt::Debug for KeyState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::Sealed(_) => f.write_str("Sealed(<opaque>)"),
            Self::Present(_) => f.write_str("Present(<redacted>)"),
        }
    }
}
impl Serialize for KeyState {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::None => s.serialize_none(),
            Self::Sealed(v) => s.serialize_bytes(v),
            Self::Present(_) => s.serialize_none(),
        }
    }
}
impl<'de> Deserialize<'de> for KeyState {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Option::<Vec<u8>>::deserialize(d)?;
        Ok(v.map(Self::Sealed).unwrap_or(Self::None))
    }
}
impl KeyState {
    /// 保存外部封装的字节，不执行加密或格式验证。
    pub fn sealed(bytes: impl Into<Vec<u8>>) -> Self {
        Self::Sealed(bytes.into())
    }
    /// 创建仅驻留内存且释放时清零的明文凭据。
    pub fn present(secret: impl Into<String>) -> Self {
        Self::Present(Zeroizing::new(secret.into()))
    }
    /// 是否配置了封装凭据或内存凭据。
    pub fn is_configured(&self) -> bool {
        !matches!(self, Self::None)
    }
    /// 返回封装字节；其它状态返回空。
    pub fn sealed_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Sealed(v) => Some(v),
            _ => None,
        }
    }
    /// 返回内存中的明文凭据；不会解封封装字节。
    pub fn secret(&self) -> Option<&str> {
        match self {
            Self::Present(v) => Some(v.as_str()),
            _ => None,
        }
    }
}

/// 编辑凭据时请求执行的变更类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyChange {
    /// 保留现有凭据。
    Keep,
    /// 清除现有凭据。
    Clear,
    /// 替换现有凭据。
    Replace,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoreFile {
    providers: Vec<Provider>,
}

/// 在内存中管理服务商配置，并可显式保存至加载路径。
#[derive(Debug, Default)]
pub struct ProviderStore {
    providers: Vec<Provider>,
    path: Option<PathBuf>,
}
impl ProviderStore {
    /// 创建没有持久化路径的空存储。
    pub fn new() -> Self {
        Self::default()
    }
    /// 从 JSON 文件加载配置；文件不存在时创建绑定该路径的空存储。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ProviderError> {
        let path = path.as_ref().to_path_buf();
        if !path.exists() {
            return Ok(Self {
                providers: Vec::new(),
                path: Some(path),
            });
        }
        let data = fs::read(&path)?;
        let file: StoreFile = serde_json::from_slice(&data)?;
        Ok(Self {
            providers: file.providers,
            path: Some(path),
        })
    }
    /// 返回当前服务商配置的只读切片。
    pub fn providers(&self) -> &[Provider] {
        &self.providers
    }
    /// 校验并规范化所有线路地址，按服务商标识插入或替换；不自动保存。
    pub fn upsert(&mut self, mut provider: Provider) -> Result<(), ProviderError> {
        for endpoint in &mut provider.endpoints {
            endpoint.base_url = normalize_url(&endpoint.base_url)?;
        }
        match self.providers.iter().position(|p| p.id == provider.id) {
            Some(i) => self.providers[i] = provider,
            None => self.providers.push(provider),
        }
        Ok(())
    }
    /// 删除指定服务商并返回是否找到；不自动保存。
    pub fn remove(&mut self, id: &str) -> bool {
        self.providers
            .iter()
            .position(|p| p.id == id)
            .map(|i| {
                self.providers.remove(i);
                true
            })
            .unwrap_or(false)
    }
    /// 通过临时文件写入并重命名至加载路径；无路径时返回错误。
    pub fn save(&self) -> Result<(), ProviderError> {
        let path = self
            .path
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "store has no path"))?;
        let bytes = serde_json::to_vec_pretty(&StoreFile {
            providers: self.providers.clone(),
        })?;
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, bytes)?;
        fs::rename(tmp, path)?;
        Ok(())
    }
    /// 从首条包含模型的线路构造连接；仅复制内存明文凭据，不解封密钥。
    pub fn resolve(&self, provider: &str, model: &str) -> Result<AiConnection, ProviderError> {
        let p = self
            .providers
            .iter()
            .find(|p| p.id == provider)
            .ok_or_else(|| ProviderError::ProviderNotFound(provider.into()))?;
        let e = p
            .endpoints
            .iter()
            .find(|e| e.models.iter().any(|m| m.id == model))
            .ok_or_else(|| ProviderError::ModelNotFound(model.into()))?;
        let capabilities = e
            .models
            .iter()
            .find(|entry| entry.id == model)
            .ok_or_else(|| ProviderError::ModelNotFound(model.into()))?
            .capabilities;
        Ok(AiConnection {
            base_url: e.base_url.clone(),
            protocol: e.protocol,
            api_key: p.key.secret().map(|s| Zeroizing::new(s.to_owned())),
            model: model.into(),
            settings: crate::model_config::ModelSettings {
                supports_streaming: capabilities.streaming,
                supports_tools: capabilities.tools.then_some(true),
                supports_vision: capabilities.vision,
                ..Default::default()
            },
        })
    }
    /// 返回指定服务商中首个匹配模型的能力；找不到时返回错误。
    pub fn capabilities(
        &self,
        provider: &str,
        model: &str,
    ) -> Result<ModelCapabilities, ProviderError> {
        let p = self
            .providers
            .iter()
            .find(|p| p.id == provider)
            .ok_or_else(|| ProviderError::ProviderNotFound(provider.into()))?;
        p.endpoints
            .iter()
            .flat_map(|e| &e.models)
            .find(|m| m.id == model)
            .map(|m| m.capabilities)
            .ok_or_else(|| ProviderError::ModelNotFound(model.into()))
    }
}

/// 校验 HTTP 或 HTTPS 地址，去掉首尾空白、片段及末尾斜杠。
pub fn normalize_url(raw: &str) -> Result<String, ProviderError> {
    let mut u =
        reqwest::Url::parse(raw.trim()).map_err(|e| ProviderError::InvalidUrl(e.to_string()))?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
        return Err(ProviderError::InvalidUrl(raw.into()));
    }
    u.set_fragment(None);
    let mut out = u.to_string();
    while out.ends_with('/') {
        out.pop();
    }
    Ok(out)
}
/// 生成凭据掩码；短凭据显示四个圆点，长凭据保留首尾各两字节。
///
/// 无明文且封装字节不是 UTF-8 时返回空字符串。
///
/// # Panics
///
/// 可用凭据超过四字节且首尾两字节切片不在 UTF-8 字符边界时会恐慌。
pub fn key_mask(key: &KeyState) -> String {
    let secret = key
        .secret()
        .or_else(|| key.sealed_bytes().and_then(|b| std::str::from_utf8(b).ok()));
    secret
        .map(|s| {
            if s.len() <= 4 {
                "••••".into()
            } else {
                format!("{}••••{}", &s[..2], &s[s.len() - 2..])
            }
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn url_and_resolve() {
        assert_eq!(
            normalize_url(" https://example.com/api/// ").unwrap(),
            "https://example.com/api"
        );
        let mut s = ProviderStore::new();
        s.upsert(Provider {
            id: "p".into(),
            name: "P".into(),
            key: KeyState::present("secret"),
            endpoints: vec![Endpoint {
                name: "e".into(),
                base_url: "https://x/".into(),
                protocol: Protocol::OpenAiChat,
                models: vec![Model {
                    id: "m".into(),
                    capabilities: ModelCapabilities {
                        tools: true,
                        ..Default::default()
                    },
                }],
            }],
        })
        .unwrap();
        assert_eq!(s.resolve("p", "m").unwrap().base_url, "https://x");
        let connection = s.resolve("p", "m").unwrap();
        assert_eq!(connection.settings.supports_tools, Some(true));
        assert!(connection.settings.supports_streaming);
        s.providers[0].endpoints[0].models[0].capabilities = ModelCapabilities {
            streaming: false,
            tools: false,
            vision: true,
        };
        let connection = s.resolve("p", "m").unwrap();
        assert!(!connection.settings.supports_streaming);
        assert_eq!(connection.settings.supports_tools, None);
        assert!(connection.settings.supports_vision);
    }
    #[test]
    fn key_never_outputs_full_secret() {
        let key = KeyState::present("super-secret");
        let text = format!("{:?} {}", key, key_mask(&key));
        assert!(!text.contains("super-secret"));
    }
}
