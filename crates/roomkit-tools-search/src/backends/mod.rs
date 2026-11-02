use crate::{BackendOutput, SearchBackend, SearchFuture, SearchInput};
use reqwest::{
    Url,
    header::{self, HeaderMap, HeaderValue},
};
use roomkit_tools_core::ToolError;
use roomkit_tools_http::{checked_client, io_error, normalize_policy, parse_url, read_body};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

pub use roomkit_tools_http::NetworkPolicy;
#[cfg(feature = "exa")]
mod exa;
#[cfg(feature = "ollama")]
mod ollama;
#[cfg(feature = "openai")]
mod openai;
#[cfg(feature = "exa")]
pub use exa::ExaSearch;
#[cfg(feature = "ollama")]
pub use ollama::OllamaSearch;
#[cfg(feature = "openai")]
pub use openai::{OpenAISearch, OpenAISearchConfig};

/// Host-only transport configuration; never exposed in model arguments.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchHttpConfig {
    /// Override service URL, primarily for local tests or an explicit proxy.
    pub endpoint: Option<String>,
    /// Exact origin policy and explicit private-network exceptions.
    pub policy: NetworkPolicy,
    /// Decoded HTTP response limit, default 512 KiB, maximum 16 MiB.
    pub max_response_bytes: usize,
}
impl Default for SearchHttpConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            policy: NetworkPolicy::default(),
            max_response_bytes: 512 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
struct Transport {
    endpoint: Url,
    policy: NetworkPolicy,
    headers: HeaderMap,
    max_response_bytes: usize,
    decoders: Arc<Semaphore>,
}
impl Transport {
    fn new(default: &str, config: SearchHttpConfig, headers: HeaderMap) -> Result<Self, ToolError> {
        let raw = config.endpoint.as_deref().unwrap_or(default);
        let endpoint = parse_url(raw, 8192)?;
        let policy = normalize_policy(config.policy, 8192)?;
        if endpoint.query().is_some() || raw.contains('#') {
            return Err(ToolError::InvalidInput(
                "search endpoints cannot contain queries or fragments".into(),
            ));
        }
        if endpoint.scheme() != "https"
            && !policy
                .private_origins
                .contains(&endpoint.origin().ascii_serialization())
        {
            return Err(ToolError::AccessRefused(
                "search requires HTTPS or an explicit private origin".into(),
            ));
        }
        if config.max_response_bytes == 0 || config.max_response_bytes > 16 * 1024 * 1024 {
            return Err(ToolError::InvalidInput(
                "invalid search response byte limit".into(),
            ));
        }
        Ok(Self {
            endpoint,
            policy,
            headers,
            max_response_bytes: config.max_response_bytes,
            decoders: Arc::new(Semaphore::new(2)),
        })
    }

    async fn post<D>(
        &self,
        body: serde_json::Value,
        decode: D,
        cancel: &CancellationToken,
    ) -> Result<BackendOutput, ToolError>
    where
        D: FnOnce(&[u8], &str) -> Result<BackendOutput, ToolError> + Send + 'static,
    {
        let client = checked_client(&self.endpoint, &self.policy).await?;
        let bytes = serde_json::to_vec(&body)
            .map_err(|_| ToolError::InvalidInput("invalid search request".into()))?;
        let mut response = client
            .post(self.endpoint.clone())
            .headers(self.headers.clone())
            .header(header::CONTENT_TYPE, "application/json")
            .body(bytes)
            .send()
            .await
            .map_err(|e| io_error("send search request", e))?;
        let status = response.status().as_u16();
        match status {
            200..=299 => (),
            300..=399 => {
                return Err(ToolError::AccessRefused(
                    "search redirects are not followed".into(),
                ));
            }
            401 | 403 => {
                return Err(ToolError::AccessRefused(
                    "search service rejected authentication or access".into(),
                ));
            }
            429 => {
                return Err(ToolError::LimitExceeded(
                    "search service rate limit exceeded".into(),
                ));
            }
            _ => {
                return Err(ToolError::UnsupportedResource(format!(
                    "search service returned HTTP {status}"
                )));
            }
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let bytes = read_body(&mut response, self.max_response_bytes).await?;
        let slot = Arc::clone(&self.decoders)
            .acquire_owned()
            .await
            .map_err(|e| io_error("acquire search decoder", e))?;
        let token = cancel.clone();
        tokio::task::spawn_blocking(move || {
            let _slot = slot;
            if token.is_cancelled() {
                return Err(ToolError::Cancelled);
            }
            let result = decode(&bytes, &content_type);
            if token.is_cancelled() {
                return Err(ToolError::Cancelled);
            }
            result
        })
        .await
        .map_err(|e| io_error("join search decoding", e))?
    }
}

fn credential(value: &str, bearer: bool) -> Result<HeaderValue, ToolError> {
    if value.trim().is_empty() || value.trim() != value {
        return Err(ToolError::InvalidInput(
            "search API key must be nonempty without surrounding whitespace".into(),
        ));
    }
    let text = if bearer {
        format!("Bearer {value}")
    } else {
        value.to_owned()
    };
    let mut header = HeaderValue::from_str(&text)
        .map_err(|_| ToolError::InvalidInput("invalid search API key header".into()))?;
    header.set_sensitive(true);
    Ok(header)
}

fn mime(value: &str) -> &str {
    value.split(';').next().unwrap_or("").trim()
}
fn invalid() -> ToolError {
    ToolError::UnsupportedResource("unsupported or malformed search service response".into())
}
