//! 消费方可观察的模型对、偏好、重启和 SQLite 回滚合同。
use lumakit_ai::{
    ModelRef, Protocol,
    chat::{ChatSession, ComposerSettings, ModelOption, ModelPair, ModelPickerPreferences},
    model_config::{ModelSettings, ReasoningConfig, ReasoningDialect, ReasoningEffort},
};
use lumakit_support::new_id;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

type TestError = Box<dyn std::error::Error + Send + Sync>;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self, TestError> {
        let path = std::env::temp_dir().join(format!("lumakit-picker-{}", new_id()?));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
    fn db(&self) -> PathBuf {
        self.0.join("chat.db")
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn open(path: &Path) -> Result<ChatSession, TestError> {
    Ok(ChatSession::open(path, Arc::new(|| {}))?)
}
fn option(name: &str) -> ModelOption {
    ModelOption {
        model: ModelRef {
            provider: "provider".into(),
            endpoint: "endpoint".into(),
            model: name.into(),
            protocol: Protocol::OpenAiChat,
        },
        name: name.into(),
        provider: "提供方".into(),
        endpoint: "端点".into(),
        has_key: false,
        settings: ModelSettings {
            reasoning: Some(ReasoningConfig {
                dialect: ReasoningDialect::OpenAi,
                efforts: vec![
                    ReasoningEffort::Low,
                    ReasoningEffort::High,
                    ReasoningEffort::None,
                ],
                budget_tokens: None,
            }),
            ..Default::default()
        },
    }
}
fn saved_composer(db: &Connection) -> Result<Value, TestError> {
    Ok(serde_json::from_str(&db.query_row::<String, _, _>(
        "SELECT value FROM chat_meta WHERE key='composer'",
        [],
        |row| row.get(0),
    )?)?)
}
fn reject_composer(db: &Connection) -> Result<(), TestError> {
    db.execute_batch("CREATE TRIGGER reject_picker BEFORE UPDATE ON chat_meta WHEN NEW.key='composer' BEGIN SELECT RAISE(ABORT, 'injected picker failure'); END;")?;
    Ok(())
}
fn restore(db: &Connection, session: &mut ChatSession) -> Result<(), TestError> {
    db.execute_batch("DROP TRIGGER reject_picker;")?;
    session.retry_save();
    assert!(!session.save_failed());
    Ok(())
}

#[test]
fn catalog_reasoning_validates_and_reconciles_without_saved_capability_override()
-> Result<(), TestError> {
    let root = Directory::new()?;
    let mut session = open(&root.db())?;
    let mut selected = option("openai/gpt-6.1-sol");
    selected.settings = ModelSettings::default();
    session.select_model_pair(&selected, Some(ReasoningEffort::Max))?;
    session.set_default_model_pair(&selected, Some(ReasoningEffort::Max))?;
    session.reconcile_model_catalog(std::slice::from_ref(&selected))?;
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::Max));
    assert_eq!(
        session
            .model_picker_preferences()
            .default_pair
            .as_ref()
            .ok_or("missing expected value")?
            .effort,
        Some(ReasoningEffort::Max)
    );
    assert!(
        session
            .select_model_pair(&selected, Some(ReasoningEffort::None))
            .is_err()
    );
    selected.settings.reasoning_disabled = true;
    session.reconcile_model_catalog(std::slice::from_ref(&selected))?;
    assert_eq!(session.composer().reasoning, None);
    assert_eq!(
        session
            .model_picker_preferences()
            .default_pair
            .as_ref()
            .ok_or("missing expected value")?
            .effort,
        None
    );
    selected.model.protocol = Protocol::AnthropicMessages;
    selected.settings.reasoning_disabled = false;
    assert!(
        session
            .select_model_pair(&selected, Some(ReasoningEffort::High))
            .is_err()
    );
    Ok(())
}

#[test]
fn pending_pair_survives_restart_without_creating_conversation() -> Result<(), TestError> {
    let root = Directory::new()?;
    let selected = option("pending");
    let mut session = open(&root.db())?;
    session.select_model_pair(&selected, Some(ReasoningEffort::High))?;
    assert!(session.selected().is_none());
    assert!(session.history().conversations.is_empty());
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::High));
    assert_eq!(session.history().model.as_ref(), Some(&selected.model));
    drop(session);
    let mut session = open(&root.db())?;
    assert!(session.selected().is_none());
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::High));
    assert_eq!(
        session.model_picker_preferences().recents,
        vec![selected.model.clone()]
    );
    session.new_conversation();
    assert_eq!(session.request_model(false)?, selected.model);
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::High));
    Ok(())
}

#[test]
fn defaults_only_apply_to_new_chats_and_switching_restores_pairs() -> Result<(), TestError> {
    let root = Directory::new()?;
    let current = option("current");
    let default = option("default");
    let mut session = open(&root.db())?;
    session.new_conversation();
    session.select_model_pair(&current, Some(ReasoningEffort::High))?;
    session.set_draft("不能丢失的草稿".into());
    let original = session
        .selected()
        .ok_or("missing selected conversation")?
        .id
        .clone();
    session.set_default_model_pair(&default, None)?;
    assert_eq!(session.history().model.as_ref(), Some(&current.model));
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::High));
    assert_eq!(
        session
            .selected()
            .ok_or("missing selected conversation")?
            .draft,
        "不能丢失的草稿"
    );
    drop(session);
    let mut session = open(&root.db())?;
    assert_eq!(session.history().model.as_ref(), Some(&current.model));
    session.new_conversation();
    let new_chat = session
        .selected()
        .ok_or("missing selected conversation")?
        .id
        .clone();
    assert_eq!(session.request_model(false)?, default.model);
    assert_eq!(session.composer().reasoning, None);
    assert_eq!(
        session.model_picker_preferences().recents,
        vec![current.model.clone()]
    );
    session.select_conversation(&original);
    assert_eq!(session.request_model(false)?, current.model);
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::High));
    assert_eq!(
        session
            .selected()
            .ok_or("missing selected conversation")?
            .draft,
        "不能丢失的草稿"
    );
    session.select_conversation(&new_chat);
    assert_eq!(session.request_model(false)?, default.model);
    session.set_default_model_pair(&current, Some(ReasoningEffort::None))?;
    assert_eq!(session.composer().reasoning, None);
    session.new_conversation();
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::None));
    Ok(())
}

#[test]
fn recents_and_favorites_use_full_identity_and_stable_order() -> Result<(), TestError> {
    let root = Directory::new()?;
    let mut session = open(&root.db())?;
    let first = option("same");
    let mut provider = first.clone();
    provider.model.provider = "other-provider".into();
    let mut endpoint = first.clone();
    endpoint.model.endpoint = "other-endpoint".into();
    let mut protocol = first.clone();
    protocol.model.protocol = Protocol::OpenAiResponses;
    for item in [&first, &provider, &endpoint, &protocol] {
        session.select_model_pair(item, None)?;
        session.toggle_model_favorite(item.model.clone());
    }
    session.toggle_model_favorite(provider.model.clone());
    session.toggle_model_favorite(provider.model.clone());
    assert_eq!(
        session.model_picker_preferences().favorites,
        vec![
            first.model.clone(),
            endpoint.model.clone(),
            protocol.model.clone(),
            provider.model.clone()
        ]
    );
    session.select_model_pair(&first, Some(ReasoningEffort::High))?;
    assert_eq!(
        session.model_picker_preferences().recents,
        vec![
            first.model.clone(),
            protocol.model.clone(),
            endpoint.model.clone(),
            provider.model.clone()
        ]
    );
    for index in 0..12 {
        session.select_model_pair(&option(&format!("model-{index}")), None)?;
    }
    let expected: Vec<_> = (2..12)
        .rev()
        .map(|index| option(&format!("model-{index}")).model)
        .collect();
    assert_eq!(session.model_picker_preferences().recents, expected);
    let favorites = session.model_picker_preferences().favorites.clone();
    drop(session);
    let session = open(&root.db())?;
    assert_eq!(session.model_picker_preferences().favorites, favorites);
    assert_eq!(session.model_picker_preferences().recents, expected);
    Ok(())
}

#[test]
fn unsupported_effort_is_rejected_not_clamped_and_inherited_is_not_off() -> Result<(), TestError> {
    let root = Directory::new()?;
    let mut session = open(&root.db())?;
    let first = option("first");
    session.select_model_pair(&first, Some(ReasoningEffort::High))?;
    let history = serde_json::to_value(session.history())?;
    let preferences = session.model_picker_preferences().clone();
    let mut plain = option("plain");
    plain.settings.reasoning = None;
    assert!(
        session
            .select_model_pair(&plain, Some(ReasoningEffort::High))
            .is_err()
    );
    assert!(
        session
            .select_model_pair(&plain, Some(ReasoningEffort::None))
            .is_err()
    );
    assert!(
        session
            .set_default_model_pair(&plain, Some(ReasoningEffort::Low))
            .is_err()
    );
    assert_eq!(serde_json::to_value(session.history())?, history);
    assert_eq!(session.model_picker_preferences(), &preferences);
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::High));
    session.select_model_pair(&plain, None)?;
    assert_eq!(session.composer().reasoning, None);
    session.select_model_pair(&first, Some(ReasoningEffort::None))?;
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::None));
    session.select_model_pair(&first, None)?;
    assert_eq!(session.composer().reasoning, None);
    Ok(())
}

#[test]
fn selection_failure_rolls_back_model_effort_recents_and_pending_composer() -> Result<(), TestError>
{
    let root = Directory::new()?;
    let mut session = open(&root.db())?;
    let first = option("first");
    let second = option("second");
    session.select_model_pair(&first, Some(ReasoningEffort::High))?;
    let db = Connection::open(root.db())?;
    let before = saved_composer(&db)?;
    reject_composer(&db)?;
    assert!(
        session
            .select_model_pair(&second, Some(ReasoningEffort::Low))
            .is_err()
    );
    assert!(session.save_failed());
    assert_eq!(session.history().model.as_ref(), Some(&first.model));
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::High));
    assert_eq!(
        session.model_picker_preferences().recents,
        vec![first.model.clone()]
    );
    assert_eq!(saved_composer(&db)?, before);
    let saved_model: String =
        db.query_row("SELECT value FROM chat_meta WHERE key='model'", [], |row| {
            row.get(0)
        })?;
    assert_eq!(
        serde_json::from_str::<Option<ModelRef>>(&saved_model)?,
        Some(first.model.clone())
    );
    restore(&db, &mut session)?;
    session.new_conversation();
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::High));
    reject_composer(&db)?;
    session.select_model(second.model);
    assert_eq!(session.history().model.as_ref(), Some(&first.model));
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::High));
    restore(&db, &mut session)?;
    drop(session);
    let session = open(&root.db())?;
    assert_eq!(session.history().model.as_ref(), Some(&first.model));
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::High));
    Ok(())
}

#[test]
fn preference_and_new_chat_failures_leave_existing_state_intact() -> Result<(), TestError> {
    let root = Directory::new()?;
    let mut session = open(&root.db())?;
    let first = option("first");
    let second = option("second");
    session.new_conversation();
    session.select_model_pair(&first, Some(ReasoningEffort::High))?;
    session.set_draft("保留现有对话".into());
    session.toggle_model_favorite(first.model.clone());
    session.set_default_model_pair(&first, None)?;
    let preferences = session.model_picker_preferences().clone();
    let db = Connection::open(root.db())?;
    reject_composer(&db)?;
    session.toggle_model_favorite(second.model.clone());
    assert_eq!(session.model_picker_preferences(), &preferences);
    restore(&db, &mut session)?;
    reject_composer(&db)?;
    assert!(
        session
            .set_default_model_pair(&second, Some(ReasoningEffort::Low))
            .is_err()
    );
    assert_eq!(session.model_picker_preferences(), &preferences);
    restore(&db, &mut session)?;
    session.set_default_model_pair(&second, None)?;
    let history = serde_json::to_value(session.history())?;
    reject_composer(&db)?;
    session.new_conversation();
    assert_eq!(serde_json::to_value(session.history())?, history);
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::High));
    assert_eq!(
        session
            .selected()
            .ok_or("missing selected conversation")?
            .draft,
        "保留现有对话"
    );
    restore(&db, &mut session)?;
    Ok(())
}

#[test]
fn reconcile_prunes_dead_preferences_without_guessing_or_losing_input() -> Result<(), TestError> {
    let root = Directory::new()?;
    let mut session = open(&root.db())?;
    let current = option("current");
    let gone = option("gone");
    session.new_conversation();
    session.select_model_pair(&gone, None)?;
    session.toggle_model_favorite(gone.model.clone());
    session.select_model_pair(&current, Some(ReasoningEffort::High))?;
    session.toggle_model_favorite(current.model.clone());
    session.set_default_model_pair(&gone, None)?;
    session.set_draft("草稿和附件都保留".into());
    session.set_composer(ComposerSettings {
        reasoning: Some(ReasoningEffort::High),
        web_search: true,
        image_mode: true,
        image_model: Some(option("painting").model),
    });
    let attachment = root.0.join("context.txt");
    fs::write(&attachment, "真实附件内容")?;
    session.add_attachment(&attachment)?;
    let attachment_id = session.draft_attachments()[0].id.clone();
    let mut reduced = current.clone();
    reduced
        .settings
        .reasoning
        .as_mut()
        .ok_or("missing expected mutable value")?
        .efforts = vec![ReasoningEffort::Low];
    session.reconcile_model_catalog(std::slice::from_ref(&reduced))?;
    assert_eq!(
        session.model_picker_preferences().favorites,
        vec![current.model.clone()]
    );
    assert_eq!(
        session.model_picker_preferences().recents,
        vec![current.model.clone()]
    );
    assert_eq!(session.model_picker_preferences().default_pair, None);
    assert_eq!(session.history().model.as_ref(), Some(&current.model));
    assert_eq!(session.composer().reasoning, None);
    assert!(session.composer().web_search && session.composer().image_mode);
    assert_eq!(
        session.composer().image_model,
        Some(option("painting").model)
    );
    assert_eq!(session.draft_attachments()[0].id, attachment_id);
    assert_eq!(
        session
            .selected()
            .ok_or("missing selected conversation")?
            .draft,
        "草稿和附件都保留"
    );
    session.set_default_model_pair(&reduced, Some(ReasoningEffort::Low))?;
    reduced.settings.reasoning = None;
    session.reconcile_model_catalog(&[reduced])?;
    assert_eq!(
        session
            .model_picker_preferences()
            .default_pair
            .as_ref()
            .ok_or("missing expected value")?
            .effort,
        None
    );
    session.reconcile_model_catalog(&[])?;
    assert_eq!(session.history().model.as_ref(), Some(&current.model));
    drop(session);
    let session = open(&root.db())?;
    assert_eq!(
        session
            .selected()
            .ok_or("missing selected conversation")?
            .draft,
        "草稿和附件都保留"
    );
    assert_eq!(session.draft_attachments()[0].id, attachment_id);
    assert!(session.composer().web_search && session.composer().image_mode);
    Ok(())
}

#[test]
fn reconciliation_failure_is_atomic_and_legacy_composer_loads_defaults() -> Result<(), TestError> {
    let root = Directory::new()?;
    let mut session = open(&root.db())?;
    let selected = option("selected");
    session.select_model_pair(&selected, Some(ReasoningEffort::High))?;
    session.toggle_model_favorite(selected.model.clone());
    session.set_default_model_pair(&selected, Some(ReasoningEffort::High))?;
    let preferences = session.model_picker_preferences().clone();
    let db = Connection::open(root.db())?;
    reject_composer(&db)?;
    assert!(session.reconcile_model_catalog(&[]).is_err());
    assert_eq!(session.model_picker_preferences(), &preferences);
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::High));
    restore(&db, &mut session)?;
    drop(session);
    db.execute(
        "UPDATE chat_meta SET value=?1 WHERE key='composer'",
        [json!({"drafts":{},"turns":{}}).to_string()],
    )?;
    let session = open(&root.db())?;
    assert_eq!(
        session.model_picker_preferences(),
        &ModelPickerPreferences::default()
    );
    assert_eq!(session.composer().reasoning, None);
    let inherited = ModelPair {
        model: selected.model.clone(),
        effort: None,
    };
    let off = ModelPair {
        model: selected.model,
        effort: Some(ReasoningEffort::None),
    };
    assert_eq!(
        serde_json::from_value::<ModelPair>(serde_json::to_value(&inherited)?)?,
        inherited
    );
    assert_ne!(serde_json::to_value(inherited)?, serde_json::to_value(off)?);
    Ok(())
}

#[test]
fn picker_changes_preserve_sent_snapshot_and_failed_selection_keeps_draft() -> Result<(), TestError>
{
    let root = Directory::new()?;
    let source = option("source");
    let legacy = json!({"schemaVersion":1,"value":{"selected":"conversation","model":source.model,"conversations":[{"id":"conversation","title":"已有对话","draft":"新草稿","turns":[{"id":"turn","input":"已发送问题","attempts":[{"id":"attempt","model":source.model,"text":"已完成回答","status":"completed","error":null}]}]}]}});
    fs::write(
        root.0.join("chat-history.json"),
        serde_json::to_vec(&legacy)?,
    )?;
    drop(open(&root.db())?);
    let db = Connection::open(root.db())?;
    let snapshot = json!({"settings":{"reasoning":"high","web_search":true,"image_mode":true,"image_model":option("original-painting").model},"attachments":[]});
    let composer = json!({"drafts":{},"turns":{"turn":snapshot}});
    db.execute(
        "INSERT OR REPLACE INTO chat_meta(key,value) VALUES('composer',?1)",
        [composer.to_string()],
    )?;
    let mut session = open(&root.db())?;
    let attachment = root.0.join("draft.txt");
    fs::write(&attachment, "尚未发送附件")?;
    session.add_attachment(&attachment)?;
    let attachment_id = session.draft_attachments()[0].id.clone();
    session.set_composer(ComposerSettings {
        reasoning: Some(ReasoningEffort::High),
        web_search: true,
        image_mode: true,
        image_model: Some(option("draft-painting").model),
    });
    reject_composer(&db)?;
    assert!(
        session
            .select_model_pair(&option("failed"), Some(ReasoningEffort::Low))
            .is_err()
    );
    assert_eq!(
        session
            .selected()
            .ok_or("missing selected conversation")?
            .draft,
        "新草稿"
    );
    assert_eq!(session.draft_attachments()[0].id, attachment_id);
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::High));
    assert!(session.composer().web_search && session.composer().image_mode);
    restore(&db, &mut session)?;
    let replacement = option("replacement");
    session.select_model_pair(&replacement, Some(ReasoningEffort::Low))?;
    session.set_default_model_pair(&replacement, None)?;
    session.toggle_model_favorite(replacement.model.clone());
    let mut reduced = replacement.clone();
    reduced.settings.reasoning = None;
    session.reconcile_model_catalog(&[reduced])?;
    assert_eq!(session.request_model(true)?, source.model);
    assert_eq!(
        session.request_painting_model(true)?,
        Some(option("original-painting").model)
    );
    assert_eq!(session.request_model(false)?, replacement.model);
    assert_eq!(saved_composer(&db)?["turns"]["turn"], snapshot);
    assert_eq!(
        session
            .selected()
            .ok_or("missing selected conversation")?
            .draft,
        "新草稿"
    );
    assert_eq!(session.draft_attachments()[0].id, attachment_id);
    assert_eq!(session.composer().reasoning, None);
    assert!(session.composer().web_search && session.composer().image_mode);
    Ok(())
}

#[test]
fn first_draft_keeps_pending_pair_but_explicit_new_applies_default() -> Result<(), TestError> {
    let root = Directory::new()?;
    let default = option("default-a");
    let pending = option("pending-b");
    let mut session = open(&root.db())?;
    session.set_default_model_pair(&default, None)?;
    session.select_model_pair(&pending, Some(ReasoningEffort::High))?;
    assert!(session.selected().is_none());
    drop(session);
    let mut session = open(&root.db())?;
    session.set_draft("首次输入不能重置已选模型".into());
    let first = session
        .selected()
        .ok_or("missing selected conversation")?
        .id
        .clone();
    assert_eq!(session.request_model(false)?, pending.model);
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::High));
    assert_eq!(
        session
            .selected()
            .ok_or("missing selected conversation")?
            .draft,
        "首次输入不能重置已选模型"
    );
    session.new_conversation();
    assert_eq!(session.request_model(false)?, default.model);
    assert_eq!(session.composer().reasoning, None);
    session.select_conversation(&first);
    assert_eq!(session.request_model(false)?, pending.model);
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::High));
    assert_eq!(
        session
            .selected()
            .ok_or("missing selected conversation")?
            .draft,
        "首次输入不能重置已选模型"
    );
    Ok(())
}

#[test]
fn first_draft_without_pending_model_uses_saved_default_pair() -> Result<(), TestError> {
    let root = Directory::new()?;
    let default = option("default-a");
    let mut session = open(&root.db())?;
    session.set_default_model_pair(&default, Some(ReasoningEffort::Low))?;
    assert!(session.history().model.is_none());
    drop(session);
    let mut session = open(&root.db())?;
    session.set_draft("使用默认模型".into());
    assert_eq!(session.request_model(false)?, default.model);
    assert_eq!(session.composer().reasoning, Some(ReasoningEffort::Low));
    assert_eq!(
        session
            .selected()
            .ok_or("missing selected conversation")?
            .draft,
        "使用默认模型"
    );
    Ok(())
}

#[test]
fn implicit_composer_attachment_and_send_keep_pending_model() -> Result<(), TestError> {
    for input in ["composer", "attachment", "send"] {
        let root = Directory::new()?;
        let default = option("default-a");
        let pending = option("pending-b");
        let mut session = open(&root.db())?;
        session.set_default_model_pair(&default, None)?;
        session.select_model_pair(&pending, Some(ReasoningEffort::High))?;
        match input {
            "composer" => session.set_composer(ComposerSettings {
                reasoning: Some(ReasoningEffort::High),
                web_search: true,
                ..Default::default()
            }),
            "attachment" => {
                let path = root.0.join("first.txt");
                fs::write(&path, "首次上传内容")?;
                session.add_attachment(&path)?;
                assert_eq!(session.draft_attachments()[0].name, "first.txt");
            }
            "send" => {
                // 空输入在网络请求前失败，但仍须按待用模型建立输入上下文。
                session.send(lumakit_ai::AiConnection {
                    base_url: "http://127.0.0.1:1".into(),
                    model: pending.model.model.clone(),
                    protocol: pending.model.protocol,
                    api_key: None,
                    settings: pending.settings.clone(),
                });
                assert!(!session.busy());
                assert_eq!(session.notice(), "请输入问题或添加附件。");
            }
            other => return Err(format!("unknown fixture input: {other}").into()),
        }
        assert_eq!(session.request_model(false)?, pending.model);
        assert_eq!(session.composer().reasoning, Some(ReasoningEffort::High));
    }
    Ok(())
}
