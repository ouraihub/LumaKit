//! 内置服务商预设：公开、不含密钥的模板，宿主据此播种为停用状态的服务商。
//!
//! 预设只描述「去哪里注册、用哪个地址、走哪种协议」，不含密钥、模型列表和启用状态。
//! 宿主各自的 `Protocol` 类型由应用定义，因此这里用 [`PresetProtocol`] 表达协议，
//! 由调用方映射到自己的枚举。

use serde::{Deserialize, Serialize};

/// 预设端点的传输协议。宿主把它映射到自身的协议枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PresetProtocol {
    /// OpenAI 兼容的 Chat Completions。
    OpenaiChat,
    /// OpenAI Responses。
    OpenaiResponses,
    /// Anthropic Messages。
    AnthropicMessages,
}

/// 一条预设。不含密钥、模型与启用状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderPreset {
    /// 稳定标识，持久化在服务商记录上；不对用户展示。
    pub id: &'static str,
    /// 展示名称。
    pub name: &'static str,
    /// 接口基地址。
    pub base_url: &'static str,
    /// 该地址使用的协议。
    pub protocol: PresetProtocol,
    /// 用户申请密钥的页面；始终为 `https://`。
    pub api_key_url: &'static str,
}

/// 全部内置预设。新增服务商只改这一处，所有宿主同时生效。
pub const PROVIDER_PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "qiniu",
        name: "七牛云",
        base_url: "https://api.qnaigc.com",
        protocol: PresetProtocol::OpenaiChat,
        api_key_url: "https://s.qiniu.com/BfyeU3",
    },
    ProviderPreset {
        id: "siliconflow",
        name: "硅基流动",
        base_url: "https://api.siliconflow.cn/v1",
        protocol: PresetProtocol::OpenaiChat,
        api_key_url: "https://cloud.siliconflow.cn/i/ieP7r6Bx",
    },
    ProviderPreset {
        id: "fenno",
        name: "Fenno",
        base_url: "https://api.fenno.ai/v1",
        protocol: PresetProtocol::OpenaiChat,
        api_key_url: "https://api.fenno.ai/s/vWbM",
    },
];

/// 返回 `id` 对应的预设；该预设已不再内置时返回 `None`。
#[must_use]
pub fn provider_preset(id: &str) -> Option<&'static ProviderPreset> {
    PROVIDER_PRESETS.iter().find(|preset| preset.id == id)
}

#[cfg(test)]
mod tests {
    use super::{PROVIDER_PRESETS, provider_preset};

    #[test]
    fn every_preset_is_addressable_by_its_id() {
        for preset in PROVIDER_PRESETS {
            assert_eq!(provider_preset(preset.id), Some(preset));
        }
        assert_eq!(provider_preset("unknown"), None);
    }

    #[test]
    fn presets_carry_unique_ids_and_https_urls() {
        let mut ids: Vec<&str> = PROVIDER_PRESETS.iter().map(|p| p.id).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "preset ids must be unique");
        for preset in PROVIDER_PRESETS {
            assert!(
                preset.base_url.starts_with("https://"),
                "{} base_url must be https",
                preset.id
            );
            assert!(
                preset.api_key_url.starts_with("https://"),
                "{} api_key_url must be https",
                preset.id
            );
            assert!(!preset.name.is_empty(), "{} needs a name", preset.id);
        }
    }
}
