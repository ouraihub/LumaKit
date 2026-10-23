//! Transport-free MCP server core. Hosts bind only [`McpServer::bind_addr`], enforce
//! [`MAX_REQUEST_BYTES`] while reading (before JSON parsing), and wrap results/errors
//! in JSON-RPC envelopes. No listener is started by this module.
use crate::mcp::McpTool;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    net::{IpAddr, Ipv4Addr, SocketAddrV4},
};
use thiserror::Error;
use zeroize::Zeroizing;

/// Maximum complete HTTP request body, including the JSON-RPC envelope.
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// SHA-256 digest of a token; plaintext tokens must never be persisted.
pub type TokenHash = [u8; 32];

/// A host-declared permission. All scopes start disabled.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScopeSpec {
    /// Stable permission name.
    pub name: String,
    /// Human-readable explanation for the host's permission UI.
    pub description: String,
    /// Enabling this scope requires a second confirmation in the host UI.
    pub destructive: bool,
}

/// A tool and its host-owned implementation.
pub struct ExposedTool {
    /// Public MCP tool metadata.
    pub tool: McpTool,
    /// The declared permission required to list or call this tool.
    pub scope: String,
    handler: Box<dyn Fn(&Value) -> Result<Value, RpcError> + Send + Sync>,
}

impl ExposedTool {
    /// Register a closure returning an MCP tool result (`content`, `isError`, etc.).
    /// Errors must be safe to expose to clients, not raw provider errors.
    pub fn new(
        tool: McpTool,
        scope: impl Into<String>,
        handler: impl Fn(&Value) -> Result<Value, RpcError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            tool,
            scope: scope.into(),
            handler: Box::new(handler),
        }
    }
}

/// Persistable token metadata. Contains only a hash, never a recoverable token.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenRecord {
    /// Stable identifier used for revocation and persistence.
    pub hash: TokenHash,
    /// Permissions granted to the bearer; enabled server scopes still apply.
    pub scopes: BTreeSet<String>,
}

/// One-time issuance response. Deliberately neither serializable nor Debug.
/// The host returns the plaintext once, then drops it; persist only `record`.
pub struct IssuedToken {
    /// Plaintext bearer credential, zeroized on drop.
    pub token: Zeroizing<String>,
    /// Safe metadata for the host's database.
    pub record: TokenRecord,
}

/// Transport facts supplied by the HTTP host, never from forwarded headers.
/// Do not log or derive Debug for this type: it borrows a plaintext credential.
pub struct RequestAuth<'a> {
    /// Actual socket peer IP, not X-Forwarded-For.
    pub peer_ip: IpAddr,
    /// The single Origin header, if present. Reject duplicate headers in the host.
    pub origin: Option<&'a str>,
    /// Token extracted from the Authorization: Bearer header, without its prefix.
    pub bearer_token: &'a str,
    /// Actual complete raw body length, measured by the host before JSON parsing.
    pub body_bytes: usize,
}

/// Safe state for a host status page; does not claim an HTTP listener is running.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct McpServerStatus {
    /// The only address the HTTP adapter may bind.
    pub bind_addr: SocketAddrV4,
    /// Always true: remote listeners are not supported.
    pub loopback_only: bool,
    /// Currently enabled permissions.
    pub enabled_scopes: BTreeSet<String>,
    /// Number of active credentials, without their hashes or plaintext.
    pub token_count: usize,
    /// Number of registered tools, including tools hidden by scope settings.
    pub tool_count: usize,
}

/// Startup and permission-management errors, containing no credential material.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum McpServerError {
    /// Invalid registration or bind configuration.
    #[error("invalid MCP server configuration: {0}")]
    InvalidConfiguration(&'static str),
    /// A requested permission was not registered by the host.
    #[error("unknown MCP scope")]
    UnknownScope,
    /// The UI must explicitly confirm destructive scope activation.
    #[error("destructive MCP scope requires confirmation")]
    ConfirmationRequired,
    /// The operating system could not supply cryptographic randomness.
    #[error("MCP token randomness unavailable")]
    RandomUnavailable,
}

/// JSON-RPC error object. The HTTP adapter supplies the envelope and request ID.
#[derive(Debug, Clone, Serialize, Error, PartialEq, Eq)]
#[error("{message}")]
pub struct RpcError {
    /// JSON-RPC or server-defined error code.
    pub code: i64,
    /// Safe, static client-facing text.
    pub message: &'static str,
}

impl RpcError {
    /// Scope, peer or Origin policy denied the request.
    pub const FORBIDDEN: Self = Self {
        code: -32003,
        message: "forbidden",
    };
    /// Missing, invalid or revoked bearer token.
    pub const UNAUTHORIZED: Self = Self {
        code: -32001,
        message: "unauthorized",
    };
    /// Request exceeds the hard body limit.
    pub const TOO_LARGE: Self = Self {
        code: -32600,
        message: "request exceeds 64 KiB",
    };
    /// Malformed method parameters.
    pub const INVALID_PARAMS: Self = Self {
        code: -32602,
        message: "invalid params",
    };
    /// Unimplemented MCP method.
    pub const METHOD_NOT_FOUND: Self = Self {
        code: -32601,
        message: "method not found",
    };
}

/// Registered tools, scope policy and hashed credentials, independent of HTTP.
pub struct McpServer {
    bind_addr: SocketAddrV4,
    scopes: BTreeMap<String, ScopeSpec>,
    enabled_scopes: BTreeSet<String>,
    tools: BTreeMap<String, ExposedTool>,
    tokens: BTreeMap<TokenHash, TokenRecord>,
}

impl McpServer {
    /// Validate all registrations before the host opens its fixed loopback port.
    /// Port zero (an ephemeral port), duplicate names and undeclared scopes fail.
    pub fn new(
        port: u16,
        scopes: Vec<ScopeSpec>,
        tools: Vec<ExposedTool>,
    ) -> Result<Self, McpServerError> {
        if port == 0 {
            return Err(McpServerError::InvalidConfiguration(
                "fixed nonzero port required",
            ));
        }
        let mut registered_scopes = BTreeMap::new();
        for scope in scopes {
            if scope.name.trim().is_empty() || registered_scopes.contains_key(&scope.name) {
                return Err(McpServerError::InvalidConfiguration(
                    "empty or duplicate scope",
                ));
            }
            registered_scopes.insert(scope.name.clone(), scope);
        }
        let mut registered_tools = BTreeMap::new();
        for tool in tools {
            if tool.tool.name.trim().is_empty() || registered_tools.contains_key(&tool.tool.name) {
                return Err(McpServerError::InvalidConfiguration(
                    "empty or duplicate tool",
                ));
            }
            if !registered_scopes.contains_key(&tool.scope) {
                return Err(McpServerError::UnknownScope);
            }
            if !tool.tool.input_schema.is_object() {
                return Err(McpServerError::InvalidConfiguration(
                    "tool schema must be an object",
                ));
            }
            registered_tools.insert(tool.tool.name.clone(), tool);
        }
        Ok(Self {
            bind_addr: SocketAddrV4::new(Ipv4Addr::LOCALHOST, port),
            scopes: registered_scopes,
            enabled_scopes: BTreeSet::new(),
            tools: registered_tools,
            tokens: BTreeMap::new(),
        })
    }

    /// The HTTP host must bind this exact address and independently check peers.
    pub fn bind_addr(&self) -> SocketAddrV4 {
        self.bind_addr
    }

    /// Return safe core status; HTTP listener lifecycle belongs to the host.
    pub fn status(&self) -> McpServerStatus {
        McpServerStatus {
            bind_addr: self.bind_addr,
            loopback_only: true,
            enabled_scopes: self.enabled_scopes.clone(),
            token_count: self.tokens.len(),
            tool_count: self.tools.len(),
        }
    }

    /// Enable/disable a declared scope. Only pass `confirmed` after host UI confirmation.
    pub fn set_scope_enabled(
        &mut self,
        name: &str,
        enabled: bool,
        confirmed: bool,
    ) -> Result<(), McpServerError> {
        let scope = self.scopes.get(name).ok_or(McpServerError::UnknownScope)?;
        if enabled && scope.destructive && !confirmed {
            return Err(McpServerError::ConfirmationRequired);
        }
        if enabled {
            self.enabled_scopes.insert(name.to_owned());
        } else {
            self.enabled_scopes.remove(name);
        }
        Ok(())
    }

    /// Issue a random 8-byte prefix plus a random 32-byte secret, encoded as hex.
    /// Only its SHA-256 digest is retained in this server's state.
    pub fn issue_token(&mut self, scopes: BTreeSet<String>) -> Result<IssuedToken, McpServerError> {
        self.validate_scopes(&scopes)?;
        let mut random = Zeroizing::new([0_u8; 40]);
        getrandom::fill(&mut random[..]).map_err(|_| McpServerError::RandomUnavailable)?;
        let mut token = Zeroizing::new(String::with_capacity(85));
        token.push_str("lmk_");
        for (index, byte) in random.iter().enumerate() {
            if index == 8 {
                token.push('_');
            }
            // Writing hex into a String cannot fail.
            let _ = write!(&mut *token, "{byte:02x}");
        }
        let record = TokenRecord {
            hash: hash_token(token.as_str()),
            scopes,
        };
        self.tokens.insert(record.hash, record.clone());
        Ok(IssuedToken { token, record })
    }

    /// Load a hash-only record from the host database after validating its scopes.
    pub fn restore_token(&mut self, record: TokenRecord) -> Result<(), McpServerError> {
        self.validate_scopes(&record.scopes)?;
        self.tokens.insert(record.hash, record);
        Ok(())
    }

    /// Revoke immediately in memory; the host must also delete its persisted record.
    pub fn revoke_token(&mut self, hash: &TokenHash) -> bool {
        self.tokens.remove(hash).is_some()
    }

    /// Hash-only records suitable for persistence; plaintext is never available here.
    pub fn token_records(&self) -> impl Iterator<Item = &TokenRecord> {
        self.tokens.values()
    }

    /// Authenticate, enforce peer/Origin/body/scope policy, then dispatch an MCP method.
    /// The host must not send a JSON-RPC response for notifications (missing request ID).
    pub fn handle_request(
        &self,
        auth: RequestAuth<'_>,
        method: &str,
        params: &Value,
    ) -> Result<Value, RpcError> {
        if auth.body_bytes > MAX_REQUEST_BYTES {
            return Err(RpcError::TOO_LARGE);
        }
        if !auth.peer_ip.is_loopback() || auth.origin.is_some_and(|origin| !loopback_origin(origin))
        {
            return Err(RpcError::FORBIDDEN);
        }
        // Bound hashing work even if the HTTP host hands us an invalid bearer header.
        if auth.bearer_token.len() != 85 {
            return Err(RpcError::UNAUTHORIZED);
        }
        let token = self
            .tokens
            .get(&hash_token(auth.bearer_token))
            .ok_or(RpcError::UNAUTHORIZED)?;
        match method {
            "initialize" => Ok(json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "lumakit-ai", "version": env!("CARGO_PKG_VERSION")}
            })),
            "ping" => Ok(json!({})),
            "notifications/initialized" => Ok(Value::Null),
            "tools/list" => Ok(json!({"tools": self.tools.values()
                .filter(|tool| self.allowed(token, &tool.scope))
                .map(|tool| &tool.tool).collect::<Vec<_>>() })),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or(RpcError::INVALID_PARAMS)?;
                let tool = self.tools.get(name).ok_or(RpcError::INVALID_PARAMS)?;
                if !self.allowed(token, &tool.scope) {
                    return Err(RpcError::FORBIDDEN);
                }
                let empty = json!({});
                let arguments = params.get("arguments").unwrap_or(&empty);
                if !arguments.is_object() {
                    return Err(RpcError::INVALID_PARAMS);
                }
                (tool.handler)(arguments)
            }
            _ => Err(RpcError::METHOD_NOT_FOUND),
        }
    }

    fn allowed(&self, token: &TokenRecord, scope: &str) -> bool {
        self.enabled_scopes.contains(scope) && token.scopes.contains(scope)
    }

    fn validate_scopes(&self, scopes: &BTreeSet<String>) -> Result<(), McpServerError> {
        if scopes.iter().any(|scope| !self.scopes.contains_key(scope)) {
            return Err(McpServerError::UnknownScope);
        }
        Ok(())
    }
}

/// Calculate the SHA-256 identifier used in hash-only token persistence.
pub fn hash_token(token: &str) -> TokenHash {
    Sha256::digest(token.as_bytes()).into()
}

fn loopback_origin(origin: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(origin) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn auth(token: &str) -> RequestAuth<'_> {
        RequestAuth {
            peer_ip: Ipv4Addr::LOCALHOST.into(),
            origin: Some("http://localhost:4000"),
            bearer_token: token,
            body_bytes: 128,
        }
    }

    #[test]
    fn hash_only_tokens_scope_gating_and_transport_guards() -> Result<(), Box<dyn std::error::Error>>
    {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let scope = ScopeSpec {
            name: "write".into(),
            description: "Write data".into(),
            destructive: true,
        };
        let tool = ExposedTool::new(
            McpTool {
                name: "write_item".into(),
                description: "Write an item".into(),
                input_schema: json!({"type": "object"}),
            },
            "write",
            move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"content": [{"type": "text", "text": "written"}]}))
            },
        );
        let mut server = McpServer::new(4318, vec![scope], vec![tool])?;
        let issued = server.issue_token(BTreeSet::from(["write".into()]))?;
        let token = issued.token.as_str();
        assert_eq!(issued.record.hash, hash_token(token));
        let stored = serde_json::to_string(&server.token_records().collect::<Vec<_>>())?;
        assert!(!stored.contains(token));
        assert!(!stored.contains(&token[21..]));
        assert!(server.bind_addr().ip().is_loopback());
        let call = json!({"name": "write_item", "arguments": {}});
        assert_eq!(
            server.handle_request(auth(token), "tools/call", &call),
            Err(RpcError::FORBIDDEN)
        );
        assert_eq!(
            server.handle_request(auth(token), "tools/list", &Value::Null)?["tools"],
            json!([])
        );
        assert_eq!(
            server.set_scope_enabled("write", true, false),
            Err(McpServerError::ConfirmationRequired)
        );
        server.set_scope_enabled("write", true, true)?;
        let unscoped = server.issue_token(BTreeSet::new())?;
        assert_eq!(
            server.handle_request(auth(unscoped.token.as_str()), "tools/call", &call),
            Err(RpcError::FORBIDDEN)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            server.handle_request(auth(token), "tools/list", &Value::Null)?["tools"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        server.handle_request(auth(token), "tools/call", &call)?;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let mut remote = auth(token);
        remote.peer_ip = Ipv4Addr::new(192, 0, 2, 1).into();
        assert_eq!(
            server.handle_request(remote, "tools/call", &call),
            Err(RpcError::FORBIDDEN)
        );
        let mut foreign_origin = auth(token);
        foreign_origin.origin = Some("https://evil.example");
        assert_eq!(
            server.handle_request(foreign_origin, "tools/call", &call),
            Err(RpcError::FORBIDDEN)
        );
        let mut oversized = auth(token);
        oversized.body_bytes = MAX_REQUEST_BYTES + 1;
        assert_eq!(
            server.handle_request(oversized, "tools/call", &call),
            Err(RpcError::TOO_LARGE)
        );
        assert!(server.revoke_token(&issued.record.hash));
        assert_eq!(
            server.handle_request(auth(token), "tools/call", &call),
            Err(RpcError::UNAUTHORIZED)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn origin_policy_rejects_remote_and_non_origin_urls() {
        for origin in ["http://127.0.0.1:4318", "https://[::1]", "http://localhost"] {
            assert!(loopback_origin(origin), "{origin}");
        }
        for origin in [
            "null",
            "https://localhost.evil",
            "http://127.0.0.1/path",
            "http://user@localhost",
            "file://localhost",
            "http://localhost?x=1",
        ] {
            assert!(!loopback_origin(origin), "{origin}");
        }
    }
}
