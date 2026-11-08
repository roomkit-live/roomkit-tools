use std::num::{NonZeroU64, NonZeroUsize};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A supported source language.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodeLanguage {
    /// Rust (`.rs`).
    Rust,
    /// Python (`.py`, `.pyi`).
    Python,
}

/// A page of a source file's outline.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeOutlineInput {
    /// File in the workspace.
    pub path: String,
    /// Overrides the language inferred from the extension.
    #[serde(default)]
    pub language: Option<CodeLanguage>,
    /// First symbol in source order (not a line).
    #[serde(default = "first_symbol")]
    pub offset: NonZeroUsize,
    /// Symbol count, capped by the host.
    #[serde(default)]
    pub limit: Option<NonZeroUsize>,
    /// Revision from read or `code_outline`; refuses a changed file.
    #[serde(default)]
    pub expected_revision: Option<String>,
    /// Total deadline in ms; one above the host maximum is refused.
    #[serde(default)]
    pub timeout_ms: Option<NonZeroU64>,
}

const fn first_symbol() -> NonZeroUsize {
    NonZeroUsize::MIN
}

impl CodeOutlineInput {
    /// Infer the language and request the first page with host defaults.
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            language: None,
            offset: first_symbol(),
            limit: None,
            expected_revision: None,
            timeout_ms: None,
        }
    }
}

/// Host-owned bounds; parser allocations are not a hard memory sandbox.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CodeOutlineLimits {
    /// Maximum UTF-8 source bytes retained for parsing. Default: 512 KiB.
    /// Parser memory grows with the input: deeply nested invalid source can
    /// need about 290 bytes per input byte before traversal bounds apply.
    pub max_file_bytes: usize,
    /// Maximum complete serialized data-plus-text output, the text's closing
    /// line of facts apart. Default: 64 KiB.
    pub max_output_bytes: usize,
    /// Maximum symbols per page. Default: 200.
    pub max_symbols: usize,
    /// Maximum definitions discovered in one file. Default: 5,000.
    pub max_scan_symbols: usize,
    /// Maximum syntax nodes traversed. Default: 200,000.
    pub max_scan_nodes: usize,
    /// Maximum syntax-tree traversal depth. Default: 256.
    pub max_depth: usize,
    /// Maximum UTF-8 bytes per signature; clipping is explicitly reported.
    /// Names above this limit fail rather than silently losing their identity.
    pub max_signature_bytes: usize,
    /// Default overall deadline. Default: 10 seconds.
    pub default_timeout_ms: usize,
    /// Maximum allowed deadline. Default: 120 seconds; at most 24 hours.
    pub max_timeout_ms: usize,
}

impl Default for CodeOutlineLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 512 * 1024,
            max_output_bytes: 64 * 1024,
            max_symbols: 200,
            max_scan_symbols: 5_000,
            max_scan_nodes: 200_000,
            max_depth: 256,
            max_signature_bytes: 1_024,
            default_timeout_ms: 10_000,
            max_timeout_ms: 120_000,
        }
    }
}

/// Syntactic definition kinds; these are not resolved semantic symbols.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    /// A free or locally nested function.
    Function,
    /// A method in a class, trait or impl block.
    Method,
    /// A Python class.
    Class,
    /// A Rust struct.
    Struct,
    /// A Rust enum, without enumerating individual variants.
    Enum,
    /// A Rust trait.
    Trait,
    /// A Rust impl block; its name is the implemented type.
    Impl,
    /// A Rust module declaration; external files are not followed.
    Module,
    /// A Rust type alias.
    TypeAlias,
}

/// One definition, with a complete source range and a bounded header.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CodeSymbol {
    /// One-based symbol index, stable only for this file revision.
    pub index: usize,
    /// Nearest enclosing definition's index, possibly on a previous page.
    pub parent: Option<usize>,
    /// The syntactic definition category.
    pub kind: SymbolKind,
    /// Declared name (implemented type for Rust impl blocks).
    pub name: String,
    /// Original header without the body, possibly clipped at a UTF-8 boundary.
    pub signature: String,
    /// Whether signature bytes were omitted; read the source for the full header.
    pub signature_truncated: bool,
    /// First source line, one-based; Python decorators are included.
    pub start_line: usize,
    /// Last source line, inclusive. Use `read(offset=start_line, limit=end-start+1)`.
    pub end_line: usize,
}

/// A page of structural source information, not a compiler diagnostic report.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CodeOutlinePage {
    /// Workspace-relative path of the outlined file, usable by `read`.
    pub path: String,
    /// Selected or inferred parser language.
    pub language: CodeLanguage,
    /// SHA-256 of raw bytes, compatible with read/write/edit revisions.
    pub revision: String,
    /// Definitions in source order with explicit parent indexes.
    pub symbols: Vec<CodeSymbol>,
    /// Total discovered definitions, including those outside this page.
    pub total_symbols: usize,
    /// Tree-sitter recovered syntax errors; output may omit invalid definitions.
    pub syntax_errors: bool,
    /// More symbols remain; independent of syntax errors or clipped signatures.
    pub truncated: bool,
    /// Next one-based symbol offset, or None after the last definition.
    pub next_offset: Option<usize>,
}
