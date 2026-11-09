//! 无界面消费共享库；默认输出演示状态，显式设置服务地址后才发起模型请求。
use lumakit_ai::{
    AiConnection, AiError, AiHost, Approval, RunRequest, ToolCall, ToolFuture, ToolOutput, ToolSpec,
};
use lumakit_observability::{LogHandle, LogLevel};
use std::sync::Arc;
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
    let runtime = lumakit_ai::AiRuntime::new(Arc::new(Host))?;
    println!(
        "LumaKit CLI | tools={}",
        runtime.available_tools(&request).len()
    );
    if std::env::var_os("LUMAKIT_AI_URL").is_some() {
        let run = runtime
            .run(
                request,
                tokio_util::sync::CancellationToken::new(),
                |delta| print!("{delta}"),
            )
            .await?;
        println!("\nAI text length={}", run.text.chars().count());
    }
    println!("log={}", logs.status().path.display());
    Ok(())
}

// `live-provider` is intentionally a marker feature: credentials stay in environment variables.
