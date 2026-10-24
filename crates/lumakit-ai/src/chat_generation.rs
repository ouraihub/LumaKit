//! OpenAI 兼容图片生成；不复用文本接口，不向外部图片 URL 发送密钥。
use super::ImageRef;
use crate::{AiConnection, AiError, AiRun, images, model_config::{ModelType, RoundUsage, TokenUsage}};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{io::Write, path::Path};
use tokio_util::sync::CancellationToken;

pub(super) fn validate(connection: &AiConnection) -> Result<(), String> {
    if connection.protocol == crate::Protocol::AnthropicMessages { return Err("图片生成仅支持 OpenAI 兼容 images/generations 接口。".into()); }
    if connection.settings.model_type != ModelType::Image { return Err("请选择已保存的图片模型。".into()); }
    connection.settings.validate(connection.protocol)?;
    let base = reqwest::Url::parse(&connection.base_url).map_err(|_| "图片接口地址无效。")?;
    if !base.username().is_empty() || base.password().is_some() || base.query().is_some() || base.fragment().is_some() { return Err("图片接口地址不能包含凭据、查询或片段。".into()); }
    if connection.model.trim().is_empty() { return Err("图片模型标识为空。".into()); }
    crate::endpoint(connection).map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) async fn generate(connection: AiConnection, prompt: String, root: &Path, cancel: &CancellationToken) -> Result<(AiRun, Vec<ImageRef>), AiError> {
    validate(&connection).map_err(AiError::Provider)?;
    let http = crate::http_client()?;
    let url = format!("{}/images/generations", connection.base_url.trim_end_matches('/'));
    let mut request = http.post(url).timeout(std::time::Duration::from_secs(120)).json(&serde_json::json!({"model": connection.model, "prompt": prompt, "n": 1, "size": "1024x1024"}));
    if let Some(key) = &connection.api_key { request = request.bearer_auth(key.as_str()); }
    let response = tokio::select! { biased; _ = cancel.cancelled() => return Err(AiError::Cancelled), result = request.send() => result.map_err(crate::network)? };
    if !response.status().is_success() { return Err(crate::provider_response(response, connection.api_key.as_ref(), cancel).await); }
    let body = read_body(response, 24 * 1024 * 1024, cancel).await?;
    let value: serde_json::Value = serde_json::from_slice(&body).map_err(|_| error("图片接口返回了无效 JSON。"))?;
    let data = value.get("data").and_then(serde_json::Value::as_array).filter(|data| !data.is_empty() && data.len() <= images::MAX_IMAGES).ok_or_else(|| error("图片接口未返回 1 至 4 张图片。"))?;
    let mut decoded = Vec::new();
    for item in data {
        let bytes = if let Some(encoded) = item.get("b64_json").and_then(serde_json::Value::as_str) {
            if encoded.len() > (images::MAX_IMAGE_BYTES + 2) / 3 * 4 { return Err(error("生成图片超过 4 MiB。")); }
            STANDARD.decode(encoded).map_err(|_| error("图片 Base64 编码无效。"))?
        } else if let Some(url) = item.get("url").and_then(serde_json::Value::as_str) {
            let url = reqwest::Url::parse(url).map_err(|_| error("生成图片 URL 无效。"))?;
            if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() { return Err(error("生成图片仅允许无凭据的 HTTPS URL。")); }
            let host = url.host_str().ok_or_else(|| error("生成图片缺少主机。"))?;
            if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") { return Err(error("生成图片不能指向本机。")); }
            let port = url.port_or_known_default().ok_or_else(|| error("生成图片端口无效。"))?;
            // 先解析并固定公网地址，避免 DNS 重绑定；禁止重定向及代理绕过固定解析。
            let addresses: Vec<_> = tokio::select! { biased; _ = cancel.cancelled() => return Err(AiError::Cancelled), result = tokio::time::timeout(std::time::Duration::from_secs(10), tokio::net::lookup_host((host, port))) => result.map_err(|_| error("图片主机解析超时。"))?.map_err(|_| error("图片主机解析失败。"))? }.collect();
            if addresses.is_empty() || addresses.iter().any(|address| !public_ip(address.ip())) { return Err(error("生成图片 URL 必须指向公网地址。")); }
            let download = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).no_proxy().resolve_to_addrs(host, &addresses).build().map_err(crate::network)?;
            let response = tokio::select! { biased; _ = cancel.cancelled() => return Err(AiError::Cancelled), result = download.get(url.clone()).timeout(std::time::Duration::from_secs(60)).send() => result.map_err(crate::network)? };
            if !response.status().is_success() { return Err(error("生成图片下载失败；不跟随重定向。")); }
            read_body(response, images::MAX_IMAGE_BYTES, cancel).await?
        } else { return Err(error("图片接口没有返回 b64_json 或 HTTPS url。")); };
        let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") { "image/png" } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) { "image/jpeg" } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") { "image/gif" } else { "image/webp" };
        images::from_bytes(mime, &bytes).map_err(|_| error("生成图片内容校验失败或超过 4 MiB。"))?;
        decoded.push((mime, bytes));
    }
    if cancel.is_cancelled() { return Err(AiError::Cancelled); }
    std::fs::create_dir_all(root).map_err(|_| error("无法创建图片存储目录。"))?;
    let mut stored: Vec<ImageRef> = Vec::new();
    let save = (|| {
        for (mime, bytes) in decoded {
            let id = lumakit_support::new_id().map_err(|_| error("无法生成媒体标识。"))?;
            let extension = match mime { "image/png" => "png", "image/jpeg" => "jpg", "image/gif" => "gif", _ => "webp" };
            let key = format!("{id}.{extension}");
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(root.join(&key)).map_err(|_| error("无法保存生成图片。"))?;
            stored.push(ImageRef { id, mime: mime.into(), bytes: bytes.len() as u64, width: None, height: None, storage_key: key });
            file.write_all(&bytes).and_then(|()| file.sync_all()).map_err(|_| error("无法完整保存生成图片。"))?;
        }
        Ok::<_, AiError>(())
    })();
    if let Err(error) = save { for image in &stored { let _ = std::fs::remove_file(root.join(&image.storage_key)); } return Err(error); }
    let usage = value.get("usage").and_then(|usage| Some(TokenUsage { input_tokens: usage.get("input_tokens")?.as_u64()?, output_tokens: usage.get("output_tokens")?.as_u64()?, cache_read_tokens: 0, cache_write_tokens: 0 }));
    Ok((AiRun { text: String::new(), tool_calls: Vec::new(), rounds: vec![RoundUsage { usage, pricing: None, cost: None }], reasoning_blocks: Vec::new() }, stored))
}

fn error(message: &str) -> AiError { AiError::Provider(message.into()) }
async fn read_body(mut response: reqwest::Response, limit: usize, cancel: &CancellationToken) -> Result<Vec<u8>, AiError> {
    let read = async {
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(crate::network)? {
            if chunk.len() > limit.saturating_sub(bytes.len()) { return Err(error("图片响应超过大小上限。")); }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    };
    tokio::select! { biased; _ = cancel.cancelled() => Err(AiError::Cancelled), result = tokio::time::timeout(std::time::Duration::from_secs(120), read) => result.map_err(|_| error("图片响应读取超时。"))? }
}
fn public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => { let [a, b, c, _] = ip.octets(); !(ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() || a == 0 || a >= 240 || (a == 100 && (64..=127).contains(&b)) || (a == 192 && b == 0 && c == 0) || (a == 192 && b == 0 && c == 2) || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100))) || (a == 203 && b == 0 && c == 113)) }
        std::net::IpAddr::V6(ip) => { let segments = ip.segments(); if let Some(ip) = ip.to_ipv4_mapped() { public_ip(ip.into()) } else { segments[0] & 0xe000 == 0x2000 && !(segments[0] == 0x2001 && segments[1] < 0x0200) && !(segments[0] == 0x2001 && segments[1] == 0x0db8) && !(segments[0] == 0x2002) } }
    }
}
