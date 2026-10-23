//! Restricted, permission-free SKILL.md storage.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};
use thiserror::Error;

pub const MAX_DOCUMENT_BYTES: usize = 64 * 1024;
pub const MAX_LICENSE_BYTES: usize = 32 * 1024;
pub const MAX_NAME_BYTES: usize = 64;
pub const MAX_DESCRIPTION_BYTES: usize = 1024;
pub const MAX_VERSION_BYTES: usize = 64;

#[derive(Debug, Error)]
pub enum SkillError {
    #[error("invalid SKILL.md: {0}")]
    Invalid(String),
    #[error("skill not found: {0}")]
    NotFound(String),
    #[error("digest confirmation failed")]
    DigestMismatch,
    #[error("skill revision mismatch for {id}: expected {expected}, found {actual}")]
    RevisionMismatch {
        id: String,
        expected: u64,
        actual: u64,
    },
    #[error("skill budget exceeded: {used} > {limit} bytes")]
    BudgetExceeded { used: usize, limit: usize },
    #[error("skill store I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("skill store JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub body: String,
    #[serde(default)]
    pub license: Option<String>,
    pub revision: u64,
    pub digest: String,
    pub origin: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillRef {
    pub id: String,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillSnapshot {
    pub store_revision: u64,
    pub skills: Vec<Skill>,
    pub total_body_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillPreview {
    pub skill: Skill,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SkillState {
    revision: u64,
    skills: BTreeMap<String, Skill>,
}

#[derive(Debug)]
pub struct SkillStore {
    path: PathBuf,
    state: SkillState,
}

/// Parse a restricted SKILL.md document. Only scalar name/description/version
/// frontmatter keys are accepted; no YAML implementation is intentionally used.
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
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SkillError> {
        let path = path.as_ref().to_owned();
        let state = if path.exists() {
            serde_json::from_slice(&fs::read(&path)?)?
        } else {
            SkillState::default()
        };
        Ok(Self { path, state })
    }

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

    pub fn install(
        &mut self,
        document: &str,
        origin: impl Into<String>,
        confirmed_digest: &str,
    ) -> Result<SkillRef, SkillError> {
        let preview = self.preview(document, origin)?;
        self.install_preview(&preview, confirmed_digest)
    }

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

    pub fn get(&self, id: &str) -> Option<&Skill> {
        self.state.skills.get(id)
    }
    pub fn revision(&self) -> u64 {
        self.state.revision
    }

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
    fn changed_digest_is_rejected() {
        let path = std::env::temp_dir().join(format!("lumakit-skills-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        let mut store = SkillStore::open(&path).unwrap();
        let preview = store.preview(&doc("one"), "test").unwrap();
        assert!(matches!(
            store.install(&doc("two"), "test", &preview.digest),
            Err(SkillError::DigestMismatch)
        ));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn revision_mismatch_rejects_snapshot() {
        let path = std::env::temp_dir().join(format!(
            "lumakit-skills-revision-{}.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let mut store = SkillStore::open(&path).unwrap();
        let p = store.preview(&doc("one"), "test").unwrap();
        let r = store.install_preview(&p, &p.digest).unwrap();
        let stale = SkillRef {
            id: r.id.clone(),
            revision: r.revision - 1,
        };
        assert!(matches!(
            store.snapshot(&[stale], 100),
            Err(SkillError::RevisionMismatch { .. })
        ));
        let _ = fs::remove_file(path);
    }
}
