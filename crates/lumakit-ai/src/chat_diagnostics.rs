//! 聊天存储与线程边界的安全诊断；不记录底层错误正文。
use super::*;
use crate::history::HistoryError;
use lumakit_observability::{OperationGuard, TraceContext, operation};

pub(super) fn storage_code(error: &HistoryError) -> &'static str {
    match error {
        HistoryError::Io(_) => "CHAT_STORAGE_IO_FAILED",
        HistoryError::Json(_) => "CHAT_STORAGE_JSON_INVALID",
        HistoryError::NotFound => "CHAT_STORAGE_NOT_FOUND",
        HistoryError::Storage(_) => "CHAT_STORAGE_ACCESS_FAILED",
        HistoryError::Migration(_) => "CHAT_STORAGE_MIGRATION_FAILED",
        HistoryError::Database(_) => "CHAT_STORAGE_DATABASE_FAILED",
        HistoryError::InvalidData(_) => "CHAT_STORAGE_DATA_INVALID",
    }
}

pub(super) fn prepare_failure(message: &str, code: &'static str) -> String {
    let (trace, root) = TraceContext::root("chat.prepare", "service");
    root.fail(code);
    format!("{message}（错误码：{code}；诊断 ID：{}）", trace.trace_id())
}

pub(super) fn remove_media(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => tracing::debug!(
            event = "chat.media.cleanup",
            result = "REMOVED",
            "聊天媒体清理完成"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(
                event = "chat.media.cleanup",
                result = "ALREADY_ABSENT",
                "聊天媒体无需清理"
            );
        }
        Err(error) => {
            tracing::warn!(event = "chat.media.cleanup", code = "CHAT_MEDIA_CLEANUP_FAILED", kind = ?error.kind(), os_code = error.raw_os_error(), "聊天媒体清理失败")
        }
    }
}

impl ChatSession {
    pub(super) fn save_request(
        &mut self,
        trace: &TraceContext,
        root: OperationGuard,
    ) -> Option<OperationGuard> {
        let saved = {
            let _entered = root.span().enter();
            self.save_with_trace(Some(trace))
        };
        if saved {
            Some(root)
        } else {
            root.fail(self.save_code);
            None
        }
    }

    pub(super) fn save_with_trace(&mut self, trace: Option<&TraceContext>) -> bool {
        // 持续失败只记录 DEBUG 实测耗时，不为每个运行事件重复报警。
        let started = std::time::Instant::now();
        let (context, guard) = if let Some(trace) = trace {
            (
                Some(trace.clone()),
                Some(operation(trace, "chat.persistence.save", "repository")),
            )
        } else if !self.save_error {
            let (trace, root) = TraceContext::root("chat.persistence.save", "repository");
            (Some(trace), Some(root))
        } else {
            tracing::debug!(
                event = "chat.persistence.save",
                result = "ENTRY",
                code = self.save_code,
                "继续保存尚未恢复的聊天记录"
            );
            (None, None)
        };
        self.composer_data.default_composer = self.default_composer.clone();
        match store::save_composer(&mut self.connection, &self.history, &self.composer_data) {
            Ok(()) => {
                if let Some(guard) = guard {
                    guard.finish("SAVED");
                } else {
                    tracing::debug!(
                        event = "chat.persistence.save",
                        result = "SAVED",
                        recovered = true,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "聊天记录保存已恢复"
                    );
                }
                self.save_error = false;
                self.notice.clear();
                true
            }
            Err(error) => {
                let code = storage_code(&error);
                if let Some(guard) = guard {
                    guard.fail(code);
                } else {
                    tracing::debug!(
                        event = "chat.persistence.save",
                        result = "FAILED",
                        code,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "聊天记录仍未保存；保留内存记录"
                    );
                }
                if !self.save_error
                    && let Some(context) = context
                {
                    let trace_id = context.trace_id();
                    self.notice = format!(
                        "聊天记录保存失败。内存中的问题、部分回答和草稿仍保留；请检查目录权限或磁盘空间后重试保存。保存成功前禁止更改和发送。（错误码：{code}；诊断 ID：{trace_id}）"
                    );
                }
                self.save_code = code;
                self.save_error = true;
                false
            }
        }
    }
}
