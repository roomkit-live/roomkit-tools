use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Representation requested for a fetched textual resource.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FetchFormat {
    /// Convert HTML to Markdown; preserve other text as received.
    #[default]
    Markdown,
    /// Extract visible HTML text; preserve other text as received.
    Text,
    /// Return source text, including HTML markup, without executing it.
    Html,
}

/// Model-supplied request. Access policy and resource caps are host configuration.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WebFetchInput {
    /// Absolute HTTP or HTTPS URL, without embedded credentials.
    pub url: String,
    /// Output format, default markdown. JavaScript is never executed.
    #[serde(default)]
    pub format: FetchFormat,
    /// Total deadline in milliseconds, including DNS, redirects, body and conversion.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

impl WebFetchInput {
    /// Fetch one URL as Markdown using the host's default deadline.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            format: FetchFormat::default(),
            timeout_ms: None,
        }
    }
}

/// A fetched response. HTTP errors are observable responses, not transport failures.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct WebFetchResult {
    /// Normalized requested URL, with any fragment removed.
    pub url: String,
    /// URL of the final response after validated redirects.
    pub final_url: String,
    /// Number of followed redirects.
    pub redirects: usize,
    /// HTTP response status, including non-2xx responses.
    pub status: u16,
    /// Whether status is in the 200–299 range.
    pub http_ok: bool,
    /// Declared content type, including any charset parameter.
    pub content_type: String,
    /// Requested representation.
    pub format: FetchFormat,
    /// Decoded HTTP body bytes before UTF-8 decoding/conversion (gzip is supported).
    pub response_bytes: usize,
    /// UTF-8 bytes in the complete converted text before preview clipping.
    pub content_bytes: usize,
    /// Number of converted UTF-8 bytes omitted from the returned prefix.
    pub omitted_bytes: usize,
    /// Whether the content preview is incomplete; no archive or continuation is implied.
    pub truncated: bool,
    /// UTF-8 prefix that ends on a character boundary.
    pub content: String,
}

/// Host resource limits, independent of model arguments.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct WebFetchLimits {
    /// Maximum UTF-8 URL bytes, including each redirect. Default: 8 KiB.
    pub max_url_bytes: usize,
    /// Maximum decoded response body. Default: 5 MiB.
    pub max_response_bytes: usize,
    /// Maximum complete converted text; checked after conversion. Default: 5 MiB.
    pub max_converted_bytes: usize,
    /// Maximum returned content prefix, excluding metadata. Default: 50 KiB.
    pub max_output_bytes: usize,
    /// Followed redirects, 0–32. Default: 10.
    pub max_redirects: usize,
    /// Default total deadline. Default: 30,000 ms.
    pub default_timeout_ms: u64,
    /// Maximum requested deadline, at most 24 hours. Default: 120,000 ms.
    pub max_timeout_ms: u64,
}

impl Default for WebFetchLimits {
    fn default() -> Self {
        Self {
            max_url_bytes: 8192,
            max_response_bytes: 5 * 1024 * 1024,
            max_converted_bytes: 5 * 1024 * 1024,
            max_output_bytes: 50 * 1024,
            max_redirects: 10,
            default_timeout_ms: 30_000,
            max_timeout_ms: 120_000,
        }
    }
}

/// Host network scope shared with other HTTP tools.
pub use roomkit_tools_http::NetworkPolicy as WebFetchPolicy;
