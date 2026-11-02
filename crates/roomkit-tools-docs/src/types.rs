use std::num::NonZeroUsize;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn list_limit() -> usize {
    20
}
fn search_limit() -> usize {
    10
}
fn read_limit() -> usize {
    100
}
fn first_line() -> NonZeroUsize {
    NonZeroUsize::MIN
}
fn first_result() -> NonZeroUsize {
    NonZeroUsize::MIN
}

/// Discover documents in the host-configured catalogue.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocsListInput {
    /// First result, starting at one, as `list`, `glob` and `grep`.
    #[serde(default = "first_result")]
    pub offset: NonZeroUsize,
    /// Maximum results, 1..100; defaults to 20.
    #[serde(default = "list_limit")]
    pub limit: usize,
    /// Refuse a different catalogue version when continuing a page.
    #[serde(default)]
    pub expected_catalog_revision: Option<String>,
}
/// The first page of 20, unchecked, as Python's `FileDocs.list()`.
impl Default for DocsListInput {
    fn default() -> Self {
        Self {
            offset: first_result(),
            limit: list_limit(),
            expected_catalog_revision: None,
        }
    }
}
/// Literal case-insensitive search, with all whitespace-separated terms required.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocsSearchInput {
    /// Up to 16 terms and 1024 UTF-8 bytes; no regex or query operators.
    pub query: String,
    /// First result, starting at one, as `list`, `glob` and `grep`.
    #[serde(default = "first_result")]
    pub offset: NonZeroUsize,
    /// Maximum document hits, 1..50; defaults to 10.
    #[serde(default = "search_limit")]
    pub limit: usize,
    /// Refuse a different catalogue revision when continuing a search.
    #[serde(default)]
    pub expected_catalog_revision: Option<String>,
}
impl DocsSearchInput {
    /// The first 10 hits for `query`, unchecked, as Python's
    /// `FileDocs.search(query=...)`.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            offset: first_result(),
            limit: search_limit(),
            expected_catalog_revision: None,
        }
    }
}
/// Read an identified document, optionally checking its content revision.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocsReadInput {
    /// Copy an ID returned by discovery or search, not an arbitrary path or URL.
    pub document_id: String,
    /// First line, one-based; defaults to 1.
    #[serde(default = "first_line")]
    pub offset: NonZeroUsize,
    /// Maximum lines, 1..500; defaults to 100. A byte budget may shorten the page.
    #[serde(default = "read_limit")]
    pub limit: usize,
    /// Document revision returned by discovery/search/read.
    #[serde(default)]
    pub expected_revision: Option<String>,
}
impl DocsReadInput {
    /// The first 100 lines of `document_id`, unchecked, as Python's
    /// `FileDocs.read(document_id=...)`.
    pub fn new(document_id: impl Into<String>) -> Self {
        Self {
            document_id: document_id.into(),
            offset: first_line(),
            limit: read_limit(),
            expected_revision: None,
        }
    }
}
/// Provenance of a document; documentation does not grant execution capabilities.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DocumentInfo {
    /// Stable identifier in this backend, at most 512 UTF-8 bytes.
    pub document_id: String,
    /// Human-readable title, at most 256 UTF-8 bytes.
    pub title: String,
    /// Reference selected by the host backend, at most 2048 UTF-8 bytes.
    pub source: String,
    /// Optional host-declared documentation version (not inferred from a package).
    pub version: Option<String>,
    /// SHA-256 of the original document bytes.
    pub revision: String,
    /// Total logical lines, including empty lines within the text.
    pub line_count: usize,
}
/// Paginated document catalogue.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocsListResult {
    /// Complete document metadata for this page.
    pub documents: Vec<DocumentInfo>,
    /// Revision of the complete catalogue.
    pub catalog_revision: String,
    /// Number of eligible documents in the complete catalogue.
    pub total_documents: usize,
    /// One-based first result of this page.
    pub offset: usize,
    /// One-based offset of the next page, or null at the end.
    pub next_offset: Option<usize>,
    /// Whether more records remain after this page.
    pub truncated: bool,
}
/// One matching document and a source excerpt.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentMatch {
    /// Metadata for subsequent reads and citations.
    pub document: DocumentInfo,
    /// One-based excerpt line, or null for a metadata-only match.
    pub line: Option<usize>,
    /// Matching source line, or title for a metadata-only match; at most 1024 bytes.
    pub text: String,
    /// UTF-8 bytes omitted from this excerpt.
    pub omitted_bytes: usize,
    /// Whether the excerpt was clipped; read the document for complete text.
    pub truncated: bool,
}
/// Paginated ranked document matches; one hit per document.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocsSearchResult {
    /// Ranked hits for this page.
    pub matches: Vec<DocumentMatch>,
    /// Revision of the complete catalogue.
    pub catalog_revision: String,
    /// Number of documents searched.
    pub total_documents: usize,
    /// Number of documents matching all query terms.
    pub matched_documents: usize,
    /// One-based first result of this page.
    pub offset: usize,
    /// One-based offset of the next page, or null at the end.
    pub next_offset: Option<usize>,
    /// Whether more matching documents remain after this page.
    pub truncated: bool,
}
/// Original logical document line, without its newline terminator.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentLine {
    /// One-based source line.
    pub number: usize,
    /// Complete line text.
    pub text: String,
}
/// A bounded page of complete source lines.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocsReadResult {
    /// Source metadata and content revision.
    pub document: DocumentInfo,
    /// Revision of the containing catalogue.
    pub catalog_revision: String,
    /// Complete source lines, never clipped within a line.
    pub lines: Vec<DocumentLine>,
    /// One-based first requested line.
    pub offset: usize,
    /// Next source line, or null at the end.
    pub next_offset: Option<usize>,
    /// Whether more source lines remain after this page.
    pub truncated: bool,
}
