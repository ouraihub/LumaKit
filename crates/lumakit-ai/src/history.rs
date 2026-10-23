//! Durable conversation history with crash recovery and conservative retries.
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Error)]
pub enum HistoryError {
    #[error("history I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("history JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("conversation or turn was not found")]
    NotFound,
    #[error("history storage failed: {0}")]
    Storage(#[from] lumakit_support::SupportError),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageRef {
    pub id: String,
    pub mime_type: String,
    pub bytes: u64,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageRecord {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub images: Vec<ImageRef>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TurnStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Turn {
    pub id: String,
    pub conversation_id: String,
    pub model: String,
    pub status: TurnStatus,
    pub messages: Vec<MessageRecord>,
    #[serde(default)]
    pub skill_refs: Vec<String>,
    #[serde(default)]
    pub image_refs: Vec<ImageRef>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub usage: Option<serde_json::Value>,
    #[serde(default)]
    pub source_turn_id: Option<String>,
    #[serde(default = "one")]
    pub attempt: u8,
    pub created_at: u64,
    pub finished_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Conversation {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub turns: Vec<Turn>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct HistoryFile {
    conversations: Vec<Conversation>,
}

#[derive(Debug)]
pub struct HistoryStore {
    path: PathBuf,
    data: HistoryFile,
}

impl HistoryStore {
    /// Opens a JSON history file and marks unfinished turns as interrupted.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HistoryError> {
        let path = path.as_ref().to_path_buf();
        let mut data = if path.exists() {
            serde_json::from_slice(&fs::read(&path)?)?
        } else {
            HistoryFile::default()
        };
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
        let store = Self { path, data };
        if recovered {
            store.persist()?;
        }
        Ok(store)
    }

    /// Alias for [`HistoryStore::open`].
    pub fn new(path: impl AsRef<Path>) -> Result<Self, HistoryError> {
        Self::open(path)
    }

    /// Starts and durably records a new running turn.
    pub fn begin(
        &mut self,
        conversation_id: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Turn, HistoryError> {
        let conversation_id = conversation_id.into();
        let timestamp = now();
        let turn = Turn {
            id: format!(
                "turn-{timestamp}-{}",
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ),
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
        lumakit_support::atomic_write(&self.path, |writer| {
            serde_json::to_writer(writer, &self.data)?;
            Ok(())
        })?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryError {
    Network,
    Timeout,
    Http(u16),
    Cancelled,
    Other,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AttemptState {
    pub text_output: bool,
    pub tool_call: bool,
    pub side_effect: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
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

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "lumakit-history-{name}-{}",
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn running_turn_is_recovered_as_interrupted() {
        let path = temp_path("recovery");
        let mut store = HistoryStore::open(&path).expect("open");
        let turn = store.begin("conversation", "model").expect("begin");
        drop(store);
        let store = HistoryStore::open(&path).expect("reopen");
        let recovered = store.load("conversation", &turn.id).expect("turn");
        assert_eq!(recovered.status, TurnStatus::Failed);
        assert_eq!(recovered.error.as_deref(), Some("interrupted"));
        let _ = fs::remove_file(path);
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
