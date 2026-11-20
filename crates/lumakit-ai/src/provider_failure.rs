//! Upstream failures expose only status and hashed diagnostic identities.
use crate::{AiError, network};
use tokio_util::sync::CancellationToken;

pub(super) async fn provider_response(
    mut response: reqwest::Response,
    cancel: &CancellationToken,
) -> AiError {
    let status = response.status().as_u16();
    let request_id = [
        "x-request-id",
        "request-id",
        "x-oneapi-request-id",
        "x-new-api-request-id",
        "trace-id",
        "cf-ray",
    ]
    .iter()
    .find_map(|name| {
        response
            .headers()
            .get(*name)
            .and_then(|value| value.to_str().ok())
            .map(lumakit_observability::diagnostic_hash)
    })
    .unwrap_or_else(|| "unavailable".into());
    let endpoint_hash = lumakit_observability::diagnostic_hash(response.url().as_str());
    // Consume only a bounded body; never return untrusted provider prose across the UI boundary.
    let read = async {
        let mut bytes = 0usize;
        while let Some(chunk) = response.chunk().await.map_err(network)? {
            bytes = bytes.saturating_add(chunk.len());
            if bytes > 8192 {
                break;
            }
        }
        Ok::<_, AiError>(())
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => return AiError::Cancelled,
        _ = tokio::time::timeout(std::time::Duration::from_secs(10), read) => {},
    }
    tracing::warn!(target: "lumakit_ai::http", status, %endpoint_hash, request_id_hash = %request_id, "ai.http.rejected");
    AiError::Http {
        status,
        request_id,
        body: "上游拒绝请求；原始响应已省略，请按诊断 ID 排查".into(),
    }
}

#[cfg(test)]
#[path = "provider_failure_tests.rs"]
mod tests;
