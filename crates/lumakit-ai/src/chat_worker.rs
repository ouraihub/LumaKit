//! 请求快照执行、响应边界及入口日志终态。
use super::*;

/// Keeps ownership until the spawned thread starts; a rejected spawn drops its closure here.
pub(super) struct PendingRoot(Option<lumakit_observability::OperationGuard>);

impl PendingRoot {
    pub(super) fn new(root: lumakit_observability::OperationGuard) -> Self {
        Self(Some(root))
    }

    pub(super) fn take(mut self) -> Result<lumakit_observability::OperationGuard, &'static str> {
        self.0.take().ok_or("聊天请求诊断入口未交接。")
    }
}

impl Drop for PendingRoot {
    fn drop(&mut self) {
        if let Some(root) = self.0.take() {
            root.fail("CHAT_WORKER_START_FAILED");
        }
    }
}

/// 运行一次聊天请求并按实际结果结束入口操作，失败写稳定错误码，不写正文。
#[allow(
    clippy::too_many_arguments,
    reason = "私有 worker 入口，参数来自同一次请求快照"
)]
pub(super) fn run_session_worker(
    connection: AiConnection,
    painting: Option<AiConnection>,
    painting_model: Option<ModelRef>,
    messages: Vec<ChatMessage>,
    trace: (
        &lumakit_observability::TraceContext,
        lumakit_observability::OperationGuard,
    ),
    cancel: CancellationToken,
    tx: &EventSender,
    wake: &Arc<dyn Fn() + Send + Sync>,
    media_root: PathBuf,
) -> Result<crate::AiRun, (TurnStatus, String)> {
    let (context, root) = trace;
    let (result, code) = session_result(
        connection,
        painting,
        painting_model,
        messages,
        Some(context),
        cancel,
        tx,
        wake,
        media_root,
    );
    match (&result, code) {
        (Ok(_), _) => root.finish("SUCCESS"),
        (Err(_), Some(Ok(limit_ms))) => root.timeout(limit_ms),
        (Err((TurnStatus::Cancelled, _)), _) => root.cancel("user_cancelled"),
        (Err(_), Some(Err(code))) => root.fail(code),
        (Err(_), None) => root.fail("AI_REQUEST_FAILED"),
    }
    result
}

/// 结果与日志结论：`Some(Ok(limit_ms))` 为超时，`Some(Err(code))` 为稳定错误码。
type Outcome = (
    Result<crate::AiRun, (TurnStatus, String)>,
    Option<Result<u64, &'static str>>,
);

#[allow(
    clippy::too_many_arguments,
    reason = "私有 worker 入口，参数来自同一次请求快照"
)]
fn session_result(
    connection: AiConnection,
    painting: Option<AiConnection>,
    painting_model: Option<ModelRef>,
    messages: Vec<ChatMessage>,
    trace: Option<&lumakit_observability::TraceContext>,
    cancel: CancellationToken,
    tx: &EventSender,
    wake: &Arc<dyn Fn() + Send + Sync>,
    media_root: PathBuf,
) -> Outcome {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            return (
                Err((
                    TurnStatus::Failed,
                    "无法启动聊天网络运行时，请重试。".into(),
                )),
                Some(Err("AI_RUNTIME_UNAVAILABLE")),
            );
        }
    };
    let mut bytes = 0usize;
    let mut overflow = false;
    let host = generation::PaintingHost {
        model: ModelRef {
            provider: "snapshot".into(),
            endpoint: "snapshot".into(),
            model: connection.model.clone(),
            protocol: connection.protocol,
        },
        chat: connection,
        painting,
        painting_model,
        root: media_root,
        tx: tx.clone(),
        wake: wake.clone(),
        executions: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        trace: trace.cloned(),
    };
    let engine = match crate::AiRuntime::new(Arc::new(host)) {
        Ok(engine) => engine,
        Err(error) => {
            return (
                Err((TurnStatus::Failed, error.to_string())),
                Some(Err(error.code())),
            );
        }
    };
    let result = runtime.block_on(engine.run_with_history(
        crate::RunRequest {
            input: String::new(),
            model: None,
            purpose: "chat".into(),
        },
        messages,
        trace,
        cancel.clone(),
        |text| {
            if overflow {
                return;
            }
            if text.len() > MAX_TEXT.saturating_sub(bytes) {
                overflow = true;
                cancel.cancel();
                return;
            }
            bytes += text.len();
            let mut remaining = text.as_str();
            while !remaining.is_empty() {
                let mut end = remaining.len().min(16 * 1024);
                while !remaining.is_char_boundary(end) {
                    end -= 1;
                }
                // 有界背压不丢字；Drop 会继续排空，因此取消后也不会卡在 join。
                if tx.send(Event::Delta(remaining[..end].to_owned())).is_err() {
                    cancel.cancel();
                    return;
                }
                wake();
                remaining = &remaining[end..];
            }
        },
        |event| {
            if tx.send(Event::Runtime(event)).is_err() {
                cancel.cancel();
            }
            wake();
        },
    ));
    if overflow {
        return (
            Err((
                TurnStatus::Failed,
                "回答超过 1 MiB，已停止；保留已有内容。".into(),
            )),
            Some(Err("AI_REPLY_TOO_LARGE")),
        );
    }
    match result {
        Ok(result) if result.text.trim().is_empty() => (
            Err((
                TurnStatus::Failed,
                "服务返回了空回答，请重试；空回答不会用于后续上下文。".into(),
            )),
            Some(Err("AI_EMPTY_REPLY")),
        ),
        Ok(result) if result.text.len() <= MAX_TEXT => (Ok(result), None),
        Ok(_) => (
            Err((TurnStatus::Failed, "回答超过 1 MiB，已停止。".into())),
            Some(Err("AI_REPLY_TOO_LARGE")),
        ),
        Err(AiError::Cancelled) => (
            Err((
                TurnStatus::Cancelled,
                "已停止；部分回答不会用于后续上下文。".into(),
            )),
            None,
        ),
        Err(error @ AiError::Timeout { limit_ms, .. }) => (
            Err((TurnStatus::Failed, format!("请求失败：{error}"))),
            Some(Ok(limit_ms)),
        ),
        Err(error) => {
            let code = error.code();
            (
                Err((TurnStatus::Failed, format!("请求失败：{error}"))),
                Some(Err(code)),
            )
        }
    }
}
