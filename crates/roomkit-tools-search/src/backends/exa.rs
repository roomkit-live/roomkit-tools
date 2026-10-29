use super::{
    BackendOutput, Error, HeaderMap, SearchBackend, SearchFuture, SearchHttpConfig, SearchInput,
    Transport, credential, header, invalid, mime,
};
use crate::SearchHit;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// Exa's hosted MCP search endpoint, including its limited anonymous service.
/// This adapter implements one service method, not a general-purpose MCP client.
#[derive(Clone, Debug)]
pub struct ExaSearch {
    transport: Transport,
}
impl ExaSearch {
    /// Construct with an optional explicit API key; no environment lookup or I/O.
    /// # Errors
    /// Rejects malformed credentials or invalid host transport configuration.
    pub fn new(api_key: Option<&str>, config: SearchHttpConfig) -> Result<Self, Error> {
        let mut headers = HeaderMap::new();
        if let Some(key) = api_key {
            headers.insert("x-api-key", credential(key, false)?);
        }
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/json, text/event-stream"),
        );
        Ok(Self {
            transport: Transport::new("https://mcp.exa.ai/mcp", config, headers)?,
        })
    }
}
impl SearchBackend for ExaSearch {
    fn capabilities(&self) -> crate::SearchCapabilities {
        crate::SearchCapabilities::direct(100)
    }
    fn name(&self) -> &'static str {
        "exa"
    }
    fn search<'a>(
        &'a self,
        input: &'a SearchInput,
        cancel: &'a CancellationToken,
    ) -> SearchFuture<'a> {
        Box::pin(self.transport.post(json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"web_search_exa", "arguments":{"query":input.query,"numResults":input.limit}}}), decode, cancel))
    }
}
fn decode(bytes: &[u8], content_type: &str) -> Result<BackendOutput, Error> {
    let messages: Vec<Value> = if mime(content_type).eq_ignore_ascii_case("application/json") {
        vec![serde_json::from_slice(bytes).map_err(|_| invalid())?]
    } else if mime(content_type).eq_ignore_ascii_case("text/event-stream") {
        // SSE data lines are joined with newlines; comments/event/id fields are ignored.
        let text = std::str::from_utf8(bytes)
            .map_err(|_| invalid())?
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        let mut messages = Vec::new();
        for frame in text.split("\n\n") {
            let data = frame
                .lines()
                .filter_map(|line| {
                    if line == "data" {
                        Some("")
                    } else {
                        line.strip_prefix("data:")
                            .map(|v| v.strip_prefix(' ').unwrap_or(v))
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !data.is_empty() {
                messages.push(serde_json::from_str(&data).map_err(|_| invalid())?);
            }
        }
        messages
    } else {
        return Err(invalid());
    };
    let mut result = None;
    for message in messages {
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(invalid());
        }
        if message.get("id").is_none() && message.get("method").and_then(Value::as_str).is_some() {
            continue;
        }
        if message.get("id") != Some(&json!(1)) || result.is_some() {
            return Err(invalid());
        }
        if message.get("error").is_some() {
            return Err(Error::Unsupported("Exa JSON-RPC error".into()));
        }
        result = Some(message.get("result").cloned().ok_or_else(invalid)?);
    }
    let result = result.ok_or_else(invalid)?;
    if result
        .get("isError")
        .is_some_and(|v| v != &Value::Bool(false))
    {
        return Err(Error::Unsupported(
            "Exa search tool reported an error".into(),
        ));
    }
    let blocks = result
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    if blocks.is_empty() {
        return Err(invalid());
    }
    let mut texts = Vec::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("text") {
            return Err(invalid());
        }
        texts.push(
            block
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?,
        );
    }
    parse_text(&texts.join("\n\n"))
}
fn parse_text(text: &str) -> Result<BackendOutput, Error> {
    let mut text = text.trim();
    let mut notices = Vec::new();
    while let Some(note) = text.strip_prefix("Note: ") {
        let (line, remaining) = note.split_once('\n').ok_or_else(invalid)?;
        notices.push(line.to_owned());
        text = remaining.trim_start();
    }
    let mut output = BackendOutput::results(vec![]);
    output.partial = !notices.is_empty();
    output.notices = notices;
    if text == "No search results found. Please try a different query." {
        return Ok(output);
    }
    let mut results = Vec::new();
    for block in text.split("\n\n---\n\n") {
        let mut lines = block.lines();
        let title = lines
            .next()
            .and_then(|s| s.strip_prefix("Title: "))
            .ok_or_else(invalid)?;
        let url = lines
            .next()
            .and_then(|s| s.strip_prefix("URL: "))
            .ok_or_else(invalid)?;
        // Keep metadata and text intact; source format changes fail explicitly.
        let content = lines.collect::<Vec<_>>().join("\n");
        results.push(SearchHit {
            url: url.to_owned(),
            title: Some(title.to_owned()),
            content: Some(content),
            omitted_bytes: 0,
        });
    }
    output.payload = crate::SearchPayload::Results { results };
    Ok(output)
}
