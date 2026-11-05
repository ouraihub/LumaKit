//! 对话与候选编辑；每次修改仍由 ChatSession 原子保存。
use super::*;

impl ChatSession {
    /// 显式新建对话；已保存默认模型对存在时优先应用，不沿用待用模型选择。
    /// 保存失败时撤销新对话和模型、输入控制的全部修改。
    pub fn new_conversation(&mut self) {
        self.create_conversation(true);
    }

    /// 首次输入隐式建立对话，保留待用模型与推理控制；未选模型时应用默认模型对。
    pub(super) fn ensure_conversation(&mut self) {
        if self.selected().is_none() { self.create_conversation(self.history.model.is_none()); }
    }

    fn create_conversation(&mut self, use_default_pair: bool) {
        if !self.mutable() { return; }
        let id = match new_id() {
            Ok(id) => id,
            Err(_) => { self.notice = "无法生成对话标识，请重试。".into(); return; }
        };
        let _ = self.update_model_picker(|session| {
            session.remember_conversation_model();
            let mut settings = session.default_composer.clone();
            if let Some(pair) = session.composer_data.model_picker.default_pair.as_ref().filter(|_| use_default_pair) {
                session.history.model = Some(pair.model.clone());
                settings.reasoning = pair.effort;
            }
            session.history.selected = Some(id.clone());
            session.composer_data.drafts.insert(id.clone(), composer::InputSnapshot { settings, attachments: Vec::new() });
            session.history.conversations.push(Conversation {
                id, title: "新对话".into(), draft: String::new(), turns: Vec::new(),
            });
            session.remember_conversation_model();
        });
    }
    /// 空闲且无保存错误时切换到存在的对话，并保存选择。
    pub fn select_conversation(&mut self, id: &str) {
        if !self.mutable() || !self.history.conversations.iter().any(|c| c.id == id)
            || self.history.selected.as_deref() == Some(id) { return; }
        let _ = self.update_model_picker(|session| {
            session.remember_conversation_model();
            session.history.selected = Some(id.into());
            if let Some(model) = session.composer_data.conversation_models.get(id) {
                session.history.model = Some(model.clone());
            }
        });
    }
    /// 空闲且无保存错误时删除指定对话并保存；删除当前项后选择最后一项。
    pub fn delete_conversation(&mut self, id: &str) {
        if !self.mutable() { return; }
        let Some(index) = self.history.conversations.iter().position(|c| c.id == id) else { return; };
        let before_history = self.history.clone();
        let before_composer = self.composer_data.clone();
        self.composer_data.drafts.remove(id);
        self.composer_data.conversation_models.remove(id);
        for turn in &self.history.conversations[index].turns { self.composer_data.turns.remove(&turn.id); }
        self.history.conversations.remove(index);
        if self.history.selected.as_deref() == Some(id) {
            self.history.selected = self.history.conversations.last().map(|c| c.id.clone());
            if let Some(model) = self.history.selected.as_ref().and_then(|id| self.composer_data.conversation_models.get(id)) {
                self.history.model = Some(model.clone());
            }
        }
        if !self.save() { self.history = before_history; self.composer_data = before_composer; }
    }
    /// 编辑指定回答，使用期望正文做 CAS 冲突保护。
    pub fn edit_attempt(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str, expected_text: &str, next_text: &str) -> Result<(), String> {
        if !self.mutable() { return Err("当前聊天不可修改。".into()); }
        if next_text.trim().is_empty() || next_text.len() > MAX_TEXT { return Err("回答正文为空或超过 1 MiB。".into()); }
        let before = self.history.clone();
        let attempt = self.find_attempt_mut(conversation_id, turn_id, attempt_id)?;
        if attempt.text != expected_text { return Err("回答已被其他操作修改。".into()); }
        attempt.text = next_text.into();
        if self.save() { Ok(()) } else { self.history = before; Err(self.notice.clone()) }
    }

    /// 删除指定回答候选，不删除同问题的其它候选。
    pub fn delete_attempt(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str) -> Result<(), String> {
        if !self.mutable() { return Err("当前聊天不可修改。".into()); }
        let before = self.history.clone();
        let turn = self.find_turn_mut(conversation_id, turn_id)?;
        let Some(index) = turn.attempts.iter().position(|attempt| attempt.id == attempt_id) else { return Err("回答不存在。".into()); };
        turn.attempts.remove(index);
        if turn.selected_attempt.as_deref() == Some(attempt_id) { turn.selected_attempt = turn.attempts.last().map(|attempt| attempt.id.clone()); }
        if self.save() { Ok(()) } else { self.history = before; Err(self.notice.clone()) }
    }

    /// 选择指定候选作为该问题的后续上下文来源。
    pub fn select_attempt(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str) -> Result<(), String> {
        if !self.mutable() { return Err("当前聊天不可修改。".into()); }
        let before = self.history.clone();
        let turn = self.find_turn_mut(conversation_id, turn_id)?;
        if !turn.attempts.iter().any(|attempt| attempt.id == attempt_id) { return Err("回答不存在。".into()); }
        turn.selected_attempt = Some(attempt_id.into());
        if self.save() { Ok(()) } else { self.history = before; Err(self.notice.clone()) }
    }

    fn find_turn_mut(&mut self, conversation_id: &str, turn_id: &str) -> Result<&mut Turn, String> {
        let conversation = self.history.conversations.iter_mut().find(|conversation| conversation.id == conversation_id).ok_or("对话不存在。")?;
        conversation.turns.iter_mut().find(|turn| turn.id == turn_id).ok_or("问题不存在。".into())
    }
    pub(super) fn find_attempt_mut(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str) -> Result<&mut Attempt, String> {
        let turn = self.find_turn_mut(conversation_id, turn_id)?;
        turn.attempts.iter_mut().find(|attempt| attempt.id == attempt_id).ok_or_else(|| "回答不存在。".into())
    }
    /// 复制指定候选之前的线性历史到新对话，并重新生成全部 ID。
    pub fn fork_attempt(&mut self, conversation_id: &str, turn_id: &str, attempt_id: &str) -> Result<String, String> {
        if !self.mutable() { return Err("当前聊天不可修改。".into()); }
        let before_history = self.history.clone();
        let before_composer = self.composer_data.clone();
        let mut snapshots = Vec::new();
        let source = self.history.conversations.iter().find(|conversation| conversation.id == conversation_id).ok_or("对话不存在。")?;
        let turn_index = source.turns.iter().position(|turn| turn.id == turn_id).ok_or("问题不存在。")?;
        let _source_attempt = source.turns[turn_index].attempts.iter().find(|attempt| attempt.id == attempt_id).ok_or("回答不存在。")?;
        let mut conversation = Conversation { id: new_id().map_err(|_| "无法生成对话标识。")?, title: format!("{}（分支）", source.title), draft: String::new(), turns: Vec::new() };
        for source_turn in source.turns.iter().take(turn_index + 1) {
            let mut turn = Turn { id: new_id().map_err(|_| "无法生成问题标识。")?, input: source_turn.input.clone(), attempts: Vec::new(), selected_attempt: None };
            for source_attempt in &source_turn.attempts {
                if source_turn.id == turn_id && source_attempt.id != attempt_id { continue; }
                turn.attempts.push(Attempt { id: new_id().map_err(|_| "无法生成回答标识。")?, ..source_attempt.clone() });
            }
            turn.selected_attempt = turn.attempts.last().map(|attempt| attempt.id.clone());
            if let Some(snapshot) = self.composer_data.turns.get(&source_turn.id) { snapshots.push((turn.id.clone(), snapshot.clone())); }
            conversation.turns.push(turn);
        }
        let id = conversation.id.clone();
        self.composer_data.turns.extend(snapshots);
        if let Some(draft) = self.composer_data.drafts.get(conversation_id).cloned() { self.composer_data.drafts.insert(id.clone(), composer::InputSnapshot { attachments: Vec::new(), ..draft }); }
        self.remember_conversation_model();
        if let Some(model) = self.composer_data.conversation_models.get(conversation_id).cloned() {
            self.history.model = Some(model.clone());
            self.composer_data.conversation_models.insert(id.clone(), model);
        }
        self.history.selected = Some(id.clone());
        self.history.conversations.push(conversation);
        if self.save() { Ok(id) } else { self.history = before_history; self.composer_data = before_composer; Err(self.notice.clone()) }
    }

    /// 复制完整对话并重新生成全部 ID。
    pub fn copy_conversation(&mut self, conversation_id: &str) -> Result<String, String> {
        if !self.mutable() { return Err("当前聊天不可修改。".into()); }
        let before_history = self.history.clone();
        let before_composer = self.composer_data.clone();
        let source = self.history.conversations.iter().find(|conversation| conversation.id == conversation_id).ok_or("对话不存在。")?.clone();
        let mut copy = Conversation { id: new_id().map_err(|_| "无法生成对话标识。")?, title: format!("{}（副本）", source.title), draft: source.draft, turns: Vec::new() };
        for source_turn in source.turns {
            let mut turn = Turn { id: new_id().map_err(|_| "无法生成问题标识。")?, input: source_turn.input, attempts: Vec::new(), selected_attempt: None };
            for source_attempt in source_turn.attempts { turn.attempts.push(Attempt { id: new_id().map_err(|_| "无法生成回答标识。")?, ..source_attempt }); }
            turn.selected_attempt = turn.attempts.last().map(|attempt| attempt.id.clone());
            if let Some(snapshot) = self.composer_data.turns.get(&source_turn.id).cloned() { self.composer_data.turns.insert(turn.id.clone(), snapshot); }
            copy.turns.push(turn);
        }
        let id = copy.id.clone();
        if let Some(draft) = self.composer_data.drafts.get(conversation_id).cloned() { self.composer_data.drafts.insert(id.clone(), draft); }
        self.remember_conversation_model();
        if let Some(model) = self.composer_data.conversation_models.get(conversation_id).cloned() {
            self.history.model = Some(model.clone());
            self.composer_data.conversation_models.insert(id.clone(), model);
        }
        self.history.selected = Some(id.clone());
        self.history.conversations.push(copy);
        if self.save() { Ok(id) } else { self.history = before_history; self.composer_data = before_composer; Err(self.notice.clone()) }
    }
    /// 空闲且无保存错误时保存草稿；首次输入建立对话并保留待用模型与推理控制。
    pub fn set_draft(&mut self, text: String) {
        if !self.mutable() { return; }
        self.ensure_conversation();
        if !self.mutable() { return; }
        if let Some(index) = self.selected_index() {
            self.history.conversations[index].draft = text;
            self.save();
        }
    }
}
