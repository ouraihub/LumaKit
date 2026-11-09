//! 绘图工具宿主与有界 OpenAI 兼容图片客户端；不向外部图片 URL 发送密钥。
use super::{Event, EventSender, ImageRef};
use crate::{
    AiConnection, AiError, AiHost, AiRun, Approval, ModelRef, RunRequest, ToolCall, ToolFuture,
    ToolOutput, ToolSpec, images,
    model_config::{ModelType, RoundUsage, TokenUsage},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

/// 单张生成图片 URL 下载的时限。
const DOWNLOAD_LIMIT_MS: u64 = 60_000;

#[derive(Debug, Default, Deserialize, Serialize)]
enum ImageSize {
    #[default]
    #[serde(rename = "1024x1024")]
    Square,
    #[serde(rename = "1024x1536")]
    Portrait,
    #[serde(rename = "1536x1024")]
    Landscape,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ImageRequest {
    prompt: String,
    #[serde(default)]
    size: ImageSize,
}
impl ImageRequest {
    fn parse(value: serde_json::Value) -> Result<Self, AiError> {
        let mut request: Self = serde_json::from_value(value).map_err(|_| {
            error("绘图参数无效：prompt 为文字，size 仅支持 1024x1024、1024x1536、1536x1024。")
        })?;
        if request.prompt.trim().len() != request.prompt.len() {
            request.prompt = request.prompt.trim().to_owned();
        }
        if request.prompt.is_empty() || request.prompt.chars().count() > 4000 {
            return Err(error("绘图提示必须包含 1 至 4000 个 Unicode 字符。"));
        }
        Ok(request)
    }
}

pub(super) struct PaintingHost {
    pub model: ModelRef,
    pub chat: AiConnection,
    pub painting: Option<AiConnection>,
    pub painting_model: Option<ModelRef>,
    pub root: PathBuf,
    pub tx: EventSender,
    pub wake: Arc<dyn Fn() + Send + Sync>,
    pub executions: Arc<AtomicUsize>,
    /// 所属聊天请求的链路；图片生成与下载在其下记录生命周期。
    pub trace: Option<lumakit_observability::TraceContext>,
}
impl AiHost for PaintingHost {
    fn select_model(&self, _: &RunRequest) -> Result<ModelRef, AiError> {
        Ok(self.model.clone())
    }
    fn connection(&self, _: &ModelRef) -> Result<AiConnection, AiError> {
        Ok(self.chat.clone())
    }
    fn system_prompt(&self, _: &RunRequest) -> Result<Option<String>, AiError> {
        Ok(None)
    }
    fn tools(&self, _: &RunRequest) -> Vec<ToolSpec> {
        painting_tools(self.painting.is_some())
    }
    fn authorize(&self, _: &ToolCall, default: Approval) -> Approval {
        default
    }
    fn call_tool(&self, call: ToolCall, cancel: CancellationToken) -> ToolFuture {
        let painting = self.painting.clone();
        let painting_model = self.painting_model.clone();
        let root = self.root.clone();
        let tx = self.tx.clone();
        let wake = self.wake.clone();
        let executions = self.executions.clone();
        let trace = self.trace.clone();
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(AiError::Cancelled);
            }
            if call.name != "generate_image" {
                return Err(error("未知绘图工具。"));
            }
            if executions.fetch_add(1, Ordering::Relaxed) >= 2 {
                return Err(error(
                    "每次回复最多执行两次图片生成（失败也计入）；未再次请求图片接口。",
                ));
            }
            let request = ImageRequest::parse(call.arguments)?;
            tracing::info!(event = "ai.image.tool", call_id = %call.id, prompt_chars = request.prompt.chars().count(), size = ?request.size, "执行绘图工具");
            let painting = painting.ok_or_else(|| error("尚未配置绘图连接。"))?;
            let op = trace.as_ref().map(|trace| {
                lumakit_observability::operation(trace, "ai.image.generate", "external")
            });
            let result = generate(painting, request, &root, &cancel).await;
            if let Some(op) = op {
                crate::end_operation(op, &result, "SUCCESS");
            }
            let (mut run, images) = result?;
            for round in &mut run.rounds {
                round.model = painting_model.clone();
            }
            let text = serde_json::json!({"message":format!("Generated {} image(s).", images.len()),"images":images}).to_string();
            tx.send(Event::Generated {
                images,
                rounds: run.rounds,
            })
            .map_err(|_| error("绘图会话已关闭。"))?;
            wake();
            Ok(ToolOutput {
                text,
                is_error: false,
            })
        })
    }
}
pub(super) fn painting_tools(enabled: bool) -> Vec<ToolSpec> {
    if !enabled {
        return Vec::new();
    }
    vec![ToolSpec {
        name: "generate_image".into(), description: "仅当用户要求图片、绘画、插图或照片时生成一张图片；描述完整画面、构图与氛围，每次回复最多调用两次。失败时如实报告，不得声称已生成，不要重试同一失败请求。".into(),
        parameters: serde_json::json!({"type":"object","properties":{"prompt":{"type":"string","minLength":1,"maxLength":4000},"size":{"type":"string","enum":["1024x1024","1024x1536","1536x1024"]}},"required":["prompt"],"additionalProperties":false}),
        read_only: false, requires_approval: false,
    }]
}
pub(super) fn validate(connection: &AiConnection) -> Result<(), String> {
    if connection.protocol == crate::Protocol::AnthropicMessages {
        return Err("图片生成仅支持 OpenAI 兼容 images/generations 接口。".into());
    }
    if connection.settings.model_type != ModelType::Image {
        return Err("请选择已保存的图片模型。".into());
    }
    connection.settings.validate(connection.protocol)?;
    let base = reqwest::Url::parse(&connection.base_url).map_err(|_| "图片接口地址无效。")?;
    if !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return Err("图片接口地址不能包含凭据、查询或片段。".into());
    }
    if connection.model.trim().is_empty() {
        return Err("图片模型标识为空。".into());
    }
    crate::endpoint(connection).map_err(|error| error.to_string())?;
    Ok(())
}

async fn generate(
    connection: AiConnection,
    request: ImageRequest,
    root: &Path,
    cancel: &CancellationToken,
) -> Result<(AiRun, Vec<ImageRef>), AiError> {
    validate(&connection).map_err(AiError::Provider)?;
    let http = crate::http_client()?;
    let url = format!(
        "{}/images/generations",
        connection.base_url.trim_end_matches('/')
    );
    let mut body = serde_json::to_value(request).map_err(|_| error("无法编码绘图请求。"))?;
    body["model"] = serde_json::json!(connection.model);
    body["n"] = serde_json::json!(1);
    let mut request = http
        .post(url)
        .timeout(std::time::Duration::from_millis(crate::REQUEST_LIMIT_MS))
        .json(&body);
    if let Some(key) = &connection.api_key {
        request = request.bearer_auth(key.as_str());
    }
    let (round_usage, started) = RoundUsage::start(None).map_err(AiError::Provider)?;
    let response = tokio::select! { biased; _ = cancel.cancelled() => return Err(AiError::Cancelled), result = request.send() => result.map_err(crate::network)? };
    tracing::info!(
        event = "ai.image.response",
        status = response.status().as_u16(),
        "Image generation response received"
    );
    if !response.status().is_success() {
        return Err(crate::provider_response(response, connection.api_key.as_ref(), cancel).await);
    }
    let body = read_body(response, 24 * 1024 * 1024, cancel).await?;
    let value: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| error("图片接口返回了无效 JSON。"))?;
    let data = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .filter(|data| !data.is_empty() && data.len() <= images::MAX_IMAGES)
        .ok_or_else(|| error("图片接口未返回 1 至 4 张图片。"))?;
    let mut decoded = Vec::new();
    for item in data {
        let bytes = if let Some(encoded) = item.get("b64_json").and_then(serde_json::Value::as_str)
        {
            if encoded.len() > (images::MAX_IMAGE_BYTES + 2) / 3 * 4 {
                return Err(error("生成图片超过 4 MiB。"));
            }
            STANDARD
                .decode(encoded)
                .map_err(|_| error("图片 Base64 编码无效。"))?
        } else if let Some(url) = item.get("url").and_then(serde_json::Value::as_str) {
            let url = reqwest::Url::parse(url).map_err(|_| error("生成图片 URL 无效。"))?;
            if url.scheme() != "https"
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
            {
                return Err(error("生成图片仅允许无凭据的 HTTPS URL。"));
            }
            let host = url.host_str().ok_or_else(|| error("生成图片缺少主机。"))?;
            if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
                return Err(error("生成图片不能指向本机。"));
            }
            let port = url
                .port_or_known_default()
                .ok_or_else(|| error("生成图片端口无效。"))?;
            // 先解析并固定公网地址，避免 DNS 重绑定；禁止重定向及代理绕过固定解析。
            let addresses: Vec<_> = tokio::select! { biased; _ = cancel.cancelled() => return Err(AiError::Cancelled), result = tokio::time::timeout(std::time::Duration::from_secs(10), tokio::net::lookup_host((host, port))) => result.map_err(|_| error("图片主机解析超时。"))?.map_err(|_| error("图片主机解析失败。"))? }.collect();
            if addresses.is_empty() || addresses.iter().any(|address| !public_ip(address.ip())) {
                return Err(error("生成图片 URL 必须指向公网地址。"));
            }
            let download = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .user_agent(concat!("lumakit/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(std::time::Duration::from_secs(10))
                .resolve_to_addrs(host, &addresses)
                .build()
                .map_err(crate::network)?;
            let response = tokio::select! { biased; _ = cancel.cancelled() => return Err(AiError::Cancelled), result = download.get(url.clone()).timeout(std::time::Duration::from_millis(DOWNLOAD_LIMIT_MS)).send() => result.map_err(|error| crate::network_within(error, "image_download", DOWNLOAD_LIMIT_MS))? };
            if !response.status().is_success() {
                return Err(error("生成图片下载失败；不跟随重定向。"));
            }
            read_body(response, images::MAX_IMAGE_BYTES, cancel).await?
        } else {
            return Err(error("图片接口没有返回 b64_json 或 HTTPS url。"));
        };
        let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            "image/jpeg"
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            "image/gif"
        } else {
            "image/webp"
        };
        images::from_bytes(mime, &bytes)
            .map_err(|_| error("生成图片内容校验失败或超过 4 MiB。"))?;
        decoded.push((mime, bytes));
    }
    if cancel.is_cancelled() {
        return Err(AiError::Cancelled);
    }
    std::fs::create_dir_all(root).map_err(|_| error("无法创建图片存储目录。"))?;
    let mut stored: Vec<ImageRef> = Vec::new();
    let save = (|| {
        for (mime, bytes) in decoded {
            let id = lumakit_support::new_id().map_err(|_| error("无法生成媒体标识。"))?;
            let extension = match mime {
                "image/png" => "png",
                "image/jpeg" => "jpg",
                "image/gif" => "gif",
                _ => "webp",
            };
            let key = format!("{id}.{extension}");
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(root.join(&key))
                .map_err(|_| error("无法保存生成图片。"))?;
            stored.push(ImageRef {
                id,
                mime: mime.into(),
                bytes: bytes.len() as u64,
                width: None,
                height: None,
                storage_key: key,
            });
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| error("无法完整保存生成图片。"))?;
        }
        Ok::<_, AiError>(())
    })();
    if let Err(error) = save {
        for image in &stored {
            let _ = std::fs::remove_file(root.join(&image.storage_key));
        }
        return Err(error);
    }
    let usage = value.get("usage").and_then(|usage| {
        Some(TokenUsage {
            input_tokens: usage.get("input_tokens")?.as_u64()?,
            output_tokens: usage.get("output_tokens")?.as_u64()?,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: None,
        })
    });
    tracing::info!(
        event = "ai.image.stored",
        count = stored.len(),
        "Generated images stored"
    );
    Ok((
        AiRun {
            text: String::new(),
            tool_calls: Vec::new(),
            rounds: vec![round_usage.finish(usage, None, false, started)],
            reasoning_blocks: Vec::new(),
        },
        stored,
    ))
}

fn error(message: &str) -> AiError {
    AiError::Provider(message.into())
}
async fn read_body(
    mut response: reqwest::Response,
    limit: usize,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, AiError> {
    let read = async {
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(crate::network)? {
            if chunk.len() > limit.saturating_sub(bytes.len()) {
                return Err(error("图片响应超过大小上限。"));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    };
    tokio::select! { biased; _ = cancel.cancelled() => Err(AiError::Cancelled), result = tokio::time::timeout(std::time::Duration::from_millis(crate::REQUEST_LIMIT_MS), read) => result.map_err(|_| AiError::Timeout { stage: "image_response", limit_ms: crate::REQUEST_LIMIT_MS })? }
}
fn public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_broadcast()
                || a == 0
                || a >= 240
                || (a == 100 && (64..=127).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 192 && b == 0 && c == 2)
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        std::net::IpAddr::V6(ip) => {
            let segments = ip.segments();
            if let Some(ip) = ip.to_ipv4_mapped() {
                public_ip(ip.into())
            } else {
                segments[0] & 0xe000 == 0x2000
                    && !(segments[0] == 0x2001 && segments[1] < 0x0200)
                    && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
                    && !(segments[0] == 0x2002)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_arguments_are_typed_unicode_bounded_and_square_by_default() {
        let request =
            ImageRequest::parse(serde_json::json!({"prompt":format!(" {} ", "🙂".repeat(4000))}))
                .unwrap();
        assert_eq!(request.prompt.chars().count(), 4000);
        assert_eq!(serde_json::to_value(request).unwrap()["size"], "1024x1024");
        for args in [
            serde_json::json!({"prompt":"🙂".repeat(4001)}),
            serde_json::json!({"prompt":" \n "}),
            serde_json::json!({"prompt":1}),
            serde_json::json!({"prompt":"图","size":"auto"}),
            serde_json::json!({"prompt":"图","n":2}),
            serde_json::json!({"prompt":"图","image_ids":["private"]}),
        ] {
            assert!(ImageRequest::parse(args).is_err());
        }
    }
}
