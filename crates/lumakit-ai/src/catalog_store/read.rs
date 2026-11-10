//! Reconstructs and validates the persisted catalog without opening secrets.
use super::{CatalogStore, sql, validation::validate_catalog};
use crate::{
    Protocol,
    catalog::{EndpointEntry, ModelEntry, ProviderEntry},
};
use lumakit_support::secrets::{SavedKey, normalize_key_label};
use rusqlite::OptionalExtension;

impl CatalogStore {
    /// Loads the saved catalog in stable UI order and the preset initialization flag.
    ///
    /// # Errors
    /// Returns an error for an invalid initialization marker, database read failures,
    /// or malformed/invalid saved provider, endpoint, key, or model metadata.
    pub fn load(&self) -> Result<(Vec<ProviderEntry>, bool), String> {
        self.require_initialized()?;
        let mut providers = Vec::new();
        let mut stmt = self
            .conn
            .prepare("SELECT id,name,enabled FROM providers ORDER BY position,id")
            .map_err(sql)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)? != 0,
                ))
            })
            .map_err(sql)?;
        for row in rows {
            let (id, name, enabled) = row.map_err(sql)?;
            let mut provider = ProviderEntry {
                id: id.clone(),
                name,
                enabled,
                endpoints: Vec::new(),
            };
            let mut es = self.conn.prepare("SELECT id,name,base_url,protocol,selected_key_id,selected_model_id FROM endpoints WHERE provider_id=?1 ORDER BY position,id").map_err(sql)?;
            let endpoints = es
                .query_map([&id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                })
                .map_err(sql)?;
            for endpoint in endpoints {
                let (eid, ename, base_url, protocol, selected_key, selected_model): (
                    String,
                    String,
                    String,
                    String,
                    Option<String>,
                    Option<String>,
                ) = endpoint.map_err(sql)?;
                let protocol = parse_protocol(&protocol)?;
                let mut keys = Vec::new();
                let mut ks = self.conn.prepare("SELECT id,masked,ciphertext,label,enabled FROM endpoint_keys WHERE endpoint_id=?1 ORDER BY position,id").map_err(sql)?;
                for key in ks
                    .query_map([&eid], |r| {
                        Ok(SavedKey {
                            id: r.get(0)?,
                            masked: r.get(1)?,
                            ciphertext: r.get(2)?,
                            label: r.get(3)?,
                            enabled: r.get::<_, i64>(4)? != 0,
                        })
                    })
                    .map_err(sql)?
                {
                    let mut key = key.map_err(sql)?;
                    if let Some(label) = key.label.as_deref() {
                        let normalized = normalize_key_label(label)?;
                        if normalized != Some(label) {
                            key.label = normalized.map(str::to_owned);
                        }
                    }
                    keys.push(key);
                }
                let mut models = Vec::new();
                let mut ms = self.conn.prepare("SELECT id,streaming,tools,vision,manual,value_json FROM models WHERE endpoint_id=?1 ORDER BY position,id").map_err(sql)?;
                for model in ms
                    .query_map([&eid], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, i64>(1)? != 0,
                            r.get::<_, i64>(2)? != 0,
                            r.get::<_, i64>(3)? != 0,
                            r.get::<_, i64>(4)? != 0,
                            r.get::<_, String>(5)?,
                        ))
                    })
                    .map_err(sql)?
                {
                    let (id, streaming, tools, vision, manual, json) = model.map_err(sql)?;
                    let mut value: ModelEntry =
                        serde_json::from_str(&json).map_err(|e| format!("模型元数据损坏: {e}"))?;
                    value.id = id;
                    value.capabilities = crate::providers::ModelCapabilities {
                        streaming,
                        tools,
                        vision,
                    };
                    value.manual = manual;
                    value.settings.validate(protocol)?;
                    models.push(value);
                }
                provider.endpoints.push(EndpointEntry {
                    id: eid,
                    name: ename,
                    base_url,
                    protocol,
                    keys,
                    models,
                    selected_key,
                    selected_model,
                });
            }
            providers.push(provider);
        }
        let initialized = self
            .conn
            .query_row(
                "SELECT value FROM store_meta WHERE key='presets_initialized'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(sql)?;
        validate_catalog(&providers)?;
        Ok((providers, initialized.is_some_and(|v| v == "1")))
    }
}
fn parse_protocol(s: &str) -> Result<Protocol, String> {
    match s {
        "openai_chat" => Ok(Protocol::OpenAiChat),
        "openai_responses" => Ok(Protocol::OpenAiResponses),
        "anthropic_messages" => Ok(Protocol::AnthropicMessages),
        _ => Err("数据库中的线路协议无效".into()),
    }
}
