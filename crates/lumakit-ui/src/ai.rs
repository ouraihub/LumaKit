//! UI-neutral AI settings contracts.

use lumakit_ai::images::ImagePart;

/// Safe provider state exposed to settings UI; credentials are never included.
#[derive(Debug, Clone)]
pub struct ProviderUiSnapshot {
    pub id: String,
    pub name: String,
    pub endpoints: Vec<ProviderUiEndpoint>,
    pub has_key: bool,
}

#[derive(Debug, Clone)]
pub struct ProviderUiEndpoint {
    pub name: String,
    pub base_url: String,
    pub protocol: lumakit_ai::Protocol,
    pub models: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SkillUiMeta {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub origin: String,
    pub revision: u64,
    pub digest: String,
}

#[derive(Debug, Clone)]
pub struct McpUiSnapshot {
    pub url: String,
    pub enabled: bool,
    pub trusted: bool,
}

#[derive(Debug, Clone)]
pub enum ImageInputAction {
    Add(ImagePart),
    Remove { index: usize },
    Clear,
}

pub trait AiSettingsPort: Send + Sync {
    fn list_providers(&self) -> Result<Vec<ProviderUiSnapshot>, String>;
    fn save_provider(&self, provider: ProviderUiSnapshot) -> Result<(), String>;
    fn remove_provider(&self, provider_id: &str) -> Result<(), String>;
    fn probe_provider(&self, provider_id: &str) -> Result<(), String>;

    fn list_skills(&self) -> Result<Vec<SkillUiMeta>, String>;
    fn preview_skill(&self, document: &str) -> Result<SkillUiMeta, String>;
    fn install_skill(&self, document: &str, expected_digest: &str) -> Result<SkillUiMeta, String>;

    fn list_mcp(&self) -> Result<Vec<McpUiSnapshot>, String>;
    fn trust_mcp(&self, url: &str, trusted: bool) -> Result<(), String>;
    fn toggle_mcp(&self, url: &str, enabled: bool) -> Result<(), String>;

    fn image_action(&self, action: ImageInputAction) -> Result<(), String>;
}
