//! UI-neutral AI settings contracts.

use lumakit_ai::images::ImagePart;

/// Safe provider state exposed to settings UI; credentials are never included.
#[derive(Debug, Clone)]
pub struct ProviderUiSnapshot {
    /// 提供方标识。
    pub id: String,
    /// 提供方显示名称。
    pub name: String,
    /// 可展示的端点配置，不包含凭据。
    pub endpoints: Vec<ProviderUiEndpoint>,
    /// 是否已配置密钥；不暴露密钥内容。
    pub has_key: bool,
}

/// 设置界面使用的端点配置。
#[derive(Debug, Clone)]
pub struct ProviderUiEndpoint {
    /// 端点名称。
    pub name: String,
    /// 服务基础 URL。
    pub base_url: String,
    /// 端点使用的 AI 协议。
    pub protocol: lumakit_ai::Protocol,
    /// 此端点的模型标识列表。
    pub models: Vec<String>,
}

/// 技能预览、安装及列表界面使用的元数据。
#[derive(Debug, Clone)]
pub struct SkillUiMeta {
    /// 技能标识。
    pub id: String,
    /// 技能显示名称。
    pub name: String,
    /// 技能用途说明。
    pub description: String,
    /// 技能版本字符串。
    pub version: String,
    /// 技能来源说明。
    pub origin: String,
    /// 技能修订号。
    pub revision: u64,
    /// 技能内容摘要，用于核对预览与安装内容。
    pub digest: String,
}

/// MCP 连接在设置界面中的状态快照。
#[derive(Debug, Clone)]
pub struct McpUiSnapshot {
    /// MCP 服务地址。
    pub url: String,
    /// 是否启用连接。
    pub enabled: bool,
    /// 是否已获得用户信任。
    pub trusted: bool,
}

/// 对当前图片输入列表的编辑请求，由宿主执行。
#[derive(Debug, Clone)]
pub enum ImageInputAction {
    /// 添加一张图片。
    Add(ImagePart),
    /// 移除指定索引的图片；越界处理由宿主决定。
    Remove {
        /// 待移除图片的列表索引。
        index: usize,
    },
    /// 清空图片输入列表。
    Clear,
}

/// AI 设置界面依赖的宿主接口；失败以可展示的字符串返回。
///
/// 此接口本身不执行存储、联网或信任校验，具体行为由实现负责。
pub trait AiSettingsPort: Send + Sync {
    /// 列出不包含凭据的提供方快照。
    fn list_providers(&self) -> Result<Vec<ProviderUiSnapshot>, String>;
    /// 保存提供方配置；快照不携带密钥内容。
    fn save_provider(&self, provider: ProviderUiSnapshot) -> Result<(), String>;
    /// 按标识移除提供方。
    fn remove_provider(&self, provider_id: &str) -> Result<(), String>;
    /// 探测指定提供方是否可用。
    fn probe_provider(&self, provider_id: &str) -> Result<(), String>;

    /// 列出技能元数据。
    fn list_skills(&self) -> Result<Vec<SkillUiMeta>, String>;
    /// 预览技能文档并返回元数据，不请求安装。
    fn preview_skill(&self, document: &str) -> Result<SkillUiMeta, String>;
    /// 安装技能文档，以预期摘要核对待安装内容。
    fn install_skill(&self, document: &str, expected_digest: &str) -> Result<SkillUiMeta, String>;

    /// 列出 MCP 连接状态。
    fn list_mcp(&self) -> Result<Vec<McpUiSnapshot>, String>;
    /// 设置指定 MCP 地址的信任状态。
    fn trust_mcp(&self, url: &str, trusted: bool) -> Result<(), String>;
    /// 设置指定 MCP 地址的启用状态。
    fn toggle_mcp(&self, url: &str, enabled: bool) -> Result<(), String>;

    /// 对当前图片输入执行添加、移除或清空操作。
    fn image_action(&self, action: ImageInputAction) -> Result<(), String>;
}
