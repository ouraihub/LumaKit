//! 聊天与公开历史接口共用的 SQLite 结构；旧文件仅在首次成功打开时导入。
use super::{Attempt, AttemptKind, Conversation, History, Turn};
use crate::history::{HistoryError, TurnStatus};
use lumakit_support::sqlite::{Migration, open_and_migrate};
use rusqlite::{Connection, OptionalExtension, params};
use std::{collections::HashSet, path::Path};

const MIGRATIONS: &[Migration] = &[
    Migration { version: 1, sql: "
CREATE TABLE chat_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE conversations (id TEXT PRIMARY KEY, position INTEGER NOT NULL, title TEXT NOT NULL, draft TEXT NOT NULL);
CREATE TABLE turns (id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE, position INTEGER NOT NULL, input TEXT NOT NULL);
CREATE TABLE attempts (id TEXT PRIMARY KEY, turn_id TEXT NOT NULL REFERENCES turns(id) ON DELETE CASCADE, position INTEGER NOT NULL, model TEXT NOT NULL, text TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('running','completed','failed','cancelled')), error TEXT, rounds TEXT NOT NULL);
CREATE INDEX turns_order ON turns(conversation_id, position);
CREATE INDEX attempts_order ON attempts(turn_id, position);
CREATE TABLE legacy_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE legacy_conversations (id TEXT PRIMARY KEY, position INTEGER NOT NULL, title TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE legacy_turns (id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES legacy_conversations(id) ON DELETE CASCADE, position INTEGER NOT NULL, model TEXT NOT NULL, status TEXT NOT NULL, messages TEXT NOT NULL, skill_refs TEXT NOT NULL, image_refs TEXT NOT NULL, error TEXT, usage TEXT, source_turn_id TEXT, attempt INTEGER NOT NULL, created_at INTEGER NOT NULL, finished_at INTEGER);
CREATE INDEX legacy_turns_order ON legacy_turns(conversation_id, position);
" },
    Migration { version: 2, sql: "
ALTER TABLE turns ADD COLUMN selected_attempt TEXT;
ALTER TABLE attempts ADD COLUMN kind TEXT NOT NULL DEFAULT 'reply' CHECK(kind IN ('reply','translation'));
ALTER TABLE attempts ADD COLUMN source_attempt_id TEXT;
ALTER TABLE attempts ADD COLUMN parent_attempt_id TEXT;
ALTER TABLE attempts ADD COLUMN search TEXT NOT NULL DEFAULT '{}';
ALTER TABLE attempts ADD COLUMN generated TEXT NOT NULL DEFAULT '[]';
" },
    Migration { version: 3, sql: "
ALTER TABLE attempts ADD COLUMN tool_events TEXT NOT NULL DEFAULT '[]';
" },
    Migration { version: 4, sql: "
ALTER TABLE attempts ADD COLUMN reasoning TEXT NOT NULL DEFAULT '[]';
" },
    Migration { version: 5, sql: "
CREATE TABLE usage_requests (request_id TEXT PRIMARY KEY, started_at_ms INTEGER, legacy INTEGER NOT NULL CHECK(legacy IN (0,1)), round TEXT NOT NULL);
CREATE INDEX usage_requests_time ON usage_requests(started_at_ms);
" },
    Migration { version: 6, sql: "
ALTER TABLE attempts ADD COLUMN trace_id TEXT;
" },
];

/// 打开共用结构并拒绝未来版本。
pub(crate) fn connect(path: &Path) -> Result<Connection, HistoryError> {
    Ok(open_and_migrate(path, MIGRATIONS)?)
}

pub(crate) fn open(path: &Path) -> Result<(Connection, History), HistoryError> {
    let mut connection = connect(path)?;
    let imported = connection
        .query_row(
            "SELECT value FROM chat_meta WHERE key='imported'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if imported.is_none() {
        #[derive(serde::Deserialize)]
        struct Legacy {
            #[serde(rename = "schemaVersion")]
            version: u32,
            value: History,
        }
        let mut history = match std::fs::read(path.with_file_name("chat-history.json")) {
            Ok(bytes) => {
                let legacy: Legacy = serde_json::from_slice(&bytes)?;
                if legacy.version != 1 {
                    return Err(HistoryError::InvalidData("聊天记录版本不受支持"));
                }
                legacy.value
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => History::default(),
            Err(error) => return Err(error.into()),
        };
        for conversation in &mut history.conversations {
            for turn in &mut conversation.turns {
                if turn.selected_attempt.is_none() {
                    turn.selected_attempt = turn.attempts.last().map(|attempt| attempt.id.clone());
                }
            }
        }
        validate(&history)?;
        let transaction = connection.transaction()?;
        write(&transaction, &history)?;
        transaction.execute(
            "INSERT INTO chat_meta(key,value) VALUES('imported','1')",
            [],
        )?;
        transaction.commit()?;
    }
    let mut history = read(&connection)?;
    for conversation in &mut history.conversations {
        for turn in &mut conversation.turns {
            for attempt in &mut turn.attempts {
                for (index, round) in attempt.rounds.iter_mut().enumerate() {
                    if let std::borrow::Cow::Owned(normalized) =
                        crate::usage::normalized(round, &attempt.model, &attempt.id, index)
                    {
                        *round = normalized;
                    }
                }
            }
        }
    }
    validate(&history)?;
    let transaction = connection.transaction()?;
    crate::usage::record(&transaction, &history)?;
    transaction.commit()?;
    Ok((connection, history))
}

pub(super) fn load_composer(
    connection: &Connection,
) -> Result<super::composer::ComposerData, HistoryError> {
    let value: Option<String> = connection
        .query_row(
            "SELECT value FROM chat_meta WHERE key='composer'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(value
        .map(|value| serde_json::from_str(&value))
        .transpose()?
        .unwrap_or_default())
}

/// 在单个事务内提交聊天历史、输入控制和附件内容。
pub(super) fn save_composer(
    connection: &mut Connection,
    history: &History,
    composer: &super::composer::ComposerData,
) -> Result<(), HistoryError> {
    validate(history)?;
    let transaction = connection.transaction()?;
    write(&transaction, history)?;
    transaction.execute("INSERT INTO chat_meta(key,value) VALUES('composer',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [serde_json::to_string(composer)?])?;
    transaction.commit()?;
    Ok(())
}

fn validate(history: &History) -> Result<(), HistoryError> {
    let mut conversations = HashSet::new();
    let mut turns = HashSet::new();
    let mut attempts = HashSet::new();
    if let Some(model) = &history.model {
        validate_model(model)?;
    }
    for conversation in &history.conversations {
        if conversation.id.trim().is_empty() || !conversations.insert(&conversation.id) {
            return Err(HistoryError::InvalidData("对话标识为空或重复"));
        }
        for turn in &conversation.turns {
            if turn.id.trim().is_empty() || !turns.insert(&turn.id) {
                return Err(HistoryError::InvalidData("问题标识为空或重复"));
            }
            if let Some(selected) = &turn.selected_attempt
                && !turn.attempts.iter().any(|attempt| &attempt.id == selected)
            {
                return Err(HistoryError::InvalidData("选中的回答不存在"));
            }
            for attempt in &turn.attempts {
                if attempt.id.trim().is_empty() || !attempts.insert(&attempt.id) {
                    return Err(HistoryError::InvalidData("回答标识为空或重复"));
                }
                validate_model(&attempt.model)?;
                if let Some(source) = &attempt.source_attempt_id
                    && !attempts.contains(source)
                    && !turn
                        .attempts
                        .iter()
                        .any(|candidate| &candidate.id == source)
                {
                    return Err(HistoryError::InvalidData("回答来源不存在"));
                }
                if let Some(parent) = &attempt.parent_attempt_id
                    && !turn
                        .attempts
                        .iter()
                        .any(|candidate| &candidate.id == parent)
                {
                    return Err(HistoryError::InvalidData("回答父节点不存在"));
                }
            }
        }
    }
    Ok(())
}

fn validate_model(model: &crate::ModelRef) -> Result<(), HistoryError> {
    if [&model.provider, &model.endpoint, &model.model]
        .iter()
        .any(|value| value.trim().is_empty())
    {
        return Err(HistoryError::InvalidData("模型来源或模型标识为空"));
    }
    Ok(())
}

fn write(connection: &Connection, history: &History) -> Result<(), HistoryError> {
    crate::usage::record(connection, history)?;
    connection.execute_batch("CREATE TEMP TABLE IF NOT EXISTS chat_live (kind INTEGER NOT NULL, id TEXT NOT NULL, PRIMARY KEY(kind,id)); DELETE FROM chat_live;")?;
    for (ci, conversation) in history.conversations.iter().enumerate() {
        connection.execute("INSERT INTO chat_live VALUES(0,?1)", [&conversation.id])?;
        connection.execute("INSERT INTO conversations(id,position,title,draft) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET position=excluded.position,title=excluded.title,draft=excluded.draft WHERE position IS NOT excluded.position OR title IS NOT excluded.title OR draft IS NOT excluded.draft", params![conversation.id, ci as i64, conversation.title, conversation.draft])?;
        for (ti, turn) in conversation.turns.iter().enumerate() {
            connection.execute("INSERT INTO chat_live VALUES(1,?1)", [&turn.id])?;
            connection.execute("INSERT INTO turns(id,conversation_id,position,input,selected_attempt) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET conversation_id=excluded.conversation_id,position=excluded.position,input=excluded.input,selected_attempt=excluded.selected_attempt", params![turn.id, conversation.id, ti as i64, turn.input, turn.selected_attempt])?;
            for (ai, attempt) in turn.attempts.iter().enumerate() {
                connection.execute("INSERT INTO chat_live VALUES(2,?1)", [&attempt.id])?;
                connection.execute("INSERT INTO attempts(id,turn_id,position,model,text,status,error,rounds,kind,source_attempt_id,parent_attempt_id,search,tool_events,reasoning,generated,trace_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16) ON CONFLICT(id) DO UPDATE SET turn_id=excluded.turn_id,position=excluded.position,model=excluded.model,text=excluded.text,status=excluded.status,error=excluded.error,rounds=excluded.rounds,kind=excluded.kind,source_attempt_id=excluded.source_attempt_id,parent_attempt_id=excluded.parent_attempt_id,search=excluded.search,tool_events=excluded.tool_events,reasoning=excluded.reasoning,generated=excluded.generated,trace_id=excluded.trace_id", params![attempt.id, turn.id, ai as i64, serde_json::to_string(&attempt.model)?, attempt.text, status(attempt.status), attempt.error, serde_json::to_string(&attempt.rounds)?, kind(attempt.kind), attempt.source_attempt_id, attempt.parent_attempt_id, serde_json::to_string(&attempt.search)?, serde_json::to_string(&attempt.tool_events)?, serde_json::to_string(&attempt.reasoning)?, serde_json::to_string(&attempt.generated)?, attempt.trace_id])?;
            }
        }
    }
    connection.execute_batch("DELETE FROM attempts WHERE id NOT IN (SELECT id FROM chat_live WHERE kind=2); DELETE FROM turns WHERE id NOT IN (SELECT id FROM chat_live WHERE kind=1); DELETE FROM conversations WHERE id NOT IN (SELECT id FROM chat_live WHERE kind=0);")?;
    for (key, value) in [
        ("selected", serde_json::to_string(&history.selected)?),
        ("model", serde_json::to_string(&history.model)?),
    ] {
        connection.execute("INSERT INTO chat_meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value IS NOT excluded.value", params![key, value])?;
    }
    Ok(())
}

fn read(connection: &Connection) -> Result<History, HistoryError> {
    let selected: String = connection.query_row(
        "SELECT value FROM chat_meta WHERE key='selected'",
        [],
        |row| row.get(0),
    )?;
    let model: String =
        connection.query_row("SELECT value FROM chat_meta WHERE key='model'", [], |row| {
            row.get(0)
        })?;
    let mut history = History {
        conversations: Vec::new(),
        selected: serde_json::from_str(&selected)?,
        model: serde_json::from_str(&model)?,
    };
    let mut conversations =
        connection.prepare("SELECT id,title,draft FROM conversations ORDER BY position")?;
    let mut turns = connection.prepare(
        "SELECT id,input,selected_attempt FROM turns WHERE conversation_id=?1 ORDER BY position",
    )?;
    let mut attempts = connection.prepare("SELECT id,model,text,status,error,rounds,kind,source_attempt_id,parent_attempt_id,search,tool_events,reasoning,generated,trace_id FROM attempts WHERE turn_id=?1 ORDER BY position")?;
    let mut rows = conversations.query([])?;
    while let Some(row) = rows.next()? {
        let mut conversation = Conversation {
            id: row.get(0)?,
            title: row.get(1)?,
            draft: row.get(2)?,
            turns: Vec::new(),
        };
        let mut turn_rows = turns.query([&conversation.id])?;
        while let Some(row) = turn_rows.next()? {
            let mut turn = Turn {
                id: row.get(0)?,
                input: row.get(1)?,
                attempts: Vec::new(),
                selected_attempt: row.get(2)?,
            };
            let mut attempt_rows = attempts.query([&turn.id])?;
            while let Some(row) = attempt_rows.next()? {
                turn.attempts.push(Attempt {
                    id: row.get(0)?,
                    model: serde_json::from_str(&row.get::<_, String>(1)?)?,
                    text: row.get(2)?,
                    status: parse_status(&row.get::<_, String>(3)?)?,
                    error: row.get(4)?,
                    trace_id: row.get(13)?,
                    rounds: serde_json::from_str(&row.get::<_, String>(5)?)?,
                    reasoning: serde_json::from_str(&row.get::<_, String>(11)?)?,
                    kind: parse_kind(&row.get::<_, String>(6)?)?,
                    source_attempt_id: row.get(7)?,
                    parent_attempt_id: row.get(8)?,
                    search: serde_json::from_str(&row.get::<_, String>(9)?)?,
                    tool_events: serde_json::from_str(&row.get::<_, String>(10)?)?,
                    generated: serde_json::from_str(&row.get::<_, String>(12)?)?,
                });
            }
            conversation.turns.push(turn);
        }
        history.conversations.push(conversation);
    }
    Ok(history)
}

/// 将状态保存为可查询的独立列。
pub(crate) fn status(value: TurnStatus) -> &'static str {
    match value {
        TurnStatus::Running => "running",
        TurnStatus::Completed => "completed",
        TurnStatus::Failed => "failed",
        TurnStatus::Cancelled => "cancelled",
    }
}

/// 拒绝未知状态，避免将坏数据误作空历史。
pub(crate) fn parse_status(value: &str) -> Result<TurnStatus, HistoryError> {
    match value {
        "running" => Ok(TurnStatus::Running),
        "completed" => Ok(TurnStatus::Completed),
        "failed" => Ok(TurnStatus::Failed),
        "cancelled" => Ok(TurnStatus::Cancelled),
        _ => Err(HistoryError::InvalidData("未知的回答状态")),
    }
}
fn kind(value: AttemptKind) -> &'static str {
    match value {
        AttemptKind::Reply => "reply",
        AttemptKind::Translation => "translation",
    }
}

fn parse_kind(value: &str) -> Result<AttemptKind, HistoryError> {
    match value {
        "reply" => Ok(AttemptKind::Reply),
        "translation" => Ok(AttemptKind::Translation),
        _ => Err(HistoryError::InvalidData("未知的回答类型")),
    }
}
