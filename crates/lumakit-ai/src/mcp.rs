//! Small, browser-free MCP Streamable HTTP client.
use reqwest::{Client, Url, header};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use thiserror::Error;

const MAX_TOOLS: usize = 200;
const MAX_PAGES: usize = 10;
const MAX_DESCRIPTION: usize = 1024;
const MAX_SCHEMA: usize = 32 * 1024;

/// Configuration for one MCP HTTP server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerConfig {
    /// MCP endpoint URL. HTTPS is required except for loopback HTTP URLs.
    pub url: String,
    /// Whether this server is enabled by the user.
    pub enabled: bool,
    /// Whether this server has been explicitly trusted by the user.
    pub trusted: bool,
}

impl McpServerConfig {
    /// Creates a disabled and untrusted server configuration.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            enabled: false,
            trusted: false,
        }
    }

    /// Returns the parsed and policy-checked endpoint.
    pub fn validate_url(&self) -> Result<Url, McpError> {
        validate_url(&self.url)
    }

    /// Only enabled, trusted servers may expose or invoke tools.
    pub fn is_allowed(&self) -> bool {
        self.enabled && self.trusted
    }
}

/// A tool advertised by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

impl McpTool {
    fn validate(&self) -> Result<(), McpError> {
        if self.name.is_empty() {
            return Err(McpError::InvalidTool("tool name is empty".into()));
        }
        if self.description.chars().count() > MAX_DESCRIPTION {
            return Err(McpError::InvalidTool(
                "tool description exceeds 1024 characters".into(),
            ));
        }
        let schema = serde_json::to_vec(&self.input_schema)
            .map_err(|e| McpError::InvalidTool(format!("invalid input schema: {e}")))?;
        if schema.len() > MAX_SCHEMA {
            return Err(McpError::InvalidTool("tool schema exceeds 32 KiB".into()));
        }
        Ok(())
    }
}

/// Result of an MCP tool call. A transport failure after sending is unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpCallResult {
    Text(String),
    Error(String),
    Unknown,
}

/// Errors raised while configuring or speaking MCP.
#[derive(Debug, Error)]
pub enum McpError {
    #[error("invalid MCP URL: {0}")]
    InvalidUrl(String),
    #[error("MCP server is not enabled and trusted")]
    NotAllowed,
    #[error("MCP HTTP request failed: {0}")]
    Http(String),
    #[error("MCP JSON-RPC error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("invalid MCP response: {0}")]
    Protocol(String),
    #[error("invalid MCP tool: {0}")]
    InvalidTool(String),
}

/// Minimal Streamable HTTP MCP client.
#[derive(Clone)]
pub struct McpClient {
    config: McpServerConfig,
    http: Client,
    next_id: Arc<AtomicU64>,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl McpClient {
    /// Creates a client after applying the HTTPS/loopback URL policy.
    pub fn new(config: McpServerConfig) -> Result<Self, McpError> {
        config.validate_url()?;
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| McpError::Http(e.to_string()))?;
        Ok(Self {
            config,
            http,
            next_id: Arc::new(AtomicU64::new(1)),
        })
    }

    /// Returns this client's server configuration.
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }

    /// Performs MCP initialization and the required initialized notification.
    pub async fn initialize(&self) -> Result<(), McpError> {
        self.ensure_allowed()?;
        let result = self
            .rpc(
                "initialize",
                Some(json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "clientInfo": {"name": "lumakit-ai", "version": env!("CARGO_PKG_VERSION")}
                })),
            )
            .await?;
        if !result.is_object() {
            return Err(McpError::Protocol(
                "initialize result is not an object".into(),
            ));
        }
        self.notify("notifications/initialized", json!({})).await
    }

    /// Lists and validates all advertised tools, following at most ten pages.
    pub async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        self.ensure_allowed()?;
        let mut cursor: Option<String> = None;
        let mut tools = Vec::new();
        for _ in 0..MAX_PAGES {
            let params = cursor.as_ref().map(|c| json!({"cursor": c}));
            let result = self.rpc("tools/list", params).await?;
            let page: ToolPage = serde_json::from_value(result)
                .map_err(|e| McpError::Protocol(format!("invalid tools/list result: {e}")))?;
            for tool in page.tools {
                tool.validate()?;
                tools.push(tool);
                if tools.len() > MAX_TOOLS {
                    return Err(McpError::Protocol(
                        "MCP server advertises more than 200 tools".into(),
                    ));
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(tools);
            }
        }
        Err(McpError::Protocol(
            "MCP tools/list exceeded ten pages".into(),
        ))
    }

    /// Alias matching the MCP method name.
    pub async fn tools_list(&self) -> Result<Vec<McpTool>, McpError> {
        self.list_tools().await
    }

    /// Calls a server tool. The client never executes tools locally.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<McpCallResult, McpError> {
        self.ensure_allowed()?;
        if name.is_empty() {
            return Err(McpError::InvalidTool("tool name is empty".into()));
        }
        let rpc = self
            .rpc(
                "tools/call",
                Some(json!({"name": name, "arguments": arguments})),
            )
            .await;
        let result = match rpc {
            Ok(value) => value,
            Err(McpError::Http(_)) | Err(McpError::Protocol(_)) => {
                return Ok(McpCallResult::Unknown);
            }
            Err(error) => return Err(error),
        };
        let is_error = result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let content = result
            .get("content")
            .and_then(Value::as_array)
            .ok_or_else(|| McpError::Protocol("tools/call result has no content".into()))?;
        let mut text = String::new();
        for item in content {
            if item.get("type").and_then(Value::as_str) != Some("text") {
                return Ok(McpCallResult::Error(
                    "MCP tool returned non-text content".into(),
                ));
            }
            let part = item
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| McpError::Protocol("text content has no text".into()))?;
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(part);
        }
        Ok(if is_error {
            McpCallResult::Error(text)
        } else {
            McpCallResult::Text(text)
        })
    }

    /// Alias matching the MCP method name.
    pub async fn tools_call(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<McpCallResult, McpError> {
        self.call_tool(name, arguments).await
    }

    fn ensure_allowed(&self) -> Result<(), McpError> {
        self.config
            .is_allowed()
            .then_some(())
            .ok_or(McpError::NotAllowed)
    }

    async fn rpc(&self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut request = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if let Some(params) = params {
            request["params"] = params;
        }
        let response = self
            .http
            .post(self.config.validate_url()?)
            .header(header::ACCEPT, "application/json, text/event-stream")
            .json(&request)
            .send()
            .await
            .map_err(|e| McpError::Http(e.to_string()))?;
        decode_response(response).await
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        let request = json!({"jsonrpc": "2.0", "method": method, "params": params});
        self.http
            .post(self.config.validate_url()?)
            .header(header::ACCEPT, "application/json, text/event-stream")
            .json(&request)
            .send()
            .await
            .map_err(|e| McpError::Http(e.to_string()))?
            .error_for_status()
            .map_err(|e| McpError::Http(e.to_string()))?;
        Ok(())
    }
}

#[derive(Deserialize)]
struct ToolPage {
    #[serde(default)]
    tools: Vec<McpTool>,
    #[serde(rename = "nextCursor")]
    next_cursor: Option<String>,
}

fn validate_url(raw: &str) -> Result<Url, McpError> {
    let url = Url::parse(raw).map_err(|e| McpError::InvalidUrl(e.to_string()))?;
    let host = url
        .host_str()
        .ok_or_else(|| McpError::InvalidUrl("URL has no host".into()))?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host == "[::1]"
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(McpError::InvalidUrl(
            "MCP requires HTTPS; HTTP is only allowed for loopback".into(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(McpError::InvalidUrl("userinfo is not allowed".into()));
    }
    for (key, _) in url.query_pairs() {
        let key = key.to_ascii_lowercase();
        if [
            "key",
            "api_key",
            "api-key",
            "apikey",
            "token",
            "access_token",
            "access-token",
            "bearer",
            "secret",
            "secret_key",
            "client_secret",
            "password",
            "credential",
            "auth",
        ]
        .contains(&key.as_str())
        {
            return Err(McpError::InvalidUrl(
                "credential-like query parameters are not allowed".into(),
            ));
        }
    }
    Ok(url)
}

async fn decode_response(response: reqwest::Response) -> Result<Value, McpError> {
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let body = response
        .text()
        .await
        .map_err(|e| McpError::Http(e.to_string()))?;
    if !status.is_success() {
        return Err(McpError::Http(format!("HTTP {status}")));
    }
    let payload = if content_type.starts_with("text/event-stream")
        || body.lines().any(|l| l.trim_start().starts_with("data:"))
    {
        body.lines()
            .filter_map(|line| line.trim_start().strip_prefix("data:").map(str::trim))
            .find(|line| !line.is_empty() && *line != "[DONE]")
            .ok_or_else(|| McpError::Protocol("empty MCP event stream".into()))?
            .to_owned()
    } else {
        body
    };
    let value: Value = serde_json::from_str(&payload)
        .map_err(|e| McpError::Protocol(format!("invalid JSON-RPC response: {e}")))?;
    if let Some(error) = value.get("error") {
        return Err(McpError::Rpc {
            code: error.get("code").and_then(Value::as_i64).unwrap_or(-1),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .into(),
        });
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| McpError::Protocol("JSON-RPC response has no result".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_https_and_loopback_http_only() {
        for url in [
            "https://example.com/mcp",
            "http://127.0.0.1:4318/mcp",
            "http://[::1]/mcp",
            "http://localhost/mcp",
        ] {
            assert!(validate_url(url).is_ok(), "{url}");
        }
        for url in [
            "http://example.com/mcp",
            "ftp://127.0.0.1/mcp",
            "https://user@example.com/mcp",
            "https://example.com/mcp?token=x",
        ] {
            assert!(validate_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn enforces_tool_limits_and_trust_gate() {
        let config = McpServerConfig {
            url: "http://127.0.0.1:9".into(),
            enabled: true,
            trusted: false,
        };
        assert!(!config.is_allowed());
        let tool = McpTool {
            name: "x".into(),
            description: "ok".into(),
            input_schema: json!({"type":"object"}),
        };
        assert!(tool.validate().is_ok());
        assert!(
            McpTool {
                description: "x".repeat(1025),
                ..tool.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            McpTool {
                input_schema: Value::String("x".repeat(MAX_SCHEMA)),
                ..tool
            }
            .validate()
            .is_err()
        );
    }
}
