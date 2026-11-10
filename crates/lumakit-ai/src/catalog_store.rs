//! SQLite persistence for the saved provider catalog.
use crate::catalog::ProviderEntry;
use lumakit_support::{
    load_versioned,
    sqlite::{Migration, open_and_migrate},
};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

mod persistence;
mod read;
mod validation;

use persistence::upsert_snapshot;
use validation::validate_catalog;

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
    Migration {
        version: 3,
        sql: "ALTER TABLE endpoint_keys ADD COLUMN label TEXT;",
    },
    Migration {
        version: 4,
        sql: "ALTER TABLE endpoint_keys ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1));",
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
    ///
    /// # Errors
    /// Returns an error for directory/database I/O, migration or initialization failures,
    /// or invalid legacy configuration that cannot be imported atomically.
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

fn placeholders(n: usize) -> String {
    std::iter::repeat_n("?", n).collect::<Vec<_>>().join(",")
}
fn sql(e: rusqlite::Error) -> String {
    format!("SQLite操作失败: {e}")
}

#[cfg(test)]
mod tests;
