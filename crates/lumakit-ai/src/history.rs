//! SQLite 历史记录、崩溃恢复与保守重试策略。
use crate::chat::store;
use lumakit_support::FileLock;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

/// 历史记录读取、解析或持久化失败的原因。
#[derive(Debug, Error)]
pub enum HistoryError {
    /// 文件系统读写失败。
    #[error("history I/O failed: {0}")]
    Io(#[from] io::Error),
    /// 历史文件的 JSON 无法解析。
    #[error("history JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    /// 指定的对话或轮次不存在。
    #[error("conversation or turn was not found")]
    NotFound,
    /// 数据目录独占锁或旧文件读取失败。
    #[error("history storage failed: {0}")]
    Storage(#[from] lumakit_support::SupportError),
    /// SQLite 迁移失败或数据库版本过新。
    #[error("history migration failed: {0}")]
    Migration(#[from] lumakit_support::sqlite::SqliteError),
    /// 数据库读取、事务或提交失败。
    #[error("history database failed: {0}")]
    Database(#[from] rusqlite::Error),
    /// 历史数据包含重复标识或不支持的版本、状态。
    #[error("invalid history data: {0}")]
    InvalidData(&'static str),
}

/// 消息或轮次所引用的图片元数据。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageRef {
    /// 图片标识。
    pub id: String,
    /// 图片的 MIME 类型。
    pub mime_type: String,
    /// 图片数据的字节数。
    pub bytes: u64,
    /// 图片宽度，单位为像素；未知时为空。
    #[serde(default)]
    pub width: Option<u32>,
    /// 图片高度，单位为像素；未知时为空。
    #[serde(default)]
    pub height: Option<u32>,
}

/// 持久化的一条消息及其图片引用。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageRecord {
    /// 消息角色。
    pub role: String,
    /// 消息文本。
    pub content: String,
    /// 消息附带的图片引用。
    #[serde(default)]
    pub images: Vec<ImageRef>,
}

/// 一轮请求的执行状态。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TurnStatus {
    /// 请求尚未结束。
    Running,
    /// 请求已成功完成。
    Completed,
    /// 请求失败或在恢复时被判定为中断。
    Failed,
    /// 请求已取消。
    Cancelled,
}

/// 一轮请求的模型、消息、执行状态与持久化元数据。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Turn {
    /// 轮次标识。
    pub id: String,
    /// 所属对话标识。
    pub conversation_id: String,
    /// 请求使用的模型标识。
    pub model: String,
    /// 当前执行状态。
    pub status: TurnStatus,
    /// 本轮记录的消息。
    pub messages: Vec<MessageRecord>,
    /// 本轮引用的技能标识。
    #[serde(default)]
    pub skill_refs: Vec<String>,
    /// 本轮引用的图片元数据。
    #[serde(default)]
    pub image_refs: Vec<ImageRef>,
    /// 失败或中断说明。
    #[serde(default)]
    pub error: Option<String>,
    /// 服务返回的用量数据。
    #[serde(default)]
    pub usage: Option<serde_json::Value>,
    /// 重试来源轮次的标识。
    #[serde(default)]
    pub source_turn_id: Option<String>,
    /// 尝试序号，缺省为 1。
    #[serde(default = "one")]
    pub attempt: u8,
    /// 创建时间，单位为 Unix 秒。
    pub created_at: u64,
    /// 结束时间，单位为 Unix 秒；未结束时为空。
    pub finished_at: Option<u64>,
}

/// 按插入顺序保存轮次的对话。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Conversation {
    /// 对话标识。
    pub id: String,
    /// 可选的对话标题。
    #[serde(default)]
    pub title: Option<String>,
    /// 创建时间，单位为 Unix 秒。
    pub created_at: u64,
    /// 最近更新时间，单位为 Unix 秒。
    pub updated_at: u64,
    /// 对话中的轮次，按插入顺序排列。
    #[serde(default)]
    pub turns: Vec<Turn>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct HistoryFile {
    conversations: Vec<Conversation>,
}

impl HistoryFile {
    fn validate(&self) -> Result<(), HistoryError> {
        let mut conversations = std::collections::HashSet::new();
        let mut turns = std::collections::HashSet::new();
        for conversation in &self.conversations {
            if conversation.id.trim().is_empty() || !conversations.insert(&conversation.id) {
                return Err(HistoryError::InvalidData("对话标识为空或重复"));
            }
            for turn in &conversation.turns {
                if turn.id.trim().is_empty() || !turns.insert(&turn.id) {
                    return Err(HistoryError::InvalidData("问题标识为空或重复"));
                }
                if turn.conversation_id != conversation.id
                    || turn.model.trim().is_empty()
                    || turn.attempt == 0
                {
                    return Err(HistoryError::InvalidData(
                        "问题所属对话、模型或尝试序号无效",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// 以 SQLite 保存历史；与 ChatSession 使用不同表，旧 JSON 仅作首次导入备份。
#[derive(Debug)]
pub struct HistoryStore {
    connection: Connection,
    _lock: FileLock,
    data: HistoryFile,
}

impl HistoryStore {
    /// 打开实际数据库路径，并将未完成轮次标为中断。
    /// 首次成功打开时导入同名 `.json` 的旧裸历史；导入及标记在同一事务提交。
    /// 同目录聊天锁阻止另一个写实例；之后即使历史为空也不会重新导入。
    ///
    /// # Errors
    /// Returns an error if the writer lock cannot be acquired, database migration/read/write
    /// fails, or legacy/stored history is malformed or violates history invariants.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HistoryError> {
        let path = path.as_ref();
        let lock = FileLock::acquire(&path.with_file_name("chat-history.lock"))?;
        let connection = store::connect(path)?;
        let imported = connection
            .query_row(
                "SELECT value FROM legacy_meta WHERE key='imported'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let mut data = if imported.is_none() {
            let legacy = path.with_extension("json");
            match fs::read(legacy) {
                Ok(bytes) => serde_json::from_slice(&bytes)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => HistoryFile::default(),
                Err(error) => return Err(error.into()),
            }
        } else {
            Self::read(&connection)?
        };
        data.validate()?;
        let mut recovered = false;
        for conversation in &mut data.conversations {
            for turn in &mut conversation.turns {
                if turn.status == TurnStatus::Running {
                    turn.status = TurnStatus::Failed;
                    turn.error = Some("interrupted".to_owned());
                    turn.finished_at = Some(now());
                    conversation.updated_at = turn.finished_at.unwrap_or(conversation.updated_at);
                    recovered = true;
                }
            }
        }
        let store = Self {
            connection,
            _lock: lock,
            data,
        };
        if recovered || imported.is_none() {
            store.persist()?;
        }
        Ok(store)
    }

    /// Alias for [`HistoryStore::open`].
    ///
    /// # Errors
    /// Returns the same locking, storage, and history-validation errors as [`Self::open`].
    pub fn new(path: impl AsRef<Path>) -> Result<Self, HistoryError> {
        Self::open(path)
    }

    /// Starts and durably records a new running turn.
    ///
    /// # Errors
    /// Returns an error if an ID cannot be generated or validated history cannot be persisted.
    /// Failed persistence restores the previous in-memory history.
    pub fn begin(
        &mut self,
        conversation_id: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Turn, HistoryError> {
        let conversation_id = conversation_id.into();
        let timestamp = now();
        let turn = Turn {
            id: lumakit_support::new_id()
                .map_err(|_| HistoryError::InvalidData("无法生成问题标识"))?,
            conversation_id: conversation_id.clone(),
            model: model.into(),
            status: TurnStatus::Running,
            messages: Vec::new(),
            skill_refs: Vec::new(),
            image_refs: Vec::new(),
            error: None,
            usage: None,
            source_turn_id: None,
            attempt: 1,
            created_at: timestamp,
            finished_at: None,
        };
        let previous = self.data.clone();
        if let Some(conversation) = self
            .data
            .conversations
            .iter_mut()
            .find(|conversation| conversation.id == conversation_id)
        {
            conversation.updated_at = timestamp;
            conversation.turns.push(turn.clone());
        } else {
            self.data.conversations.push(Conversation {
                id: conversation_id,
                title: None,
                created_at: timestamp,
                updated_at: timestamp,
                turns: vec![turn.clone()],
            });
        }
        if let Err(error) = self.persist() {
            self.data = previous;
            return Err(error);
        }
        Ok(turn)
    }

    /// Replaces a running turn with its final, atomically persisted result.
    ///
    /// # Errors
    /// Returns an error if the conversation/turn is missing or validated history cannot be
    /// persisted. Failed persistence restores the previous in-memory history.
    pub fn finish(&mut self, mut turn: Turn) -> Result<(), HistoryError> {
        if turn.finished_at.is_none() {
            turn.finished_at = Some(now());
        }
        let previous = self.data.clone();
        let conversation = self
            .data
            .conversations
            .iter_mut()
            .find(|conversation| conversation.id == turn.conversation_id)
            .ok_or(HistoryError::NotFound)?;
        let stored = conversation
            .turns
            .iter_mut()
            .find(|stored| stored.id == turn.id)
            .ok_or(HistoryError::NotFound)?;
        *stored = turn;
        conversation.updated_at = now();
        if let Err(error) = self.persist() {
            self.data = previous;
            return Err(error);
        }
        Ok(())
    }

    /// Lists turns in insertion order, including failed and cancelled turns.
    pub fn list(&self, conversation_id: &str) -> Vec<Turn> {
        self.data
            .conversations
            .iter()
            .find(|conversation| conversation.id == conversation_id)
            .map(|conversation| conversation.turns.clone())
            .unwrap_or_default()
    }

    /// Loads one turn, if it exists.
    pub fn load(&self, conversation_id: &str, turn_id: &str) -> Option<Turn> {
        self.data
            .conversations
            .iter()
            .find(|conversation| conversation.id == conversation_id)
            .and_then(|conversation| conversation.turns.iter().find(|turn| turn.id == turn_id))
            .cloned()
    }

    /// Lists all conversations without exposing the store's mutable state.
    pub fn conversations(&self) -> Vec<Conversation> {
        self.data.conversations.clone()
    }

    fn persist(&self) -> Result<(), HistoryError> {
        self.data.validate()?;
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute_batch("CREATE TEMP TABLE IF NOT EXISTS legacy_live (kind INTEGER NOT NULL, id TEXT NOT NULL, PRIMARY KEY(kind,id)); DELETE FROM legacy_live;")?;
        for (ci, conversation) in self.data.conversations.iter().enumerate() {
            transaction.execute("INSERT INTO legacy_live VALUES(0,?1)", [&conversation.id])?;
            transaction.execute("INSERT INTO legacy_conversations(id,position,title,created_at,updated_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET position=excluded.position,title=excluded.title,created_at=excluded.created_at,updated_at=excluded.updated_at WHERE position IS NOT excluded.position OR title IS NOT excluded.title OR created_at IS NOT excluded.created_at OR updated_at IS NOT excluded.updated_at", params![conversation.id, ci as i64, conversation.title, timestamp_to_sql(conversation.created_at)?, timestamp_to_sql(conversation.updated_at)?])?;
            for (ti, turn) in conversation.turns.iter().enumerate() {
                transaction.execute("INSERT INTO legacy_live VALUES(1,?1)", [&turn.id])?;
                transaction.execute("INSERT INTO legacy_turns(id,conversation_id,position,model,status,messages,skill_refs,image_refs,error,usage,source_turn_id,attempt,created_at,finished_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14) ON CONFLICT(id) DO UPDATE SET conversation_id=excluded.conversation_id,position=excluded.position,model=excluded.model,status=excluded.status,messages=excluded.messages,skill_refs=excluded.skill_refs,image_refs=excluded.image_refs,error=excluded.error,usage=excluded.usage,source_turn_id=excluded.source_turn_id,attempt=excluded.attempt,created_at=excluded.created_at,finished_at=excluded.finished_at WHERE conversation_id IS NOT excluded.conversation_id OR position IS NOT excluded.position OR model IS NOT excluded.model OR status IS NOT excluded.status OR messages IS NOT excluded.messages OR skill_refs IS NOT excluded.skill_refs OR image_refs IS NOT excluded.image_refs OR error IS NOT excluded.error OR usage IS NOT excluded.usage OR source_turn_id IS NOT excluded.source_turn_id OR attempt IS NOT excluded.attempt OR created_at IS NOT excluded.created_at OR finished_at IS NOT excluded.finished_at", params![turn.id, turn.conversation_id, ti as i64, turn.model, store::status(turn.status), serde_json::to_string(&turn.messages)?, serde_json::to_string(&turn.skill_refs)?, serde_json::to_string(&turn.image_refs)?, turn.error, turn.usage.as_ref().map(serde_json::to_string).transpose()?, turn.source_turn_id, turn.attempt, timestamp_to_sql(turn.created_at)?, turn.finished_at.map(timestamp_to_sql).transpose()?])?;
            }
        }
        transaction.execute_batch("DELETE FROM legacy_turns WHERE id NOT IN (SELECT id FROM legacy_live WHERE kind=1); DELETE FROM legacy_conversations WHERE id NOT IN (SELECT id FROM legacy_live WHERE kind=0); INSERT INTO legacy_meta(key,value) VALUES('imported','1') ON CONFLICT(key) DO NOTHING;")?;
        transaction.commit()?;
        Ok(())
    }

    fn read(connection: &Connection) -> Result<HistoryFile, HistoryError> {
        let mut data = HistoryFile::default();
        let mut conversations = connection.prepare(
            "SELECT id,title,created_at,updated_at FROM legacy_conversations ORDER BY position",
        )?;
        let mut turns = connection.prepare("SELECT id,model,status,messages,skill_refs,image_refs,error,usage,source_turn_id,attempt,created_at,finished_at FROM legacy_turns WHERE conversation_id=?1 ORDER BY position")?;
        let mut rows = conversations.query([])?;
        while let Some(row) = rows.next()? {
            let mut conversation = Conversation {
                id: row.get(0)?,
                title: row.get(1)?,
                created_at: timestamp_from_sql(row.get(2)?)?,
                updated_at: timestamp_from_sql(row.get(3)?)?,
                turns: Vec::new(),
            };
            let mut turn_rows = turns.query([&conversation.id])?;
            while let Some(row) = turn_rows.next()? {
                conversation.turns.push(Turn {
                    id: row.get(0)?,
                    conversation_id: conversation.id.clone(),
                    model: row.get(1)?,
                    status: store::parse_status(&row.get::<_, String>(2)?)?,
                    messages: serde_json::from_str(&row.get::<_, String>(3)?)?,
                    skill_refs: serde_json::from_str(&row.get::<_, String>(4)?)?,
                    image_refs: serde_json::from_str(&row.get::<_, String>(5)?)?,
                    error: row.get(6)?,
                    usage: row
                        .get::<_, Option<String>>(7)?
                        .map(|value| serde_json::from_str(&value))
                        .transpose()?,
                    source_turn_id: row.get(8)?,
                    attempt: row.get(9)?,
                    created_at: timestamp_from_sql(row.get(10)?)?,
                    finished_at: row
                        .get::<_, Option<i64>>(11)?
                        .map(timestamp_from_sql)
                        .transpose()?,
                });
            }
            data.conversations.push(conversation);
        }
        Ok(data)
    }
}

fn timestamp_to_sql(value: u64) -> Result<i64, HistoryError> {
    i64::try_from(value).map_err(|_| HistoryError::InvalidData("时间戳超出 SQLite 整数范围"))
}

fn timestamp_from_sql(value: i64) -> Result<u64, HistoryError> {
    u64::try_from(value).map_err(|_| HistoryError::InvalidData("数据库时间戳不能为负数"))
}

/// 用于判断是否允许安全重试的错误分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryError {
    /// 网络连接错误。
    Network,
    /// 请求超时。
    Timeout,
    /// HTTP 错误及其状态码。
    Http(u16),
    /// 请求主动取消，不允许自动重试。
    Cancelled,
    /// 其它不允许自动重试的错误。
    Other,
}

/// 当前尝试是否已经产生输出、工具调用或副作用。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AttemptState {
    /// 已产生文本输出。
    pub text_output: bool,
    /// 已发起工具调用。
    pub tool_call: bool,
    /// 已产生副作用。
    pub side_effect: bool,
}

/// 仅在未产生输出或副作用时允许重试的策略；默认最多两次尝试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// 总尝试次数上限；实际判定最多允许两次。
    pub max_attempts: u8,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { max_attempts: 2 }
    }
}

impl RetryPolicy {
    /// Returns true only for a safe, output-free retry before the attempt limit.
    pub fn should_retry(&self, attempts: u8, error: RetryError, state: AttemptState) -> bool {
        attempts >= 1
            && attempts < self.max_attempts.min(2)
            && !state.text_output
            && !state.tool_call
            && !state.side_effect
            && matches!(
                error,
                RetryError::Network | RetryError::Timeout | RetryError::Http(429 | 502 | 503 | 504)
            )
    }
}

/// Applies the default two-attempt safe retry policy.
pub fn retry_policy(attempts: u8, error: RetryError, state: AttemptState) -> bool {
    RetryPolicy::default().should_retry(attempts, error, state)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

const fn one() -> u8 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn timestamp_storage_rejects_overflow_and_negative_values()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(timestamp_to_sql(0)?, 0);
        assert_eq!(timestamp_from_sql(0)?, 0);
        let maximum = u64::try_from(i64::MAX)?;
        assert_eq!(timestamp_to_sql(maximum)?, i64::MAX);
        assert_eq!(timestamp_from_sql(i64::MAX)?, maximum);
        assert!(matches!(
            timestamp_to_sql(maximum + 1),
            Err(HistoryError::InvalidData(_))
        ));
        assert!(matches!(
            timestamp_to_sql(u64::MAX),
            Err(HistoryError::InvalidData(_))
        ));
        assert!(matches!(
            timestamp_from_sql(-1),
            Err(HistoryError::InvalidData(_))
        ));
        Ok(())
    }

    fn temp_path(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!(
            "lumakit-history-{name}-{}",
            lumakit_support::new_id()?
        ));
        fs::create_dir_all(&root)?;
        Ok(root.join("history.db"))
    }

    #[test]
    fn running_turn_is_recovered_as_interrupted() -> Result<(), Box<dyn std::error::Error>> {
        let path = temp_path("recovery")?;
        let mut store = HistoryStore::open(&path)?;
        let turn = store.begin("conversation", "model")?;
        drop(store);
        let store = HistoryStore::open(&path)?;
        let recovered = store
            .load("conversation", &turn.id)
            .ok_or("recovered turn missing")?;
        assert_eq!(recovered.status, TurnStatus::Failed);
        assert_eq!(recovered.error.as_deref(), Some("interrupted"));
        drop(store);
        let _ = fs::remove_dir_all(path.parent().ok_or("history parent missing")?);
        Ok(())
    }

    #[test]
    fn legacy_empty_duplicate_and_mismatched_ids_are_rejected_before_import()
    -> Result<(), Box<dyn std::error::Error>> {
        let source_path = temp_path("validation-source")?;
        let mut source = HistoryStore::open(&source_path)?;
        source.begin("conversation", "model")?;
        let data = source.data.clone();
        drop(source);
        for kind in 0..6 {
            let path = temp_path("validation-target")?;
            let mut invalid = data.clone();
            match kind {
                0 => invalid.conversations[0].id.clear(),
                1 => invalid.conversations.push(invalid.conversations[0].clone()),
                2 => invalid.conversations[0].turns[0].id = " ".into(),
                3 => {
                    let turn = invalid.conversations[0].turns[0].clone();
                    invalid.conversations[0].turns.push(turn);
                }
                4 => invalid.conversations[0].turns[0].conversation_id = "missing".into(),
                _ => invalid.conversations[0].turns[0].attempt = 0,
            }
            let bytes = serde_json::to_vec(&invalid)?;
            let legacy = path.with_extension("json");
            fs::write(&legacy, &bytes)?;
            assert!(HistoryStore::open(&path).is_err(), "case {kind}");
            let database = Connection::open(&path)?;
            for table in ["legacy_meta", "legacy_conversations", "legacy_turns"] {
                assert_eq!(
                    database
                        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                            .get::<_, i64>(0))?,
                    0
                );
            }
            assert_eq!(fs::read(&legacy)?, bytes);
            drop(database);
            fs::remove_dir_all(path.parent().ok_or("history parent missing")?)?;
        }
        fs::remove_dir_all(source_path.parent().ok_or("history parent missing")?)?;
        Ok(())
    }

    #[test]
    fn legacy_import_failure_is_atomic_and_backup_is_never_reimported()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = temp_path("migration")?;
        let legacy = path.with_extension("json");
        let mut source = HistoryStore::open(&path)?;
        let turn = source.begin("conversation", "model")?;
        let mut finished = turn.clone();
        finished.status = TurnStatus::Completed;
        finished.messages.push(MessageRecord {
            role: "assistant".into(),
            content: "保留回答".into(),
            images: Vec::new(),
        });
        source.finish(finished)?;
        let bytes = serde_json::to_vec(&source.data)?;
        drop(source);
        fs::write(&legacy, &bytes)?;
        let database = Connection::open(&path)?;
        database.execute_batch("DELETE FROM legacy_turns; DELETE FROM legacy_conversations; DELETE FROM legacy_meta; CREATE TRIGGER reject_import BEFORE INSERT ON legacy_turns BEGIN SELECT RAISE(ABORT,'injected failure'); END;")?;
        assert!(HistoryStore::open(&path).is_err());
        assert_eq!(
            database.query_row("SELECT count(*) FROM legacy_meta", [], |row| row
                .get::<_, i64>(0))?,
            0
        );
        assert_eq!(
            database.query_row("SELECT count(*) FROM legacy_conversations", [], |row| row
                .get::<_, i64>(
                0
            ))?,
            0
        );
        assert_eq!(fs::read(&legacy)?, bytes);
        database.execute_batch("DROP TRIGGER reject_import;")?;
        let mut store = HistoryStore::open(&path)?;
        assert_eq!(
            store
                .load("conversation", &turn.id)
                .ok_or("imported turn missing")?
                .messages[0]
                .content,
            "保留回答"
        );
        assert!(HistoryStore::open(&path).is_err());
        database.execute_batch("CREATE TRIGGER reject_finish BEFORE INSERT ON legacy_turns BEGIN SELECT RAISE(ABORT,'injected failure'); END;")?;
        let before = store.conversations();
        assert!(store.begin("new", "model").is_err());
        assert_eq!(store.conversations(), before);
        let mut changed = store
            .load("conversation", &turn.id)
            .ok_or("stored turn missing")?;
        changed.messages[0].content = "不应保存".into();
        assert!(store.finish(changed).is_err());
        assert_eq!(store.conversations(), before);
        database.execute_batch("DROP TRIGGER reject_finish;")?;
        store.data.conversations.clear();
        store.persist()?;
        drop(store);
        fs::write(&legacy, b"broken JSON backup")?;
        assert!(HistoryStore::open(&path)?.conversations().is_empty());
        drop(database);
        fs::remove_dir_all(path.parent().ok_or("history parent missing")?)?;
        Ok(())
    }

    #[test]
    fn malformed_legacy_history_is_not_replaced_with_empty_data()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = temp_path("malformed")?;
        let legacy = path.with_extension("json");
        fs::write(&legacy, b"invalid JSON")?;
        assert!(HistoryStore::open(&path).is_err());
        let connection = Connection::open(&path)?;
        assert_eq!(
            connection.query_row("SELECT count(*) FROM legacy_meta", [], |row| row
                .get::<_, i64>(0))?,
            0
        );
        assert_eq!(fs::read(&legacy)?, b"invalid JSON");
        drop(connection);
        fs::remove_dir_all(path.parent().ok_or("history parent missing")?)?;
        Ok(())
    }

    #[test]
    fn retry_requires_empty_output_and_allowed_error() {
        let empty = AttemptState::default();
        assert!(retry_policy(1, RetryError::Network, empty));
        assert!(retry_policy(1, RetryError::Timeout, empty));
        for status in [429, 502, 503, 504] {
            assert!(retry_policy(1, RetryError::Http(status), empty));
        }
        assert!(!retry_policy(2, RetryError::Network, empty));
        assert!(!retry_policy(1, RetryError::Http(400), empty));
        assert!(!retry_policy(
            1,
            RetryError::Network,
            AttemptState {
                text_output: true,
                ..empty
            }
        ));
        assert!(!retry_policy(
            1,
            RetryError::Network,
            AttemptState {
                tool_call: true,
                ..empty
            }
        ));
        assert!(!retry_policy(
            1,
            RetryError::Network,
            AttemptState {
                side_effect: true,
                ..empty
            }
        ));
    }
}
