//! Runtime-independent definitions, results and failures for agent tools.
//!
//! Tool implementations own their I/O. Hosts own their registry, permissions
//! and conversations. Input and output schemas are derived from the same
//! Rust types an implementation uses. [`WorkspaceRoot`] holds the lexical
//! path rules that root-configured tools share; it performs no I/O.

#![forbid(unsafe_code)]

pub mod identifiers;
pub mod revision;
mod workspace;

pub use workspace::WorkspaceRoot;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A tool's description and machine-readable input and output contracts.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct Definition {
    /// The tool's name before a host applies any namespace or alias.
    pub name: String,
    /// What the tool does, for the model and the host's catalogue.
    pub description: String,
    /// JSON Schema derived from the implementation's input type.
    pub input_schema: Value,
    /// JSON Schema for the structured output, excluding its model rendering.
    pub output_schema: Value,
}

impl Definition {
    /// Describe a tool using its implementation's `Input` and `Output` types.
    pub fn new<Input: JsonSchema, Output: JsonSchema>(
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema: schemars::schema_for!(Input).to_value(),
            output_schema: schemars::schema_for!(Output).to_value(),
        }
    }
}

/// Structured tool output alongside the text presented to a model.
///
/// Metadata such as pagination and truncation belongs to `T`, so callers do
/// not have to parse the rendered text to discover it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct Output<T> {
    /// The data a caller can process without parsing model-facing text.
    pub data: T,
    /// The tool's text rendering for a model.
    pub text: String,
}

impl<T> Output<T> {
    /// Keep the structured `data` and its model-facing `text` together.
    pub fn new(data: T, text: impl Into<String>) -> Self {
        Self {
            data,
            text: text.into(),
        }
    }
}

/// Failures the host can classify without inspecting an error message.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The input cannot be used; the operation did not run.
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    /// The requested operation is outside the tool's granted access.
    #[error("Access refused: {0}")]
    Refused(String),
    /// An explicit input or output bound was exceeded.
    #[error("Limit exceeded: {0}")]
    LimitExceeded(String),
    /// The content or resource is not supported by this tool.
    #[error("Unsupported resource: {0}")]
    Unsupported(String),
    /// The caller cancelled the operation.
    #[error("Operation cancelled")]
    Cancelled,
    /// An I/O operation failed; the source is for host diagnostics.
    #[error("Unable to {operation}")]
    Io {
        /// A short description that is safe to show as the public error.
        operation: &'static str,
        /// The underlying failure, available to the host's diagnostics.
        #[source]
        source: std::io::Error,
    },
}
