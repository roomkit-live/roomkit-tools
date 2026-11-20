//! Which responses are text this tool returns, and how their bytes are read.

use reqwest::{Response, header};
use roomkit_tools_core::ToolError;

/// Application types read as text, besides `text/*` and the `+json`, `+xml`
/// and `+yaml` structured suffixes: source, configuration and data formats
/// servers commonly send under `application/`.
const TEXT_APPLICATION_TYPES: [&str; 13] = [
    "application/json",
    "application/xml",
    "application/xhtml+xml",
    "application/javascript",
    "application/x-javascript",
    "application/ecmascript",
    "application/yaml",
    "application/x-yaml",
    "application/toml",
    "application/x-sh",
    "application/x-shellscript",
    "application/x-ndjson",
    "application/ndjson",
];

/// Labels of Latin-1, and of windows-1252, which browsers read it as. Their
/// ASCII range is UTF-8's, so an ASCII body under them is read unchanged;
/// any other byte would need a decoder this tool does not carry.
const LATIN1_LABELS: [&str; 11] = [
    "iso-8859-1",
    "iso8859-1",
    "iso88591",
    "iso_8859-1",
    "latin1",
    "latin-1",
    "l1",
    "cp819",
    "windows-1252",
    "cp1252",
    "x-cp1252",
];

/// A refusal shows at most this much of a server's Content-Type.
const SHOWN_TYPE_BYTES: usize = 256;

/// How a text response's body is read.
pub(crate) struct Text {
    /// Converted from HTML, rather than returned as it is.
    pub(crate) html: bool,
    /// Declared Latin-1: read only when every byte is ASCII.
    ascii_only: bool,
}

/// What the final response declares. Every refusal names both, so a model
/// knows what it got, and a 404 keeps its status.
pub(crate) struct Head {
    /// The Content-Type header, when the response has one.
    pub(crate) content_type: Option<String>,
    /// The HTTP status code.
    pub(crate) status: u16,
}

impl Head {
    /// Read the status and Content-Type of `response`.
    ///
    /// # Errors
    /// Refuses a Content-Type that is not visible ASCII.
    pub(crate) fn of(response: &Response) -> Result<Self, ToolError> {
        let status = response.status().as_u16();
        let content_type = match response.headers().get(header::CONTENT_TYPE) {
            None => None,
            Some(value) => Some(
                value
                    .to_str()
                    .map_err(|_| {
                        ToolError::UnsupportedResource(format!(
                            "response Content-Type is not ASCII (HTTP {status})"
                        ))
                    })?
                    .to_owned(),
            ),
        };
        Ok(Self {
            content_type,
            status,
        })
    }

    /// Whether the declared type and charset are text this tool reads. A
    /// response without a type is read as plain text when its body is UTF-8.
    ///
    /// # Errors
    /// Refuses other types and charsets, naming the Content-Type and status.
    pub(crate) fn text(&self) -> Result<Text, ToolError> {
        let mut parts = self.content_type.as_deref().unwrap_or("").split(';');
        let mime = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        if mime.is_empty() {
            return Ok(Text {
                html: false,
                ascii_only: false,
            });
        }
        if !is_text(&mime) {
            return Err(self.refuse("response type is not supported text"));
        }
        let mut ascii_only = false;
        for part in parts {
            let Some((key, value)) = part.trim().split_once('=') else {
                continue;
            };
            if !key.trim().eq_ignore_ascii_case("charset") {
                continue;
            }
            let charset = value.trim().trim_matches('"').to_ascii_lowercase();
            if LATIN1_LABELS.contains(&charset.as_str()) {
                ascii_only = true;
            } else if !matches!(charset.as_str(), "utf-8" | "utf8" | "us-ascii") {
                return Err(self.refuse("response charset is not UTF-8, ASCII or Latin-1"));
            }
        }
        Ok(Text {
            html: matches!(mime.as_str(), "text/html" | "application/xhtml+xml"),
            ascii_only,
        })
    }

    /// The body as text, without a leading byte order mark.
    ///
    /// # Errors
    /// Refuses a body that is not UTF-8, holds NUL, or is not ASCII under a
    /// Latin-1 charset, naming the Content-Type and status.
    pub(crate) fn decode(&self, bytes: Vec<u8>, text: &Text) -> Result<String, ToolError> {
        if text.ascii_only && !bytes.is_ascii() {
            return Err(
                self.refuse("Latin-1 response holds non-ASCII bytes; only UTF-8 is decoded")
            );
        }
        let source =
            String::from_utf8(bytes).map_err(|_| self.refuse("response is not UTF-8 text"))?;
        if source.contains('\0') {
            return Err(self.refuse("response contains NUL bytes"));
        }
        Ok(match source.strip_prefix('\u{feff}') {
            Some(rest) => rest.to_owned(),
            None => source,
        })
    }

    fn refuse(&self, problem: &str) -> ToolError {
        let declared = match self.content_type.as_deref().map(str::trim) {
            Some(value) if !value.is_empty() => format!("Content-Type: {}", shown(value)),
            _ => "no Content-Type".to_owned(),
        };
        ToolError::UnsupportedResource(format!("{problem} ({declared}, HTTP {})", self.status))
    }
}

fn is_text(mime: &str) -> bool {
    mime.starts_with("text/")
        || TEXT_APPLICATION_TYPES.contains(&mime)
        || (mime.starts_with("application/")
            && ["+json", "+xml", "+yaml"]
                .iter()
                .any(|suffix| mime.ends_with(suffix)))
}

fn shown(value: &str) -> String {
    match value.get(..SHOWN_TYPE_BYTES) {
        Some(head) if head.len() < value.len() => format!("{head}…"),
        _ => value.to_owned(),
    }
}
