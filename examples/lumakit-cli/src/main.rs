//! 无界面消费共享库；默认输出演示状态，显式设置服务地址后才发起模型请求。
use lumakit_ai::{
    AiConnection, AiError, AiHost, Approval, RunRequest, ToolCall, ToolFuture, ToolOutput, ToolSpec,
};
use lumakit_observability::{LogHandle, LogLevel, TraceContext, operation};
use std::{io::Write as _, sync::Arc};
use zeroize::Zeroizing;

struct Host;
impl AiHost for Host {
    fn select_model(&self, request: &RunRequest) -> Result<lumakit_ai::ModelRef, AiError> {
        request.model.clone().ok_or(AiError::ModelNotConfigured)
    }
    fn system_prompt(&self, _: &RunRequest) -> Result<Option<String>, AiError> {
        Ok(Some("只使用白名单桌面工具。".to_owned()))
    }
    fn tools(&self, _: &RunRequest) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "desktop.read_status".into(),
            description: "读取演示桌面状态".into(),
            parameters: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true,
            requires_approval: false,
        }]
    }
    fn authorize(&self, _: &ToolCall, default: Approval) -> Approval {
        default
    }
    fn connection(&self, model: &lumakit_ai::ModelRef) -> Result<AiConnection, AiError> {
        let base_url = std::env::var("LUMAKIT_AI_URL").map_err(|_| AiError::ModelNotConfigured)?;
        Ok(AiConnection {
            base_url,
            protocol: model.protocol,
            api_key: std::env::var("LUMAKIT_AI_KEY").ok().map(Zeroizing::new),
            model: model.model.clone(),
            settings: Default::default(),
        })
    }
    fn call_tool(&self, _: ToolCall, _: tokio_util::sync::CancellationToken) -> ToolFuture {
        Box::pin(async {
            Ok(ToolOutput {
                text: "demo status".into(),
                is_error: false,
            })
        })
    }
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = lumakit_support::data_root("lumakit-cli")?;
    let logs = LogHandle::init(&root.join("logs"), LogLevel::Debug, 5 * 1024 * 1024)?;
    tracing::info!(event = "startup", "lumakit.cli.started");
    let request = RunRequest {
        input: "读取状态".into(),
        model: Some(lumakit_ai::ModelRef {
            provider: "demo".into(),
            endpoint: "loopback".into(),
            model: "demo".into(),
            protocol: lumakit_ai::Protocol::OpenAiChat,
        }),
        purpose: "desktop".into(),
    };
    let (trace, root) = TraceContext::root("cli.run", "command");
    let _entered = root.span().clone().entered();
    let tools = operation(&trace, "cli.tools.list", "service");
    let runtime = lumakit_ai::AiRuntime::new(Arc::new(Host))?;
    let count = runtime.available_tools(&request).len();
    tools.finish("SUCCESS");
    // 显式写 stdout：写失败（如管道关闭）作为错误返回，而不是像 print! 那样 panic。
    // 回调须为 Send，不持有 StdoutLock，每次写入时再取句柄。
    let mut out = std::io::stdout();
    writeln!(out, "LumaKit CLI | tools={count}")?;
    if std::env::var_os("LUMAKIT_AI_URL").is_some() {
        let mut write_error = None;
        let run = runtime
            .run(
                request,
                Some(&trace),
                tokio_util::sync::CancellationToken::new(),
                |delta| {
                    if write_error.is_none() {
                        let mut out = std::io::stdout();
                        write_error = out
                            .write_all(delta.as_bytes())
                            .and_then(|()| out.flush())
                            .err();
                    }
                },
            )
            .await;
        match &run {
            Ok(_) => root.finish("SUCCESS"),
            Err(AiError::Cancelled) => root.cancel("user_cancelled"),
            Err(AiError::Timeout { limit_ms, .. }) => root.timeout(*limit_ms),
            Err(error) => root.fail(error.code()),
        }
        if let Some(error) = write_error {
            return Err(error.into());
        }
        writeln!(out, "\nAI text length={}", run?.text.chars().count())?;
    } else {
        root.finish("SKIPPED_NO_URL");
    }
    writeln!(out, "log={}", logs.status().path.display())?;
    Ok(())
}

// `live-provider` is intentionally a marker feature: credentials stay in environment variables.
