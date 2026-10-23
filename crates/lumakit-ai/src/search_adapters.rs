//! HTTP adapters for Tavily, Brave Search, and Exa.
use crate::{AiError, search};
use futures_util::StreamExt;
use reqwest::{Url, header};
use serde::{Deserialize, Serialize};
use std::{fmt, future::Future, pin::Pin, time::Duration};

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 8 * 1024;

/// Supported hosted search APIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchAdapterKind {
    /// Tavily Search API.
    Tavily,
    /// Brave Web Search API.
    Brave,
    /// Exa Search API.
    Exa,
}

impl SearchAdapterKind {
    /// Stable provider identifier used in [`search::SearchResult`].
    pub const fn id(self) -> &'static str {
        match self {
            Self::Tavily => "tavily",
            Self::Brave => "brave",
            Self::Exa => "exa",
        }
    }
}

/// Configuration for a hosted search adapter.
#[derive(Clone)]
pub struct SearchAdapterConfig {
    /// Provider implementation to use.
    pub kind: SearchAdapterKind,
    /// HTTPS endpoint, or HTTP on loopback for local test servers.
    pub endpoint: String,
    /// Provider API key. It is deliberately omitted from [`Debug`].
    pub api_key: String,
}

impl fmt::Debug for SearchAdapterConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchAdapterConfig")
            .field("kind", &self.kind)
            .field("endpoint", &self.endpoint)
            .field("api_key", &"***")
            .finish()
    }
}

impl SearchAdapterConfig {
    /// Construct a provider configuration.
    pub fn new(
        kind: SearchAdapterKind,
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            endpoint: endpoint.into(),
            api_key: api_key.into(),
        }
    }
}

/// A small reqwest-backed [`search::SearchProvider`] implementation.
#[derive(Clone)]
pub struct SearchHttpAdapter {
    config: SearchAdapterConfig,
    client: reqwest::Client,
}

impl fmt::Debug for SearchHttpAdapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchHttpAdapter")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl SearchHttpAdapter {
    /// Build an adapter after validating its endpoint.
    pub fn new(config: SearchAdapterConfig) -> Result<Self, AiError> {
        search::validate_search_url(&config.endpoint)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| AiError::Provider(format!("search HTTP client: {e}")))?;
        Ok(Self { config, client })
    }

    /// Construct an adapter with a caller-owned client, useful for loopback tests.
    pub fn with_client(
        config: SearchAdapterConfig,
        client: reqwest::Client,
    ) -> Result<Self, AiError> {
        search::validate_search_url(&config.endpoint)?;
        Ok(Self { config, client })
    }

    /// Return the adapter configuration.
    pub fn config(&self) -> &SearchAdapterConfig {
        &self.config
    }

    fn endpoint(&self) -> Result<Url, AiError> {
        let mut url = search::validate_search_url(&self.config.endpoint)?;
        if url.path().is_empty() || url.path() == "/" {
            url.set_path(match self.config.kind {
                SearchAdapterKind::Tavily | SearchAdapterKind::Exa => "/search",
                SearchAdapterKind::Brave => "/res/v1/web/search",
            });
        }
        Ok(url)
    }

    async fn request(
        &self,
        query: &str,
        preferences: &search::SearchPreferences,
    ) -> Result<search::SearchResult, AiError> {
        let count = preferences.max_results.clamp(1, 50);
        let count_text = count.min(20).to_string();
        let response = match self.config.kind {
            SearchAdapterKind::Tavily => {
                self.client
                    .post(self.endpoint()?)
                    .header(header::ACCEPT, "application/json")
                    .header(
                        header::AUTHORIZATION,
                        secret_header("Bearer ", &self.config.api_key)?,
                    )
                    .json(&TavilySearchRequest {
                        query,
                        max_results: count,
                    })
                    .send()
                    .await
            }
            SearchAdapterKind::Brave => {
                self.client
                    .get(self.endpoint()?)
                    .header(header::ACCEPT, "application/json")
                    .header(
                        "X-Subscription-Token",
                        secret_header("", &self.config.api_key)?,
                    )
                    .query(&[("q", query), ("count", count_text.as_str())])
                    .send()
                    .await
            }
            SearchAdapterKind::Exa => {
                self.client
                    .post(self.endpoint()?)
                    .header(header::ACCEPT, "application/json")
                    .header("x-api-key", secret_header("", &self.config.api_key)?)
                    .json(&ExaSearchRequest {
                        query,
                        num_results: count,
                        contents: ExaContents { text: true },
                    })
                    .send()
                    .await
            }
        }
        .map_err(|e| AiError::Provider(format!("search request: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            let _ = read_bounded(response, MAX_ERROR_BYTES).await;
            return Err(AiError::Provider(format!(
                "search provider returned {status}"
            )));
        }
        let body = read_bounded(response, MAX_RESPONSE_BYTES).await?;
        let (actual_query, raw_hits): (Option<String>, Vec<(String, String, String)>) = match self
            .config
            .kind
        {
            SearchAdapterKind::Tavily => {
                let r: TavilySearchResponse = serde_json::from_slice(&body).map_err(json_error)?;
                (
                    r.query,
                    r.results
                        .into_iter()
                        .map(|x| (x.title, x.url, x.content))
                        .collect(),
                )
            }
            SearchAdapterKind::Brave => {
                let r: BraveSearchResponse = serde_json::from_slice(&body).map_err(json_error)?;
                (
                    None,
                    r.web
                        .results
                        .into_iter()
                        .map(|x| (x.title, x.url, x.description))
                        .collect(),
                )
            }
            SearchAdapterKind::Exa => {
                let r: ExaSearchResponse = serde_json::from_slice(&body).map_err(json_error)?;
                (
                    None,
                    r.results
                        .into_iter()
                        .map(|x| {
                            (
                                x.title.unwrap_or_default(),
                                x.url,
                                x.text.unwrap_or_default(),
                            )
                        })
                        .collect(),
                )
            }
        };
        Ok(search::process_result(
            search::SearchResult {
                query: actual_query.unwrap_or_else(|| query.to_owned()),
                provider: self.config.kind.id().to_owned(),
                hits: raw_hits
                    .into_iter()
                    .map(|(title, url, snippet)| search::SearchHit {
                        title,
                        url,
                        snippet,
                    })
                    .collect(),
            },
            preferences,
        ))
    }
}

impl search::SearchProvider for SearchHttpAdapter {
    fn id(&self) -> &str {
        self.config.kind.id()
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        preferences: &'a search::SearchPreferences,
    ) -> Pin<Box<dyn Future<Output = Result<search::SearchResult, AiError>> + Send + 'a>> {
        Box::pin(self.request(query, preferences))
    }
}

fn secret_header(prefix: &str, value: &str) -> Result<header::HeaderValue, AiError> {
    let mut out = header::HeaderValue::from_str(&format!("{prefix}{value}"))
        .map_err(|_| AiError::Provider("invalid search API key".into()))?;
    out.set_sensitive(true);
    Ok(out)
}

fn json_error(error: serde_json::Error) -> AiError {
    AiError::Provider(format!("invalid search response: {error}"))
}

async fn read_bounded(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, AiError> {
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(AiError::Provider(
            "search response exceeds size limit".into(),
        ));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| AiError::Provider(format!("search response: {e}")))?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(AiError::Provider(
                "search response exceeds size limit".into(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[derive(Serialize)]
struct TavilySearchRequest<'a> {
    query: &'a str,
    max_results: usize,
}
#[derive(Deserialize)]
struct TavilySearchResponse {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    results: Vec<TavilyResult>,
}
#[derive(Deserialize)]
struct TavilyResult {
    #[serde(default)]
    title: String,
    url: String,
    #[serde(default)]
    content: String,
}

#[derive(Deserialize)]
struct BraveSearchResponse {
    #[serde(default)]
    web: BraveWeb,
}
#[derive(Default, Deserialize)]
struct BraveWeb {
    #[serde(default)]
    results: Vec<BraveResult>,
}
#[derive(Deserialize)]
struct BraveResult {
    #[serde(default)]
    title: String,
    url: String,
    #[serde(default)]
    description: String,
}

#[derive(Serialize)]
struct ExaSearchRequest<'a> {
    query: &'a str,
    #[serde(rename = "numResults")]
    num_results: usize,
    contents: ExaContents,
}
#[derive(Serialize)]
struct ExaContents {
    text: bool,
}
#[derive(Deserialize)]
struct ExaSearchResponse {
    #[serde(default)]
    results: Vec<ExaResult>,
}
#[derive(Deserialize)]
struct ExaResult {
    #[serde(default)]
    title: Option<String>,
    url: String,
    #[serde(default)]
    text: Option<String>,
}
