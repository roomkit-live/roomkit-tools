use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Model arguments; provider, credentials and endpoints belong to the host.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WebSearchInput {
    /// Nonempty search query.
    pub query: String,
    /// Maximum retained documents or answer sources, default five. Cited sources must all fit.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Total deadline including DNS, HTTP and decoding.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}
const fn default_limit() -> usize {
    5
}
impl WebSearchInput {
    /// Search with five results and the host's default deadline.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            limit: default_limit(),
            timeout_ms: None,
        }
    }
}

/// A source returned by a backend; its contents are untrusted data.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchHit {
    /// Absolute HTTP(S) source URL.
    pub url: String,
    /// Title, when supplied by the service.
    pub title: Option<String>,
    /// Excerpt, not necessarily the full page.
    pub content: Option<String>,
    /// Known UTF-8 bytes omitted from the excerpt by this tool.
    #[serde(default)]
    pub omitted_bytes: usize,
}

/// Citation into an answer, using UTF-8 byte offsets (end exclusive).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchCitation {
    /// Index into the answer's sources array.
    pub source_index: usize,
    /// Inclusive byte offset; must be a character boundary.
    pub start: usize,
    /// Exclusive byte offset; must be a character boundary.
    pub end: usize,
}

/// Search results and generated answers have deliberately distinct shapes.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SearchPayload {
    /// Documents retrieved directly by a search service.
    Results {
        /// Ranked in provider order; may legitimately be empty.
        results: Vec<SearchHit>,
    },
    /// A model-generated answer grounded in retrieved sources.
    Answer {
        /// Complete answer; never clipped in a way that breaks citations.
        answer: String,
        /// Sources referenced by citation indexes.
        sources: Vec<SearchHit>,
        /// Verified offsets and source indexes, supplied by the backend.
        citations: Vec<SearchCitation>,
    },
    /// The provider did not execute a search (different from zero matches).
    NotPerformed {
        /// Observable reason, not a guessed answer.
        reason: String,
    },
}

/// Optional observed service usage. Missing values are unknown, never estimates.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SearchUsage {
    /// Completed search actions reported by the provider; not a query or billing count.
    pub search_requests: Option<u64>,
    /// Model input tokens reported by the provider.
    pub input_tokens: Option<u64>,
    /// Model output tokens reported by the provider.
    pub output_tokens: Option<u64>,
}

/// Result forms supported by a backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchKind {
    /// Direct documents.
    Results,
    /// Generated text with source annotations.
    Answer,
    /// Observable non-execution.
    NotPerformed,
}

/// Host-visible provider capabilities; unsupported arguments must fail explicitly.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchCapabilities {
    /// Response kinds this backend can produce.
    pub result_kinds: Vec<SearchKind>,
    /// Maximum requested document count, when there is a service ceiling.
    pub max_results: Option<usize>,
}
impl SearchCapabilities {
    /// A direct search service with a requested-document ceiling.
    pub fn direct(max_results: usize) -> Self {
        Self {
            result_kinds: vec![SearchKind::Results],
            max_results: Some(max_results),
        }
    }
}

/// Provider output before common bounds and rendering.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct BackendOutput {
    /// Typed result, answer or explicit non-execution.
    pub payload: SearchPayload,
    /// Provider reports incomplete results or relaxed constraints.
    #[serde(default)]
    pub partial: bool,
    /// Provider notices, preserved as untrusted data (including presentation metadata).
    #[serde(default)]
    pub notices: Vec<String>,
    /// Usage reported by the service; absent for built-in direct backends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<SearchUsage>,
}
impl BackendOutput {
    /// Complete direct results with no provider notices.
    pub fn results(results: Vec<SearchHit>) -> Self {
        Self {
            payload: SearchPayload::Results { results },
            partial: false,
            notices: vec![],
            usage: None,
        }
    }
}

/// Normalized tool output. Missing results do not imply absence on the web.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchResult {
    /// Backend selected by the host.
    pub provider: String,
    /// Query actually submitted.
    pub query: String,
    /// Typed result, answer or explicit non-execution.
    pub payload: SearchPayload,
    /// Provider reports incomplete results or relaxed constraints.
    #[serde(default)]
    pub partial: bool,
    /// Provider notices, preserved as untrusted data (including presentation metadata).
    #[serde(default)]
    pub notices: Vec<String>,
    /// Usage reported by the service; absent for built-in direct backends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<SearchUsage>,
    /// Known direct results or uncited answer sources omitted to respect the requested count.
    pub omitted_results: usize,
}

/// Host limits for the generic tool; transport limits belong to each backend.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchLimits {
    /// Maximum query UTF-8 bytes. Default 4096.
    pub max_query_bytes: usize,
    /// Maximum requested results. Default 10, hard ceiling 100.
    pub max_results: usize,
    /// Maximum excerpt bytes per source. Default 4096.
    pub max_content_bytes: usize,
    /// Maximum complete serialized Output (data and text). Default 128 KiB.
    pub max_output_bytes: usize,
    /// Default deadline, milliseconds. Default 30 seconds.
    pub default_timeout_ms: u64,
    /// Largest requested deadline, milliseconds. Default 120 seconds.
    pub max_timeout_ms: u64,
}
impl Default for SearchLimits {
    fn default() -> Self {
        Self {
            max_query_bytes: 4096,
            max_results: 10,
            max_content_bytes: 4096,
            max_output_bytes: 128 * 1024,
            default_timeout_ms: 30_000,
            max_timeout_ms: 120_000,
        }
    }
}
