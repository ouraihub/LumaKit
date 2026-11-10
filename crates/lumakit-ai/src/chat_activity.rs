//! 已保存回答的搜索来源与工具结果修改。
use super::*;

impl ChatSession {
    /// 标记指定回答开始联网搜索。
    ///
    /// # Errors
    /// Returns invalid query/provider, missing attempt, busy session or persistence errors.
    pub fn search_start(
        &mut self,
        conversation_id: &str,
        turn_id: &str,
        attempt_id: &str,
        query: &str,
        provider: &str,
    ) -> Result<(), String> {
        if !self.mutable() {
            return Err("当前聊天不可修改。".into());
        }
        if query.trim().is_empty() || provider.trim().is_empty() {
            return Err("搜索查询或提供方为空。".into());
        }
        let before = self.history.clone();
        let attempt = self.find_attempt_mut(conversation_id, turn_id, attempt_id)?;
        attempt.search = SearchState {
            status: SearchStatus::Searching,
            query: Some(query.into()),
            provider: Some(provider.into()),
            hits: Vec::new(),
            error: None,
        };
        if self.save() {
            Ok(())
        } else {
            self.history = before;
            Err(self.notice.clone())
        }
    }

    /// 持久化搜索结果；来源保持在正文之外。
    ///
    /// # Errors
    /// Returns invalid search results, missing attempt, busy session or persistence errors.
    pub fn search_result(
        &mut self,
        conversation_id: &str,
        turn_id: &str,
        attempt_id: &str,
        result: crate::search::SearchResult,
    ) -> Result<(), String> {
        if !self.mutable() {
            return Err("当前聊天不可修改。".into());
        }
        let mut result =
            crate::search::process_result(result, &crate::search::SearchPreferences::default());
        for hit in &result.hits {
            crate::search::validate_search_url(&hit.url)
                .map_err(|_| "搜索来源 URL 无效。".to_owned())?;
        }
        let before = self.history.clone();
        let attempt = self.find_attempt_mut(conversation_id, turn_id, attempt_id)?;
        attempt.search = SearchState {
            status: SearchStatus::Complete,
            query: Some(result.query),
            provider: Some(result.provider),
            hits: std::mem::take(&mut result.hits),
            error: None,
        };
        if self.save() {
            Ok(())
        } else {
            self.history = before;
            Err(self.notice.clone())
        }
    }

    /// 记录搜索失败，不伪装成无结果。
    ///
    /// # Errors
    /// Returns missing attempt, busy session or persistence errors.
    pub fn search_failed(
        &mut self,
        conversation_id: &str,
        turn_id: &str,
        attempt_id: &str,
        error: &str,
    ) -> Result<(), String> {
        self.finish_search(
            conversation_id,
            turn_id,
            attempt_id,
            SearchStatus::Failed,
            Some(error.into()),
        )
    }

    /// 记录搜索取消，不伪装成无结果。
    ///
    /// # Errors
    /// Returns missing attempt, busy session or persistence errors.
    pub fn search_cancelled(
        &mut self,
        conversation_id: &str,
        turn_id: &str,
        attempt_id: &str,
    ) -> Result<(), String> {
        self.finish_search(
            conversation_id,
            turn_id,
            attempt_id,
            SearchStatus::Cancelled,
            None,
        )
    }

    fn finish_search(
        &mut self,
        conversation_id: &str,
        turn_id: &str,
        attempt_id: &str,
        status: SearchStatus,
        error: Option<String>,
    ) -> Result<(), String> {
        if !self.mutable() {
            return Err("当前聊天不可修改。".into());
        }
        let before = self.history.clone();
        let attempt = self.find_attempt_mut(conversation_id, turn_id, attempt_id)?;
        attempt.search.status = status;
        attempt.search.error = error;
        if self.save() {
            Ok(())
        } else {
            self.history = before;
            Err(self.notice.clone())
        }
    }
    /// 记录模型发起的工具调用；调用状态与回答正文分开保存。
    ///
    /// # Errors
    /// Returns invalid tool identity/arguments, missing attempt, busy session or persistence errors.
    pub fn tool_start(
        &mut self,
        conversation_id: &str,
        turn_id: &str,
        attempt_id: &str,
        call: crate::ToolCall,
    ) -> Result<(), String> {
        if !self.mutable() {
            return Err("当前聊天不可修改。".into());
        }
        if call.id.trim().is_empty() || call.name.trim().is_empty() || !call.arguments.is_object() {
            return Err("工具调用无效。".into());
        }
        let before = self.history.clone();
        let attempt = self.find_attempt_mut(conversation_id, turn_id, attempt_id)?;
        if attempt.tool_events.iter().any(|event| event.id == call.id) {
            return Err("工具调用标识重复。".into());
        }
        attempt.tool_events.push(crate::ToolEvent {
            id: call.id,
            name: call.name,
            arguments: call.arguments,
            approval: None,
            result: None,
            is_error: false,
            error: None,
        });
        if self.save() {
            Ok(())
        } else {
            self.history = before;
            Err(self.notice.clone())
        }
    }
    /// 保存工具授权决定。
    ///
    /// # Errors
    /// Returns missing tool/attempt, busy session or persistence errors.
    pub fn tool_approval(
        &mut self,
        conversation_id: &str,
        turn_id: &str,
        attempt_id: &str,
        call_id: &str,
        approval: crate::Approval,
    ) -> Result<(), String> {
        self.update_tool_event(conversation_id, turn_id, attempt_id, call_id, |event| {
            event.approval = Some(approval)
        })
    }
    /// 保存工具输出，包括工具自身报告的业务错误。
    ///
    /// # Errors
    /// Returns missing tool/attempt, busy session or persistence errors.
    pub fn tool_result(
        &mut self,
        conversation_id: &str,
        turn_id: &str,
        attempt_id: &str,
        call_id: &str,
        output: crate::ToolOutput,
    ) -> Result<(), String> {
        self.update_tool_event(conversation_id, turn_id, attempt_id, call_id, |event| {
            event.result = Some(output.text);
            event.is_error = output.is_error;
            event.error = None;
        })
    }
    /// 保存未知工具、拒绝或执行失败的错误，不伪装成成功结果。
    ///
    /// # Errors
    /// Returns missing tool/attempt, busy session or persistence errors.
    pub fn tool_error(
        &mut self,
        conversation_id: &str,
        turn_id: &str,
        attempt_id: &str,
        call_id: &str,
        error: &str,
    ) -> Result<(), String> {
        if error.trim().is_empty() {
            return Err("工具错误为空。".into());
        }
        self.update_tool_event(conversation_id, turn_id, attempt_id, call_id, |event| {
            event.error = Some(error.into());
            event.is_error = true;
        })
    }
    fn update_tool_event(
        &mut self,
        conversation_id: &str,
        turn_id: &str,
        attempt_id: &str,
        call_id: &str,
        update: impl FnOnce(&mut crate::ToolEvent),
    ) -> Result<(), String> {
        if !self.mutable() {
            return Err("当前聊天不可修改。".into());
        }
        let before = self.history.clone();
        let attempt = self.find_attempt_mut(conversation_id, turn_id, attempt_id)?;
        let event = attempt
            .tool_events
            .iter_mut()
            .find(|event| event.id == call_id)
            .ok_or("工具调用不存在。")?;
        update(event);
        if self.save() {
            Ok(())
        } else {
            self.history = before;
            Err(self.notice.clone())
        }
    }
}
