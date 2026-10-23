//! Search boundaries shared by hosts and provider adapters.
use crate::{AiError, Protocol};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{future::Future, net::IpAddr, pin::Pin};

/// A single source returned by a search provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Ordered, filtered search output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub query: String,
    pub provider: String,
    pub hits: Vec<SearchHit>,
}

/// User-controlled search result policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchPreferences {
    /// Maximum number of retained hits (1..=50).
    pub max_results: usize,
    /// Maximum Unicode scalar values retained in each snippet.
    pub cutoff: usize,
    /// Domain rules or URL globs to exclude.
    pub blacklist: Vec<String>,
    /// Whether explicitly declared native model search is preferred.
    pub model_tools_preferred: bool,
}

impl Default for SearchPreferences {
    fn default() -> Self {
        Self {
            max_results: 5,
            cutoff: 2_000,
            blacklist: Vec::new(),
            model_tools_preferred: false,
        }
    }
}

/// Async boundary implemented by an external search adapter.
pub trait SearchProvider: Send + Sync {
    fn id(&self) -> &str;
    fn search<'a>(
        &'a self,
        query: &'a str,
        preferences: &'a SearchPreferences,
    ) -> Pin<Box<dyn Future<Output = Result<SearchResult, AiError>> + Send + 'a>>;
}

/// Native search capability and the protocol-specific tool selected for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeSearchDecision {
    pub protocol: Protocol,
    pub declared: bool,
    pub enabled: bool,
    pub preferred: bool,
    pub tool: Option<&'static str>,
}

impl NativeSearchDecision {
    pub fn new(protocol: Protocol, declared: bool, enabled: bool, preferred: bool) -> Self {
        let tool = if declared && enabled && preferred {
            match protocol {
                Protocol::OpenAiResponses => Some("web_search"),
                Protocol::AnthropicMessages => Some("web_search_20250305"),
                Protocol::OpenAiChat => None,
            }
        } else {
            None
        };
        Self {
            protocol,
            declared,
            enabled,
            preferred,
            tool,
        }
    }

    pub fn use_native(self) -> bool {
        self.tool.is_some()
    }
}

/// Applies the explicit native-search gates and returns the protocol tool name.
pub fn native_search_decision(
    protocol: Protocol,
    declared: bool,
    enabled: bool,
    preferred: bool,
) -> Option<&'static str> {
    NativeSearchDecision::new(protocol, declared, enabled, preferred).tool
}

/// Validate an outbound search/read URL: HTTPS is allowed; HTTP is loopback-only.
pub fn validate_search_url(raw: &str) -> Result<Url, AiError> {
    let url = Url::parse(raw).map_err(|_| AiError::Provider("invalid search URL".into()))?;
    if !url.username().is_empty() || url.password().is_some() || url.host_str().is_none() {
        return Err(AiError::Provider(
            "search URL must not contain credentials".into(),
        ));
    }
    match url.scheme() {
        "https" => Ok(url),
        "http" if is_loopback(&url) => Ok(url),
        _ => Err(AiError::Provider(
            "search URL must use HTTPS or loopback HTTP".into(),
        )),
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host_str().map(str::to_ascii_lowercase).as_deref() {
        Some("localhost") => true,
        Some(host) => host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()),
        None => false,
    }
}

/// Filter, de-duplicate and bound provider output without performing I/O.
pub fn process_result(mut result: SearchResult, preferences: &SearchPreferences) -> SearchResult {
    let limit = preferences.max_results.clamp(1, 50);
    let cutoff = preferences.cutoff;
    let mut seen = std::collections::HashSet::new();
    result.hits.retain_mut(|hit| {
        let Ok(url) = validate_search_url(&hit.url) else {
            return false;
        };
        if is_blacklisted(&url, &preferences.blacklist) {
            return false;
        }
        let mut key = url.clone();
        key.set_fragment(None);
        if !seen.insert(key.to_string()) {
            return false;
        }
        hit.snippet = hit.snippet.chars().take(cutoff).collect();
        true
    });
    result.hits.truncate(limit);
    result
}

/// Return whether a URL matches a domain or simple URL-glob blacklist rule.
pub fn is_blacklisted(url: &Url, rules: &[String]) -> bool {
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let mut normalized = url.clone();
    let _ = normalized.set_host(Some(&host));
    rules
        .iter()
        .any(|raw| rule_matches(raw, &host, normalized.as_str()))
}

fn rule_matches(raw: &str, host: &str, full_url: &str) -> bool {
    let rule = raw.trim().to_ascii_lowercase();
    if rule.is_empty() || rule.chars().any(char::is_whitespace) {
        return false;
    }
    if let Some((scheme, _rest)) = rule.split_once("://") {
        if !matches!(scheme, "http" | "https" | "*") {
            return false;
        }
        return glob_match(&rule, full_url);
    }
    if rule.contains('/') || rule.contains(':') {
        return false;
    }
    if rule.contains('*') || rule.contains('?') {
        return glob_match(&rule, host);
    }
    host == rule
        || host
            .strip_suffix(&rule)
            .is_some_and(|suffix| suffix.ends_with('.'))
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut i, mut j, mut star, mut retry) = (0, 0, None, 0);
    while j < t.len() {
        if i < p.len() && (p[i] == b'?' || p[i] == t[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == b'*' {
            star = Some(i);
            i += 1;
            retry = j;
        } else if let Some(s) = star {
            retry += 1;
            j = retry;
            i = s + 1;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == b'*' {
        i += 1;
    }
    i == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blacklist_matches_domains_and_globs() {
        let url = Url::parse("https://news.example.com/a").unwrap();
        assert!(is_blacklisted(&url, &["example.com".into()]));
        assert!(is_blacklisted(&url, &["n?ws.example.com".into()]));
        assert!(!is_blacklisted(&url, &["other.example.com".into()]));
        assert!(is_blacklisted(&url, &["https://*.example.com/*".into()]));
    }

    #[test]
    fn result_is_cut_off_and_deduplicated() {
        let result = SearchResult {
            query: "q".into(),
            provider: "p".into(),
            hits: vec![
                SearchHit {
                    title: "a".into(),
                    url: "https://example.com/a#one".into(),
                    snippet: "一二三".into(),
                },
                SearchHit {
                    title: "a".into(),
                    url: "https://example.com/a#two".into(),
                    snippet: "duplicate".into(),
                },
            ],
        };
        let prefs = SearchPreferences {
            cutoff: 2,
            ..Default::default()
        };
        let out = process_result(result, &prefs);
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].snippet, "一二");
    }

    #[test]
    fn native_search_requires_all_gates_and_protocol_support() {
        assert_eq!(
            native_search_decision(Protocol::OpenAiResponses, true, true, true),
            Some("web_search")
        );
        assert_eq!(
            native_search_decision(Protocol::AnthropicMessages, true, true, true),
            Some("web_search_20250305")
        );
        assert_eq!(
            native_search_decision(Protocol::OpenAiChat, true, true, true),
            None
        );
        assert_eq!(
            native_search_decision(Protocol::OpenAiResponses, true, false, true),
            None
        );
        assert_eq!(
            native_search_decision(Protocol::OpenAiResponses, false, true, true),
            None
        );
    }

    #[test]
    fn url_validation_restricts_plain_http() {
        assert!(validate_search_url("https://example.com").is_ok());
        assert!(validate_search_url("http://127.0.0.1:8080/search").is_ok());
        assert!(validate_search_url("http://example.com").is_err());
    }
}
