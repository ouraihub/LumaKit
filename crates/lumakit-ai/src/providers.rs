//! Provider registry, URL validation, and deliberately non-plaintext persistence.
use crate::{AiConnection, Protocol};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use thiserror::Error;
use zeroize::Zeroizing;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("provider not found: {0}")]
    ProviderNotFound(String),
    #[error("model not found: {0}")]
    ModelNotFound(String),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid provider data: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelCapabilities {
    pub streaming: bool,
    pub tools: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub capabilities: ModelCapabilities,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoint {
    pub name: String,
    pub base_url: String,
    pub protocol: Protocol,
    #[serde(default)]
    pub models: Vec<Model>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub endpoints: Vec<Endpoint>,
    #[serde(default)]
    pub key: KeyState,
}

/// Secret states are explicit. `Present` is memory-only and never serializes;
/// `Sealed` is an opaque OS/keyring envelope, not plaintext (DPAPI is layered on later).
#[derive(Clone, Default)]
pub enum KeyState {
    #[default]
    None,
    Sealed(Vec<u8>),
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
    pub fn sealed(bytes: impl Into<Vec<u8>>) -> Self {
        Self::Sealed(bytes.into())
    }
    pub fn present(secret: impl Into<String>) -> Self {
        Self::Present(Zeroizing::new(secret.into()))
    }
    pub fn is_configured(&self) -> bool {
        !matches!(self, Self::None)
    }
    pub fn sealed_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Sealed(v) => Some(v),
            _ => None,
        }
    }
    pub fn secret(&self) -> Option<&str> {
        match self {
            Self::Present(v) => Some(v.as_str()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyChange {
    Keep,
    Clear,
    Replace,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoreFile {
    providers: Vec<Provider>,
}

#[derive(Debug, Default)]
pub struct ProviderStore {
    providers: Vec<Provider>,
    path: Option<PathBuf>,
}
impl ProviderStore {
    pub fn new() -> Self {
        Self::default()
    }
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
    pub fn providers(&self) -> &[Provider] {
        &self.providers
    }
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
        Ok(AiConnection {
            base_url: e.base_url.clone(),
            protocol: e.protocol,
            api_key: p.key.secret().map(|s| Zeroizing::new(s.to_owned())),
            model: model.into(),
        })
    }
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
    }
    #[test]
    fn key_never_outputs_full_secret() {
        let key = KeyState::present("super-secret");
        let text = format!("{:?} {}", key, key_mask(&key));
        assert!(!text.contains("super-secret"));
    }
}
