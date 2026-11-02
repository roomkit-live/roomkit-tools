use super::{
    BackendOutput, HeaderMap, SearchBackend, SearchFuture, SearchHttpConfig, SearchInput,
    ToolError, Transport, credential, header, invalid, mime,
};
use crate::SearchHit;
use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// Ollama's hosted search API, independent of the agent's LLM provider.
#[derive(Clone, Debug)]
pub struct OllamaSearch {
    transport: Transport,
}
impl OllamaSearch {
    /// Construct with an explicit Ollama API key. No environment lookup or I/O.
    /// # Errors
    /// Rejects malformed/empty credentials or invalid host transport configuration.
    pub fn new(api_key: &str, config: SearchHttpConfig) -> Result<Self, ToolError> {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, credential(api_key, true)?);
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/json"),
        );
        Ok(Self {
            transport: Transport::new("https://ollama.com/api/web_search", config, headers)?,
        })
    }
}
impl SearchBackend for OllamaSearch {
    fn capabilities(&self) -> crate::SearchCapabilities {
        crate::SearchCapabilities::direct(10)
    }
    fn name(&self) -> &'static str {
        "ollama"
    }
    fn search<'a>(
        &'a self,
        input: &'a SearchInput,
        cancel: &'a CancellationToken,
    ) -> SearchFuture<'a> {
        Box::pin(async move {
            if input.limit > 10 {
                return Err(ToolError::InvalidInput(
                    "Ollama supports at most 10 results".into(),
                ));
            }
            self.transport
                .post(
                    json!({"query": input.query, "max_results": input.limit}),
                    decode,
                    cancel,
                )
                .await
        })
    }
}
#[derive(Deserialize)]
struct Response {
    results: Vec<Hit>,
}
#[derive(Deserialize)]
struct Hit {
    title: String,
    url: String,
    content: String,
}
fn decode(bytes: &[u8], content_type: &str) -> Result<BackendOutput, ToolError> {
    if !mime(content_type).eq_ignore_ascii_case("application/json") {
        return Err(invalid());
    }
    let response: Response = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    Ok(BackendOutput::results(
        response
            .results
            .into_iter()
            .map(|h| SearchHit {
                url: h.url,
                title: Some(h.title),
                content: Some(h.content),
                omitted_bytes: 0,
            })
            .collect(),
    ))
}
