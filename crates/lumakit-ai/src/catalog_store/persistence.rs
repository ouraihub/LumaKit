//! Transactional catalog and key writes.
use super::{CatalogStore, placeholders, sql, validation::validate_catalog};
use crate::{Protocol, catalog::ProviderEntry};
use lumakit_support::secrets::{SavedKey, normalize_key_label};
use rusqlite::{OptionalExtension, Transaction, params};

impl CatalogStore {
    /// Saves only changed providers and removes deleted providers in one transaction.
    ///
    /// # Errors
    /// Returns an error for invalid catalog data, failure to load existing data,
    /// or a database transaction failure; changes are committed together.
    pub fn save(
        &mut self,
        providers: &[ProviderEntry],
        presets_initialized: bool,
    ) -> Result<(), String> {
        validate_catalog(providers)?;
        let previous = self.load()?.0;
        let tx = self.conn.transaction().map_err(sql)?;
        let ids: Vec<&str> = providers.iter().map(|p| p.id.as_str()).collect();
        if ids.is_empty() {
            tx.execute("DELETE FROM providers", []).map_err(sql)?;
        } else {
            tx.execute(
                &format!(
                    "DELETE FROM providers WHERE id NOT IN ({})",
                    placeholders(ids.len())
                ),
                rusqlite::params_from_iter(ids.iter().copied()),
            )
            .map_err(sql)?;
        }
        for (position, provider) in providers.iter().enumerate() {
            let old = previous
                .iter()
                .position(|p| p.id == provider.id)
                .and_then(|i| previous.get(i));
            let changed = old.is_none_or(|p| {
                serde_json::to_value(p).ok() != serde_json::to_value(provider).ok()
                    || previous.iter().position(|x| x.id == p.id) != Some(position)
            });
            if changed {
                upsert_provider(&tx, provider, position)?;
            }
        }
        tx.execute("INSERT INTO store_meta(key,value) VALUES('presets_initialized',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [if presets_initialized { "1" } else { "0" }]).map_err(sql)?;
        tx.commit().map_err(sql)
    }

    /// Saves only an endpoint's keys and selection; endpoint draft fields are untouched.
    ///
    /// # Errors
    /// Returns an error for invalid key metadata/selection, a missing endpoint,
    /// or a database transaction failure.
    pub fn save_keys(
        &mut self,
        endpoint_id: &str,
        keys: &[SavedKey],
        selected: Option<&str>,
    ) -> Result<(), String> {
        if endpoint_id.is_empty()
            || keys.len() > 16
            || selected.is_some_and(|id| !keys.iter().any(|key| key.id == id))
        {
            return Err("密钥配置无效".into());
        }
        for key in keys {
            if key.id.is_empty() || key.id.len() > 128 || key.ciphertext.is_empty() {
                return Err("密钥元数据无效；原配置未覆盖".into());
            }
            normalize_key_label(key.label.as_deref().unwrap_or_default())?;
        }
        let tx = self.conn.transaction().map_err(sql)?;
        if tx
            .query_row("SELECT 1 FROM endpoints WHERE id=?1", [endpoint_id], |_| {
                Ok(())
            })
            .optional()
            .map_err(sql)?
            .is_none()
        {
            return Err("线路已不存在".into());
        }
        let ids: Vec<&str> = keys.iter().map(|key| key.id.as_str()).collect();
        if ids.is_empty() {
            tx.execute(
                "DELETE FROM endpoint_keys WHERE endpoint_id=?1",
                [endpoint_id],
            )
            .map_err(sql)?;
        } else {
            tx.execute(
                &format!(
                    "DELETE FROM endpoint_keys WHERE endpoint_id=?1 AND id NOT IN ({})",
                    placeholders(ids.len())
                ),
                rusqlite::params_from_iter(std::iter::once(endpoint_id).chain(ids.iter().copied())),
            )
            .map_err(sql)?;
        }
        for (position, key) in keys.iter().enumerate() {
            upsert_key(&tx, endpoint_id, key, position)?;
        }
        tx.execute(
            "UPDATE endpoints SET selected_key_id=?1 WHERE id=?2",
            params![selected, endpoint_id],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)
    }

    /// Updates only one provider's enabled state, leaving all other drafts untouched.
    ///
    /// # Errors
    /// Returns an error if the provider no longer exists or the database update fails.
    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), String> {
        let changed = self
            .conn
            .execute(
                "UPDATE providers SET enabled=?1 WHERE id=?2",
                params![enabled as i64, id],
            )
            .map_err(sql)?;
        if changed == 0 {
            Err("服务商已不存在".into())
        } else {
            Ok(())
        }
    }
}
pub(super) fn upsert_snapshot(
    tx: &Transaction<'_>,
    providers: &[ProviderEntry],
) -> Result<(), String> {
    let keep_p: Vec<&str> = providers.iter().map(|p| p.id.as_str()).collect();
    if keep_p.is_empty() {
        tx.execute("DELETE FROM providers", []).map_err(sql)?;
    } else {
        tx.execute(
            &format!(
                "DELETE FROM providers WHERE id NOT IN ({})",
                placeholders(keep_p.len())
            ),
            rusqlite::params_from_iter(keep_p),
        )
        .map_err(sql)?;
    }
    for (pi, p) in providers.iter().enumerate() {
        upsert_provider(tx, p, pi)?;
    }
    Ok(())
}

fn upsert_provider(tx: &Transaction<'_>, p: &ProviderEntry, pi: usize) -> Result<(), String> {
    tx.execute("INSERT INTO providers(id,name,enabled,position) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET name=excluded.name,enabled=excluded.enabled,position=excluded.position", params![p.id,p.name,p.enabled as i64,pi as i64]).map_err(sql)?;
    let endpoint_ids: Vec<&str> = p.endpoints.iter().map(|e| e.id.as_str()).collect();
    if endpoint_ids.is_empty() {
        tx.execute("DELETE FROM endpoints WHERE provider_id=?1", [&p.id])
            .map_err(sql)?;
    } else {
        tx.execute(
            &format!(
                "DELETE FROM endpoints WHERE provider_id=?1 AND id NOT IN ({})",
                placeholders(endpoint_ids.len())
            ),
            rusqlite::params_from_iter(
                std::iter::once(p.id.as_str()).chain(endpoint_ids.iter().copied()),
            ),
        )
        .map_err(sql)?;
    }
    for (ei, e) in p.endpoints.iter().enumerate() {
        tx.execute("INSERT INTO endpoints(id,provider_id,name,base_url,protocol,position,selected_key_id,selected_model_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(id) DO UPDATE SET provider_id=excluded.provider_id,name=excluded.name,base_url=excluded.base_url,protocol=excluded.protocol,position=excluded.position,selected_key_id=excluded.selected_key_id,selected_model_id=excluded.selected_model_id", params![e.id,p.id,e.name,e.base_url,protocol_name(e.protocol),ei as i64,e.selected_key,e.selected_model]).map_err(sql)?;
        let key_ids: Vec<&str> = e.keys.iter().map(|k| k.id.as_str()).collect();
        if key_ids.is_empty() {
            tx.execute("DELETE FROM endpoint_keys WHERE endpoint_id=?1", [&e.id])
                .map_err(sql)?;
        } else {
            tx.execute(
                &format!(
                    "DELETE FROM endpoint_keys WHERE endpoint_id=?1 AND id NOT IN ({})",
                    placeholders(key_ids.len())
                ),
                rusqlite::params_from_iter(
                    std::iter::once(e.id.as_str()).chain(key_ids.iter().copied()),
                ),
            )
            .map_err(sql)?;
        }
        for (ki, k) in e.keys.iter().enumerate() {
            upsert_key(tx, &e.id, k, ki)?;
        }
        let model_ids: Vec<&str> = e.models.iter().map(|m| m.id.as_str()).collect();
        if model_ids.is_empty() {
            tx.execute("DELETE FROM models WHERE endpoint_id=?1", [&e.id])
                .map_err(sql)?;
        } else {
            tx.execute(
                &format!(
                    "DELETE FROM models WHERE endpoint_id=?1 AND id NOT IN ({})",
                    placeholders(model_ids.len())
                ),
                rusqlite::params_from_iter(
                    std::iter::once(e.id.as_str()).chain(model_ids.iter().copied()),
                ),
            )
            .map_err(sql)?;
        }
        for (mi, m) in e.models.iter().enumerate() {
            let json =
                serde_json::to_string(m).map_err(|e| format!("模型元数据序列化失败: {e}"))?;
            tx.execute("INSERT INTO models(endpoint_id,id,streaming,tools,vision,manual,position,value_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(endpoint_id,id) DO UPDATE SET streaming=excluded.streaming,tools=excluded.tools,vision=excluded.vision,manual=excluded.manual,position=excluded.position,value_json=excluded.value_json", params![e.id,m.id,m.capabilities.streaming as i64,m.capabilities.tools as i64,m.capabilities.vision as i64,m.manual as i64,mi as i64,json]).map_err(sql)?;
        }
    }
    Ok(())
}

fn upsert_key(
    tx: &Transaction<'_>,
    endpoint_id: &str,
    key: &SavedKey,
    position: usize,
) -> Result<(), String> {
    let label = normalize_key_label(key.label.as_deref().unwrap_or_default())?;
    tx.execute(
        "INSERT INTO endpoint_keys(id,endpoint_id,masked,ciphertext,position,label,enabled) VALUES(?1,?2,?3,?4,?5,?6,?7) \
         ON CONFLICT(id) DO UPDATE SET endpoint_id=excluded.endpoint_id,masked=excluded.masked,\
         ciphertext=excluded.ciphertext,position=excluded.position,label=excluded.label,enabled=excluded.enabled",
        params![key.id, endpoint_id, key.masked, key.ciphertext, position as i64, label, key.enabled as i64],
    )
    .map_err(sql)?;
    Ok(())
}

fn protocol_name(p: Protocol) -> &'static str {
    match p {
        Protocol::OpenAiChat => "openai_chat",
        Protocol::OpenAiResponses => "openai_responses",
        Protocol::AnthropicMessages => "anthropic_messages",
    }
}
