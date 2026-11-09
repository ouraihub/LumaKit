//! SQLite persistence for the saved provider catalog.
use crate::{
    Protocol,
    catalog::{EndpointEntry, ModelEntry, ProviderEntry, validate_url},
};
use lumakit_support::secrets::SavedKey;
use lumakit_support::{
    load_versioned,
    sqlite::{Migration, open_and_migrate},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

const DB_FILE: &str = "providers.db";
const LEGACY_FILE: &str = "model-settings.json";
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: r#"
CREATE TABLE providers (
    id TEXT PRIMARY KEY CHECK(length(id) BETWEEN 1 AND 128), name TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), position INTEGER NOT NULL CHECK(position >= 0)
);
CREATE TABLE endpoints (
    id TEXT PRIMARY KEY CHECK(length(id) BETWEEN 1 AND 128), provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    name TEXT NOT NULL, base_url TEXT NOT NULL, protocol TEXT NOT NULL CHECK(protocol IN ('openai_chat','openai_responses','anthropic_messages')),
    position INTEGER NOT NULL CHECK(position >= 0), selected_key_id TEXT, selected_model_id TEXT,
    UNIQUE(provider_id,id)
);
CREATE TABLE endpoint_keys (
    id TEXT PRIMARY KEY CHECK(length(id) BETWEEN 1 AND 128), endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE,
    masked TEXT NOT NULL, ciphertext BLOB NOT NULL CHECK(length(ciphertext) > 0), position INTEGER NOT NULL CHECK(position >= 0),
    UNIQUE(endpoint_id,id)
);
CREATE TABLE models (
    endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE, id TEXT NOT NULL CHECK(length(id) BETWEEN 1 AND 512),
    streaming INTEGER NOT NULL CHECK(streaming IN (0,1)), tools INTEGER NOT NULL CHECK(tools IN (0,1)), vision INTEGER NOT NULL CHECK(vision IN (0,1)),
    manual INTEGER NOT NULL CHECK(manual IN (0,1)), position INTEGER NOT NULL CHECK(position >= 0), value_json TEXT NOT NULL CHECK(length(value_json) > 0),
    PRIMARY KEY(endpoint_id,id)
);
CREATE TABLE store_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
INSERT INTO store_meta(key,value) VALUES('store_initialized','1'),('presets_initialized','0'),('legacy_imported','0');
CREATE INDEX endpoints_provider_position ON endpoints(provider_id, position);
CREATE INDEX endpoint_keys_endpoint_position ON endpoint_keys(endpoint_id, position);
CREATE INDEX models_endpoint_position ON models(endpoint_id, position);
"#,
    },
    Migration {
        version: 2,
        sql: r#"
INSERT OR IGNORE INTO store_meta(key,value) VALUES('store_initialized','1'),('presets_initialized','0'),('legacy_imported','0');
"#,
    },
];

#[derive(Clone, Serialize, Deserialize, Default)]
struct LegacyFile {
    #[serde(default)]
    providers: Vec<ProviderEntry>,
    #[serde(default)]
    presets_initialized: bool,
}

/// SQLite-backed saved provider catalog. The legacy JSON file is used only once during import.
pub struct CatalogStore {
    conn: Connection,
    _path: PathBuf,
}

impl CatalogStore {
    /// Opens (and, when needed, migrates) the provider database under `root`.
    pub fn open(root: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(root).map_err(|e| format!("无法创建模型数据目录: {e}"))?;
        let path = root.join(DB_FILE);
        let conn = open_and_migrate(&path, MIGRATIONS).map_err(|e| e.to_string())?;
        let mut store = Self { conn, _path: path };
        store.require_initialized()?;
        store.import_legacy(root)?;
        Ok(store)
    }

    fn require_initialized(&self) -> Result<(), String> {
        let value = self
            .conn
            .query_row(
                "SELECT value FROM store_meta WHERE key='store_initialized'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(sql)?;
        if value.as_deref() != Some("1") {
            return Err("模型数据库初始化标记缺失或无效".into());
        }
        Ok(())
    }

    /// Loads the saved catalog in stable UI order and the preset initialization flag.
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
                let mut ks = self.conn.prepare("SELECT id,masked,ciphertext FROM endpoint_keys WHERE endpoint_id=?1 ORDER BY position,id").map_err(sql)?;
                for key in ks
                    .query_map([&eid], |r| {
                        Ok(SavedKey {
                            id: r.get(0)?,
                            masked: r.get(1)?,
                            ciphertext: r.get(2)?,
                        })
                    })
                    .map_err(sql)?
                {
                    keys.push(key.map_err(sql)?);
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

    /// Saves only changed providers and removes deleted providers in one transaction.
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
            tx.execute("INSERT INTO endpoint_keys(id,endpoint_id,masked,ciphertext,position) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET endpoint_id=excluded.endpoint_id,masked=excluded.masked,ciphertext=excluded.ciphertext,position=excluded.position", params![key.id,endpoint_id,key.masked,key.ciphertext,position as i64]).map_err(sql)?;
        }
        tx.execute(
            "UPDATE endpoints SET selected_key_id=?1 WHERE id=?2",
            params![selected, endpoint_id],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)
    }

    /// Updates only one provider's enabled state, leaving all other drafts untouched.
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

    fn import_legacy(&mut self, root: &Path) -> Result<(), String> {
        if self
            .conn
            .query_row(
                "SELECT value FROM store_meta WHERE key='legacy_imported'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(sql)?
            .as_deref()
            == Some("1")
        {
            return Ok(());
        }
        let path = root.join(LEGACY_FILE);
        let Some((version, legacy)) =
            load_versioned::<LegacyFile>(&path, 1).map_err(|e| e.to_string())?
        else {
            let tx = self.conn.transaction().map_err(sql)?;
            tx.execute(
                "UPDATE store_meta SET value='1' WHERE key='legacy_imported'",
                [],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)?;
            return Ok(());
        };
        if version != 1 {
            return Err(format!("不支持的模型设置版本 {version}"));
        }
        validate_catalog(&legacy.providers)?;
        let tx = self.conn.transaction().map_err(sql)?;
        upsert_snapshot(&tx, &legacy.providers)?;
        tx.execute("INSERT INTO store_meta(key,value) VALUES('presets_initialized',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [if legacy.presets_initialized { "1" } else { "0" }]).map_err(sql)?;
        tx.execute(
            "UPDATE store_meta SET value='1' WHERE key='legacy_imported'",
            [],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)
    }
}

fn upsert_snapshot(tx: &Transaction<'_>, providers: &[ProviderEntry]) -> Result<(), String> {
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
            tx.execute("INSERT INTO endpoint_keys(id,endpoint_id,masked,ciphertext,position) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET endpoint_id=excluded.endpoint_id,masked=excluded.masked,ciphertext=excluded.ciphertext,position=excluded.position", params![k.id,e.id,k.masked,k.ciphertext,ki as i64]).map_err(sql)?;
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

fn validate_catalog(providers: &[ProviderEntry]) -> Result<(), String> {
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
fn protocol_name(p: Protocol) -> &'static str {
    match p {
        Protocol::OpenAiChat => "openai_chat",
        Protocol::OpenAiResponses => "openai_responses",
        Protocol::AnthropicMessages => "anthropic_messages",
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
fn placeholders(n: usize) -> String {
    std::iter::repeat("?").take(n).collect::<Vec<_>>().join(",")
}
fn sql(e: rusqlite::Error) -> String {
    format!("SQLite操作失败: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn root(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("lumakit-catalog-{name}-{stamp}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn provider(id: &str, key: Option<SavedKey>) -> ProviderEntry {
        let selected_key = key.as_ref().map(|k| k.id.clone());
        ProviderEntry {
            id: id.into(),
            name: "Provider".into(),
            enabled: true,
            endpoints: vec![EndpointEntry {
                id: format!("{id}-endpoint"),
                name: "Endpoint".into(),
                base_url: "https://example.com/v1".into(),
                protocol: Protocol::OpenAiChat,
                keys: key.into_iter().collect(),
                models: Vec::new(),
                selected_key,
                selected_model: None,
            }],
        }
    }

    #[test]
    fn empty_database_does_not_resurrect_json_after_marker() {
        let path = root("empty");
        let json = serde_json::json!({"providers": [{"id":"old","name":"Old","enabled":true,"endpoints":[]}]});
        lumakit_support::save_versioned(&path.join(LEGACY_FILE), 1, &json).unwrap();
        let store = CatalogStore::open(&path).unwrap();
        assert_eq!(store.load().unwrap().0.len(), 1);
        drop(store);
        std::fs::remove_file(path.join(LEGACY_FILE)).unwrap();
        let mut store = CatalogStore::open(&path).unwrap();
        store.save(&[], false).unwrap();
        drop(store);
        let json = serde_json::json!({"providers": [{"id":"new-old","name":"Old","enabled":true,"endpoints":[]}]});
        lumakit_support::save_versioned(&path.join(LEGACY_FILE), 1, &json).unwrap();
        let store = CatalogStore::open(&path).unwrap();
        assert!(store.load().unwrap().0.is_empty());
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn import_failure_does_not_write_marker_or_partial_rows() {
        let path = root("bad-import");
        let json = serde_json::json!({"providers": [{"id":"bad","name":"Bad","enabled":true,"endpoints":[{"id":"e","name":"E","base_url":"http://not-loopback","protocol":"OpenAiChat","keys":[],"models":[],"selected_key":null,"selected_model":null}]}]});
        lumakit_support::save_versioned(&path.join(LEGACY_FILE), 1, &json).unwrap();
        assert!(CatalogStore::open(&path).is_err());
        let conn = Connection::open(path.join(DB_FILE)).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM providers", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT value FROM store_meta WHERE key='legacy_imported'",
                [],
                |r| r.get::<_, String>(0)
            )
            .optional()
            .unwrap()
            .as_deref(),
            Some("0")
        );
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn future_version_and_malformed_json_are_rejected() {
        let future = root("future");
        lumakit_support::save_versioned(
            &future.join(LEGACY_FILE),
            2,
            &serde_json::json!({"providers":[]}),
        )
        .unwrap();
        assert!(CatalogStore::open(&future).is_err());
        let malformed = root("malformed");
        std::fs::write(malformed.join(LEGACY_FILE), b"not-json").unwrap();
        assert!(CatalogStore::open(&malformed).is_err());
        let _ = std::fs::remove_dir_all(future);
        let _ = std::fs::remove_dir_all(malformed);
    }

    #[test]
    fn ciphertext_round_trip_and_save_rollback() {
        let path = root("cipher");
        let key = SavedKey {
            id: "key-1".into(),
            masked: "••••••••".into(),
            ciphertext: vec![0, 1, 2, 255],
        };
        let mut store = CatalogStore::open(&path).unwrap();
        let before = provider("p", Some(key.clone()));
        store.save(std::slice::from_ref(&before), false).unwrap();
        assert_eq!(
            store.load().unwrap().0[0].endpoints[0].keys[0].ciphertext,
            key.ciphertext
        );
        let mut invalid = before.clone();
        invalid.endpoints[0].base_url = "http://not-loopback".into();
        assert!(store.save(&[invalid], false).is_err());
        assert_eq!(
            store.load().unwrap().0[0].endpoints[0].keys[0].ciphertext,
            key.ciphertext
        );
        let _ = std::fs::remove_dir_all(path);
    }
}
