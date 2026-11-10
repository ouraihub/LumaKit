use crate::sqlite::{open_secret, seal_secret};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use zeroize::Zeroizing;

/// 用系统随机源生成 16 字节随机值，编码为 32 位小写十六进制标识。
///
/// # Errors
/// 系统随机源或标识编码失败时返回可展示的错误信息。
pub fn new_id() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "无法生成配置标识，请重试".to_owned())?;
    let mut id = String::with_capacity(32);
    for byte in bytes {
        write!(&mut id, "{byte:02x}").map_err(|_| "无法生成配置标识".to_owned())?;
    }
    Ok(id)
}

/// 可序列化的加密凭据；不存储明文，解密依赖当前平台与用户环境。
#[derive(Clone, Serialize, Deserialize)]
pub struct SavedKey {
    /// 稳定凭据标识；替换密钥不会改变它。
    pub id: String,
    /// 可选显示标签；旧配置未提供时不带标签。
    #[serde(default)]
    pub label: Option<String>,
    /// 是否允许该凭据参与请求、检测和轮换；旧配置默认为启用。
    #[serde(default = "default_key_enabled")]
    pub enabled: bool,
    /// 用于界面展示的固定遮罩，不反映密钥长度。
    pub masked: String,
    /// 系统保护后的密文字节；不能作为明文 API key 使用。
    pub ciphertext: Vec<u8>,
}

fn default_key_enabled() -> bool {
    true
}

impl SavedKey {
    /// 生成标识，校验并加密密钥；成功后才返回完整凭据。
    ///
    /// # Errors
    /// 随机标识生成、密钥校验或系统加密失败时返回错误。
    pub fn new(value: &str) -> Result<Self, String> {
        let mut key = Self {
            id: new_id()?,
            label: None,
            enabled: true,
            masked: String::new(),
            ciphertext: Vec::new(),
        };
        key.replace(value)?;
        Ok(key)
    }

    /// 校验并保存去除首尾空白的标签；空白标签清除为 `None`。
    ///
    /// # Errors
    /// 标签超过 128 个 Unicode 字符或含控制字符时保留原值并返回错误。
    pub fn set_label(&mut self, value: &str) -> Result<(), String> {
        self.label = normalize_key_label(value)?.map(str::to_owned);
        Ok(())
    }

    /// 去除首尾空白后替换密钥；校验或加密失败保留原密文和标识。
    ///
    /// # Errors
    /// 空值、超过 4096 字节、含控制字符或系统加密失败时返回错误。
    pub fn replace(&mut self, value: &str) -> Result<(), String> {
        let value = value.trim();
        if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
            return Err("密钥不能为空，不能含控制字符，且不能超过 4096 字节".into());
        }
        // 先加密再替换：失败时原凭据仍然可用，不能留下空密钥。
        let ciphertext = seal_secret(value.as_bytes())
            .map_err(|_| "系统无法加密密钥，原凭据未修改".to_owned())?;
        self.ciphertext = ciphertext;
        self.masked = "••••••••".into();
        Ok(())
    }

    /// 显式解密为释放时清零的字符串；不要将返回值写入日志或持久化。
    ///
    /// # Errors
    /// 平台不支持、用户环境不匹配、密文损坏或解密后非 UTF-8 时返回错误。
    pub fn open(&self) -> Result<Zeroizing<String>, String> {
        let bytes = Zeroizing::new(
            open_secret(&self.ciphertext)
                .map_err(|_| "无法解密此密钥，请在原 Windows 用户下运行或重新填写".to_owned())?,
        );
        let text = std::str::from_utf8(&bytes).map_err(|_| "已保存密钥格式无效".to_owned())?;
        Ok(Zeroizing::new(text.to_owned()))
    }
}

/// 校验显示标签并借用其去除首尾空白后的内容；空白标签返回 `None`。
///
/// # Errors
/// 去除首尾空白后的标签超过 128 个 Unicode 字符或含控制字符时返回错误。
pub fn normalize_key_label(value: &str) -> Result<Option<&str>, String> {
    let value = value.trim();
    if value
        .chars()
        .enumerate()
        .any(|(index, ch)| index >= 128 || ch.is_control())
    {
        return Err("密钥标签不能超过 128 个字符，且不能含控制字符".into());
    }
    Ok((!value.is_empty()).then_some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn encrypted_key_round_trip_and_failed_replacement_preserve_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut key = SavedKey::new("  测试-key-123456789  ")?;
        assert!(key.label.is_none());
        key.set_label("  工作账号  ")?;
        let id = key.id.clone();
        let encoded = serde_json::to_string(&key)?;
        assert!(!encoded.contains("测试-key"));
        let restored: SavedKey = serde_json::from_str(&encoded)?;
        assert_eq!(restored.open()?.as_str(), "测试-key-123456789");
        assert!(key.replace(" ").is_err());
        assert_eq!(key.id, id);
        assert_eq!(key.label.as_deref(), Some("工作账号"));
        assert_eq!(key.open()?.as_str(), "测试-key-123456789");
        key.replace("replacement-key")?;
        assert_eq!(key.id, id);
        assert_eq!(key.label.as_deref(), Some("工作账号"));
        assert_eq!(key.open()?.as_str(), "replacement-key");
        let mut damaged = key.clone();
        damaged.ciphertext[0] ^= 1;
        assert!(damaged.open().is_err());
        Ok(())
    }

    #[test]
    fn old_json_and_label_edits_preserve_secret_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let mut key: SavedKey =
            serde_json::from_str(r#"{"id":"old-key","masked":"••••••••","ciphertext":[0,1,255]}"#)?;
        assert!(key.label.is_none());
        let ciphertext = key.ciphertext.clone();
        key.set_label("\n  工作账号  \t")?;
        assert_eq!(key.label.as_deref(), Some("工作账号"));
        assert!(key.set_label("a\0b").is_err());
        assert_eq!(key.label.as_deref(), Some("工作账号"));
        assert!(key.set_label(&"标".repeat(129)).is_err());
        assert_eq!(key.label.as_deref(), Some("工作账号"));
        key.set_label(&"标".repeat(128))?;
        assert_eq!(
            key.label.as_ref().map(|label| label.chars().count()),
            Some(128)
        );
        key.set_label(" \n\t ")?;
        assert!(key.label.is_none());
        assert_eq!(key.id, "old-key");
        assert_eq!(key.ciphertext, ciphertext);
        Ok(())
    }

    #[test]
    fn old_json_defaults_enabled_and_round_trip_preserves_disabled_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let old: SavedKey =
            serde_json::from_str(r#"{"id":"old-key","masked":"••••••••","ciphertext":[0,1,255]}"#)?;
        assert!(old.enabled);
        let mut disabled = old.clone();
        disabled.enabled = false;
        let restored: SavedKey = serde_json::from_str(&serde_json::to_string(&disabled)?)?;
        assert!(!restored.enabled);
        assert_eq!(restored.id, old.id);
        assert_eq!(restored.label, old.label);
        assert_eq!(restored.masked, old.masked);
        assert_eq!(restored.ciphertext, old.ciphertext);
        Ok(())
    }
}
