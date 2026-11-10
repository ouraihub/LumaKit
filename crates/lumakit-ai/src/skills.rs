//! Restricted, permission-free SKILL.md storage.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};
use thiserror::Error;

/// SKILL.md 文档允许的最大字节数（64 KiB）。
pub const MAX_DOCUMENT_BYTES: usize = 64 * 1024;
/// 许可证内容的最大字节数（32 KiB）。
pub const MAX_LICENSE_BYTES: usize = 32 * 1024;
/// 技能名称允许的最大字节数。
pub const MAX_NAME_BYTES: usize = 64;
/// 技能描述允许的最大字节数。
pub const MAX_DESCRIPTION_BYTES: usize = 1024;
/// 技能版本字符串允许的最大字节数。
pub const MAX_VERSION_BYTES: usize = 64;

/// 技能解析、安装、快照或存储过程中发生的错误。
#[derive(Debug, Error)]
pub enum SkillError {
    /// 文档格式、字段或内容不符合限制。
    #[error("invalid SKILL.md: {0}")]
    Invalid(String),
    /// 存储中不存在指定标识的技能。
    #[error("skill not found: {0}")]
    NotFound(String),
    /// 确认摘要与预览摘要或技能摘要不一致。
    #[error("digest confirmation failed")]
    DigestMismatch,
    /// 快照请求的技能修订号与当前存储不一致。
    #[error("skill revision mismatch for {id}: expected {expected}, found {actual}")]
    RevisionMismatch {
        /// 修订号不匹配的技能标识。
        id: String,
        /// 引用所要求的修订号。
        expected: u64,
        /// 存储中技能的实际修订号。
        actual: u64,
    },
    /// 技能正文的总字节数超过预算。
    #[error("skill budget exceeded: {used} > {limit} bytes")]
    BudgetExceeded {
        /// 技能正文实际占用的总字节数。
        used: usize,
        /// 允许的最大正文总字节数。
        limit: usize,
    },
    /// 读取或保存技能存储时发生 I/O 错误。
    #[error("skill store I/O failed: {0}")]
    Io(#[from] io::Error),
    /// 技能存储的 JSON 序列化或反序列化失败。
    #[error("skill store JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}

/// 包含元数据、正文、来源及安装修订号的技能。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Skill {
    /// 技能标识；解析文档时取自名称。
    pub id: String,
    /// 由 ASCII 字母、数字、点、连字符或下划线组成的名称。
    pub name: String,
    /// 技能用途的简短描述。
    pub description: String,
    /// 文档声明的版本字符串。
    pub version: String,
    /// 去除前置元数据及正文开头换行后的正文。
    pub body: String,
    /// 可选的许可证内容；解析文档时默认为无。
    #[serde(default)]
    pub license: Option<String>,
    /// 安装时分配的存储修订号；新解析的技能为零。
    pub revision: u64,
    /// 原始文档字节的内容摘要，用于安装确认。
    pub digest: String,
    /// 调用方提供的技能来源描述。
    pub origin: String,
}

/// 固定技能标识及修订号的引用，用于构建快照。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillRef {
    /// 引用的技能标识。
    pub id: String,
    /// 引用要求的技能修订号。
    pub revision: u64,
}

/// 按引用顺序复制并通过正文预算检查的技能快照。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillSnapshot {
    /// 创建快照时存储的整体修订号。
    pub store_revision: u64,
    /// 按请求引用顺序排列的技能副本。
    pub skills: Vec<Skill>,
    /// 快照中所有技能正文的总字节数。
    pub total_body_bytes: usize,
}

/// 安装前解析得到的技能及待确认摘要。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillPreview {
    /// 待安装的技能；已存在时沿用当前修订号。
    pub skill: Skill,
    /// 需要调用方确认的文档摘要。
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SkillState {
    revision: u64,
    skills: BTreeMap<String, Skill>,
}

/// 以 JSON 文件持久化技能及整体修订号的存储。
#[derive(Debug)]
pub struct SkillStore {
    path: PathBuf,
    state: SkillState,
}

/// Parse a restricted SKILL.md document. Only scalar name/description/version
/// frontmatter keys are accepted; no YAML implementation is intentionally used.
///
/// # Errors
/// Returns an error for an oversized document, invalid/unsupported frontmatter,
/// missing required metadata, or invalid metadata/body content.
pub fn parse_skill(document: &str, origin: impl Into<String>) -> Result<Skill, SkillError> {
    if document.len() > MAX_DOCUMENT_BYTES {
        return Err(SkillError::Invalid("document exceeds 64 KiB".into()));
    }
    let opening = document
        .find('\n')
        .filter(|&i| document[..i].trim_end() == "---")
        .ok_or_else(|| SkillError::Invalid("frontmatter must start with ---".into()))?;
    let mut fields = BTreeMap::<String, String>::new();
    let mut found_end = false;
    let mut body_start = 0;
    let mut position = opening + 1;
    while position < document.len() {
        let line_end = document[position..]
            .find('\n')
            .map(|i| position + i + 1)
            .unwrap_or(document.len());
        let raw = document[position..line_end].trim_end_matches(['\r', '\n']);
        if raw == "---" {
            found_end = true;
            body_start = line_end;
            break;
        }
        let Some((key, value)) = raw.split_once(':') else {
            return Err(SkillError::Invalid(
                "frontmatter must contain scalar key: value lines".into(),
            ));
        };
        let key = key.trim();
        if !matches!(key, "name" | "description" | "version") {
            return Err(SkillError::Invalid(format!(
                "unsupported frontmatter key: {key}"
            )));
        }
        if fields.contains_key(key) {
            return Err(SkillError::Invalid(format!(
                "duplicate frontmatter key: {key}"
            )));
        }
        let value = value.trim();
        if value.is_empty()
            || matches!(
                value.as_bytes().first(),
                Some(b'[' | b'{' | b'&' | b'*' | b'!')
            )
        {
            return Err(SkillError::Invalid(format!(
                "frontmatter value for {key} is not a scalar"
            )));
        }
        let value = unquote(value)
            .ok_or_else(|| SkillError::Invalid(format!("invalid value for {key}")))?;
        if value.contains("http://")
            || value.contains("https://")
            || value.contains("${")
            || looks_secret(&value)
        {
            return Err(SkillError::Invalid(format!("unsafe value for {key}")));
        }
        fields.insert(key.to_owned(), value);
        position = line_end;
    }
    if !found_end {
        return Err(SkillError::Invalid(
            "frontmatter closing --- is missing".into(),
        ));
    }
    let name = fields
        .remove("name")
        .ok_or_else(|| SkillError::Invalid("name is required".into()))?;
    let description = fields
        .remove("description")
        .ok_or_else(|| SkillError::Invalid("description is required".into()))?;
    let version = fields
        .remove("version")
        .ok_or_else(|| SkillError::Invalid("version is required".into()))?;
    validate_name(&name)?;
    if description.len() > MAX_DESCRIPTION_BYTES || version.len() > MAX_VERSION_BYTES {
        return Err(SkillError::Invalid(
            "description or version is too long".into(),
        ));
    }
    let body = document[body_start..]
        .trim_start_matches(['\r', '\n'])
        .to_owned();
    let digest = digest(document.as_bytes());
    Ok(Skill {
        id: name.clone(),
        name,
        description,
        version,
        body,
        license: None,
        revision: 0,
        digest,
        origin: origin.into(),
    })
}

fn unquote(value: &str) -> Option<String> {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        Some(value[1..value.len() - 1].replace("\\\"", "\""))
    } else if value.bytes().any(|b| b.is_ascii_control()) {
        None
    } else {
        Some(value.to_owned())
    }
}

fn looks_secret(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    ["api_key", "apikey", "secret", "password", "token", "bearer"]
        .iter()
        .any(|x| lower.contains(x))
}

fn validate_name(name: &str) -> Result<(), SkillError> {
    if name.is_empty()
        || name.len() > MAX_NAME_BYTES
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
    {
        return Err(SkillError::Invalid(
            "name must be 1-64 ASCII letters, digits, '.', '-' or '_'".into(),
        ));
    }
    Ok(())
}

/// Stable, dependency-free content digest used for preview/install confirmation.
pub fn digest(bytes: &[u8]) -> String {
    let mut a = 0xcbf29ce484222325u64;
    let mut b = 0x9e3779b185ebca87u64;
    for &byte in bytes {
        a ^= byte as u64;
        a = a.wrapping_mul(0x100000001b3);
        b ^= (byte as u64).wrapping_add(a.rotate_left(13));
        b = b.rotate_left(17).wrapping_mul(0x100000001b3);
    }
    format!("{a:016x}{b:016x}")
}

impl SkillStore {
    /// 打开并读取 JSON 存储；文件不存在时使用空存储，不立即写入文件。
    ///
    /// # Errors
    /// Returns an error if an existing store cannot be read or decoded as JSON.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SkillError> {
        let path = path.as_ref().to_owned();
        let state = if path.exists() {
            serde_json::from_slice(&fs::read(&path)?)?
        } else {
            SkillState::default()
        };
        Ok(Self { path, state })
    }

    /// 解析文档并生成安装预览，不修改存储；已有技能保留当前修订号。
    ///
    /// # Errors
    /// Returns the document validation errors described by [`parse_skill`].
    pub fn preview(
        &self,
        document: &str,
        origin: impl Into<String>,
    ) -> Result<SkillPreview, SkillError> {
        let mut skill = parse_skill(document, origin)?;
        if let Some(old) = self.state.skills.get(&skill.id) {
            skill.revision = old.revision;
        }
        let digest = skill.digest.clone();
        Ok(SkillPreview { skill, digest })
    }

    /// 解析文档，核对确认摘要后安装或替换同标识技能并保存。
    ///
    /// # Errors
    /// Returns an error if parsing fails, the confirmed digest differs, or saving fails.
    pub fn install(
        &mut self,
        document: &str,
        origin: impl Into<String>,
        confirmed_digest: &str,
    ) -> Result<SkillRef, SkillError> {
        let preview = self.preview(document, origin)?;
        self.install_preview(&preview, confirmed_digest)
    }

    /// 核对预览及技能摘要，分配新的整体修订号并保存技能。
    ///
    /// 此方法不重新解析预览；保存失败时内存中的修改不会回滚。
    ///
    /// # Errors
    /// Returns an error for a digest mismatch or a directory, serialization, write, or rename failure.
    pub fn install_preview(
        &mut self,
        preview: &SkillPreview,
        confirmed_digest: &str,
    ) -> Result<SkillRef, SkillError> {
        if preview.digest != confirmed_digest || preview.skill.digest != confirmed_digest {
            return Err(SkillError::DigestMismatch);
        }
        let mut skill = preview.skill.clone();
        self.state.revision = self.state.revision.saturating_add(1);
        skill.revision = self.state.revision;
        let id = skill.id.clone();
        let revision = skill.revision;
        self.state.skills.insert(id.clone(), skill);
        self.save()?;
        Ok(SkillRef { id, revision })
    }

    /// 按标识借用当前技能，不存在时返回 `None`。
    pub fn get(&self, id: &str) -> Option<&Skill> {
        self.state.skills.get(id)
    }
    /// 返回存储当前的整体修订号。
    pub fn revision(&self) -> u64 {
        self.state.revision
    }

    /// 按引用顺序复制技能，并检查每个修订号及正文总字节预算。
    ///
    /// # Errors
    /// 技能缺失、修订号不匹配或正文总字节数超过 `budget` 时返回错误。
    pub fn snapshot(&self, refs: &[SkillRef], budget: usize) -> Result<SkillSnapshot, SkillError> {
        let mut skills = Vec::with_capacity(refs.len());
        for reference in refs {
            let skill = self
                .state
                .skills
                .get(&reference.id)
                .ok_or_else(|| SkillError::NotFound(reference.id.clone()))?;
            if skill.revision != reference.revision {
                return Err(SkillError::RevisionMismatch {
                    id: reference.id.clone(),
                    expected: reference.revision,
                    actual: skill.revision,
                });
            }
            skills.push(skill.clone());
        }
        let total_body_bytes = check_budget(&skills, budget)?;
        Ok(SkillSnapshot {
            store_revision: self.state.revision,
            skills,
            total_body_bytes,
        })
    }

    fn save(&self) -> Result<(), SkillError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_vec_pretty(&self.state)?;
        let temp = self.path.with_extension("tmp");
        fs::write(&temp, data)?;
        // ponytail: std has no cross-platform atomic replace; Windows needs the destination removed first.
        if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        fs::rename(temp, &self.path)?;
        Ok(())
    }
}

/// 累加技能正文的字节数，未超过 `limit` 时返回总数，否则返回预算错误。
///
/// # Errors
/// Returns [`SkillError::BudgetExceeded`] if the combined body size exceeds `limit`.
pub fn check_budget(skills: &[Skill], limit: usize) -> Result<usize, SkillError> {
    let used = skills.iter().map(|skill| skill.body.len()).sum();
    if used > limit {
        Err(SkillError::BudgetExceeded { used, limit })
    } else {
        Ok(used)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(body: &str) -> String {
        format!("---\nname: demo\ndescription: test\nversion: 1\n---\n{body}")
    }

    #[test]
    fn changed_digest_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join(format!("lumakit-skills-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        let mut store = SkillStore::open(&path)?;
        let preview = store.preview(&doc("one"), "test")?;
        assert!(matches!(
            store.install(&doc("two"), "test", &preview.digest),
            Err(SkillError::DigestMismatch)
        ));
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn revision_mismatch_rejects_snapshot() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join(format!(
            "lumakit-skills-revision-{}.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let mut store = SkillStore::open(&path)?;
        let p = store.preview(&doc("one"), "test")?;
        let r = store.install_preview(&p, &p.digest)?;
        let stale = SkillRef {
            id: r.id.clone(),
            revision: r.revision - 1,
        };
        assert!(matches!(
            store.snapshot(&[stale], 100),
            Err(SkillError::RevisionMismatch { .. })
        ));
        let _ = fs::remove_file(path);
        Ok(())
    }
}
