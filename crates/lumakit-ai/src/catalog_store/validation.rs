//! Catalog metadata invariants shared by reads, saves and legacy import.
use crate::catalog::{ProviderEntry, validate_url};
use lumakit_support::secrets::normalize_key_label;
use std::collections::HashSet;

pub(super) fn validate_catalog(providers: &[ProviderEntry]) -> Result<(), String> {
    let mut ids = HashSet::new();
    if providers.len() > 128 {
        return Err("最多保存 128 个服务商".into());
    }
    for p in providers {
        if p.id.is_empty()
            || p.id.len() > 128
            || !ids.insert(p.id.clone())
            || !valid_name(&p.name)
            || p.endpoints.len() > 64
        {
            return Err("服务商名称应为 1–128 字符且不含控制字符；每个服务商最多 64 条线路，配置标识必须唯一".into());
        }
        for e in &p.endpoints {
            if e.id.is_empty()
                || e.id.len() > 128
                || !ids.insert(e.id.clone())
                || !valid_name(&e.name)
                || e.models.len() > 10_000
            {
                return Err("线路名称应为 1–128 字符且不含控制字符；每条线路最多 10000 个模型，配置标识必须唯一".into());
            }
            validate_url(&e.base_url)?;
            if e.keys.len() > 16 {
                return Err("每条线路最多保存 16 个密钥".into());
            }
            let mut key_ids = HashSet::new();
            for k in &e.keys {
                if k.id.is_empty()
                    || k.id.len() > 128
                    || !ids.insert(k.id.clone())
                    || !key_ids.insert(&k.id)
                    || k.ciphertext.is_empty()
                {
                    return Err("密钥元数据无效；原配置未覆盖".into());
                }
                normalize_key_label(k.label.as_deref().unwrap_or_default())?;
            }
            if e.selected_key
                .as_ref()
                .is_some_and(|id| !key_ids.contains(id))
            {
                return Err("首选密钥不存在".into());
            }
            let mut model_ids = HashSet::new();
            for m in &e.models {
                if m.id.trim().is_empty()
                    || m.id.len() > 512
                    || m.id != m.id.trim()
                    || m.id.chars().any(char::is_control)
                    || !model_ids.insert(&m.id)
                {
                    return Err("模型 ID 无效或重复".into());
                }
                m.settings.validate(e.protocol)?;
            }
            if e.selected_model
                .as_ref()
                .is_some_and(|id| !model_ids.contains(id))
            {
                return Err("首选模型不存在".into());
            }
        }
    }
    Ok(())
}
fn valid_name(s: &str) -> bool {
    !s.trim().is_empty() && s.chars().count() <= 128 && !s.chars().any(char::is_control)
}
