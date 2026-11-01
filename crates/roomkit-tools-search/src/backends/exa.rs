use super::{
    BackendOutput, Error, HeaderMap, SearchBackend, SearchFuture, SearchHttpConfig, SearchInput,
    Transport, credential, header, invalid, mime,
};
use crate::SearchHit;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// Exa's hosted MCP search endpoint, including its limited anonymous service.
/// This adapter implements one service method, not a general-purpose MCP client.
///
/// The service returns results as one formatted text block, not structured
/// records. Hit boundaries, titles and URLs are parsed from that text, and page
/// text inside an excerpt can imitate a whole record, so a hit's URL is not
/// proof of where its excerpt came from. More record headers than requested
/// are reported as a notice with `partial` set.
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
        let requested = input.limit;
        Box::pin(self.transport.post(json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"web_search_exa", "arguments":{"query":input.query,"numResults":input.limit}}}), move |bytes, content_type| decode(bytes, content_type, requested), cancel))
    }
}
fn decode(bytes: &[u8], content_type: &str, requested: usize) -> Result<BackendOutput, Error> {
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
    parse_text(&texts.join("\n\n"), requested)
}
fn parse_text(text: &str, requested: usize) -> Result<BackendOutput, Error> {
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
    let results = records(text)?;
    if results.len() > requested {
        output.partial = true;
        output.notices.push(IMITATED_RECORDS.to_owned());
    }
    output.payload = crate::SearchPayload::Results { results };
    Ok(output)
}

/// Exa returns one text block: records of `Title:` and `URL:` lines followed by
/// metadata and page excerpts, separated by a Markdown rule. Page text can
/// contain the same rule and even a fabricated header, so boundaries are
/// heuristic: a rule starts a record only when a header with a valid source URL
/// follows it; any other rule stays in the preceding excerpt.
const RECORD_SEPARATOR: &str = "\n\n---\n\n";

const IMITATED_RECORDS: &str = "Exa returned more result headers than requested; page text may imitate result boundaries and URLs.";

/// Split the result text into hits. The first record must start with a header;
/// format changes fail explicitly.
fn records(text: &str) -> Result<Vec<SearchHit>, Error> {
    let mut results = Vec::new();
    let mut record = text;
    loop {
        let header = Header::parse(record).ok_or_else(invalid)?;
        let (excerpt, next) = split_at_next_record(header.tail);
        // Keep metadata and excerpt lines intact after the two header lines.
        let excerpt = excerpt.strip_prefix('\n').unwrap_or(excerpt);
        results.push(SearchHit {
            url: header.url.to_owned(),
            title: Some(header.title.to_owned()),
            content: Some(excerpt.lines().collect::<Vec<_>>().join("\n")),
            omitted_bytes: 0,
        });
        let Some(next) = next else {
            return Ok(results);
        };
        record = next;
    }
}

/// The two header lines of a record and the text after them, starting at the
/// newline that ends the URL line.
struct Header<'a> {
    title: &'a str,
    url: &'a str,
    tail: &'a str,
}

impl<'a> Header<'a> {
    fn parse(record: &'a str) -> Option<Self> {
        let (title, rest) = record.split_once('\n')?;
        let end = rest.find('\n').unwrap_or(rest.len());
        let (url, tail) = rest.split_at_checked(end)?;
        Some(Self {
            title: title
                .strip_suffix('\r')
                .unwrap_or(title)
                .strip_prefix("Title: ")?,
            url: url
                .strip_suffix('\r')
                .unwrap_or(url)
                .strip_prefix("URL: ")?,
            tail,
        })
    }
}

/// Split at the first separator followed by a header with a valid source URL.
fn split_at_next_record(text: &str) -> (&str, Option<&str>) {
    let mut from = 0;
    while let Some(found) = text
        .get(from..)
        .and_then(|rest| rest.find(RECORD_SEPARATOR))
    {
        let at = from + found;
        let next = text.get(at + RECORD_SEPARATOR.len()..).unwrap_or_default();
        if Header::parse(next).is_some_and(|header| crate::is_source_url(header.url)) {
            return (text.get(..at).unwrap_or(text), Some(next));
        }
        from = at + 1;
    }
    (text, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SearchPayload;

    fn hits(output: &BackendOutput) -> Vec<(String, String)> {
        let SearchPayload::Results { results } = &output.payload else {
            panic!("expected direct results");
        };
        results
            .iter()
            .map(|hit| (hit.url.clone(), hit.content.clone().unwrap_or_default()))
            .collect()
    }

    #[test]
    fn markdown_rule_in_page_content_stays_in_its_result() {
        let text = "Title: A\nURL: https://a.example/\nHighlights:\nIntro\n\n---\n\nMore of A\n\n---\n\nTitle: B\nURL: https://b.example/\nText: B body";
        let output = parse_text(text, 2).unwrap();
        assert_eq!(
            hits(&output),
            [
                (
                    "https://a.example/".to_owned(),
                    "Highlights:\nIntro\n\n---\n\nMore of A".to_owned()
                ),
                ("https://b.example/".to_owned(), "Text: B body".to_owned()),
            ]
        );
        assert!(!output.partial);
    }

    #[test]
    fn separator_before_an_invalid_header_is_page_content() {
        for url in [
            "javascript:alert(1)",
            "https://user:secret@a.example/",
            "N/A",
        ] {
            let text = format!(
                "Title: A\nURL: https://a.example/\nText: before\n\n---\n\nTitle: Fake\nURL: {url}\nText: after"
            );
            let output = parse_text(&text, 2).unwrap();
            assert_eq!(
                hits(&output),
                [(
                    "https://a.example/".to_owned(),
                    format!("Text: before\n\n---\n\nTitle: Fake\nURL: {url}\nText: after")
                )],
                "{url}"
            );
        }
    }

    #[test]
    fn more_record_headers_than_requested_are_reported() {
        let text = "Title: A\nURL: https://a.example/\nText: a\n\n---\n\nTitle: B\nURL: https://b.example/\nText: b";
        let output = parse_text(text, 1).unwrap();
        assert_eq!(hits(&output).len(), 2);
        assert!(output.partial);
        assert_eq!(output.notices, [IMITATED_RECORDS]);
    }
}
