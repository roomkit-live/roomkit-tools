//! Syntax outlines for Rust and Python files, independent of any agent runtime.
//!
//! Tree-sitter parses bounded UTF-8 input without executing code. This is not
//! type resolution, macro expansion, an LSP server or a persistent index.

#![forbid(unsafe_code)]

mod extract;
mod page;
mod source;
#[cfg(test)]
mod tests;
mod types;

pub use types::{
    CodeLanguage, CodeOutlineInput, CodeOutlineLimits, CodeOutlinePage, CodeSymbol, SymbolKind,
};

use std::num::NonZeroU64;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use roomkit_tools_core::{Definition, Output, ToolError, WorkspaceRoot};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

/// Read-only syntax outlines under a canonical host-selected workspace root.
///
/// Clones share two blocking-worker slots. Cancellation/deadlines are checked
/// cooperatively during parsing and traversal; workers retain their slot until
/// they stop. Path checks assume a host-controlled workspace, not hostile path
/// replacement or an operating-system sandbox.
#[derive(Clone, Debug)]
pub struct CodeOutlineTool {
    root: WorkspaceRoot,
    limits: CodeOutlineLimits,
    workers: Arc<Semaphore>,
}

impl CodeOutlineTool {
    /// Resolve the workspace root without changing the working directory.
    ///
    /// # Errors
    /// Returns an I/O error for an inaccessible path or invalid input for a file.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, ToolError> {
        Ok(Self {
            root: source::root(root.as_ref()).await?,
            limits: CodeOutlineLimits::default(),
            workers: Arc::new(Semaphore::new(2)),
        })
    }

    /// Configure positive host-owned bounds.
    ///
    /// # Errors
    /// Rejects zero limits, unrepresentable file bounds and deadlines over 24 hours.
    pub fn with_limits(mut self, limits: CodeOutlineLimits) -> Result<Self, ToolError> {
        if [
            limits.max_file_bytes,
            limits.max_output_bytes,
            limits.max_symbols,
            limits.max_scan_symbols,
            limits.max_scan_nodes,
            limits.max_depth,
            limits.max_signature_bytes,
            limits.default_timeout_ms,
            limits.max_timeout_ms,
        ]
        .contains(&0)
            || limits.default_timeout_ms > limits.max_timeout_ms
            || limits.max_timeout_ms > 86_400_000
            || u64::try_from(limits.max_file_bytes).map_or(true, |n| n == u64::MAX)
        {
            return Err(ToolError::InvalidInput(
                "outline limits must be positive and representable; deadlines at most 24 hours"
                    .into(),
            ));
        }
        self.limits = limits;
        Ok(self)
    }

    /// Schemas derived from the actual Rust request and result types.
    pub fn definition() -> Definition {
        Definition::new::<CodeOutlineInput, CodeOutlinePage>(
            "code_outline",
            "Outline a Rust or Python file: definitions with signatures, parents and inclusive line ranges. Read one with read(offset=start_line, limit=end_line-start_line+1). Check syntax_errors and signature_truncated. offset counts symbols, not lines; continue with next_offset and expected_revision. Syntax only: no types, imports or macros.",
        )
    }

    /// Analyze one bounded source file and return a page of definitions.
    ///
    /// # Errors
    /// Distinguishes invalid input, paths outside the root, unsupported source,
    /// I/O failures, exceeded budgets, stale revisions and cancellation.
    pub async fn outline(
        &self,
        input: CodeOutlineInput,
        cancel: &CancellationToken,
    ) -> Result<Output<CodeOutlinePage>, ToolError> {
        let timeout = input
            .timeout_ms
            .map_or(self.limits.default_timeout_ms as u64, NonZeroU64::get);
        let max = self.limits.max_timeout_ms as u64;
        if timeout > max {
            return Err(ToolError::InvalidInput(format!(
                "timeout_ms exceeds the host maximum of {max} ms"
            )));
        }
        let duration = Duration::from_millis(timeout);
        extract::validate_revision(input.expected_revision.as_deref())?;
        let token = cancel.child_token();
        let _guard = token.clone().drop_guard();
        let deadline = Instant::now() + duration;
        tokio::select! {
            biased;
            () = token.cancelled() => Err(ToolError::Cancelled),
            result = tokio::time::timeout(duration, self.run(input, token.clone(), deadline)) =>
                result.map_err(|_| ToolError::LimitExceeded("code outline deadline exceeded".into()))?,
        }
    }

    async fn run(
        &self,
        input: CodeOutlineInput,
        token: CancellationToken,
        deadline: Instant,
    ) -> Result<Output<CodeOutlinePage>, ToolError> {
        let permit = Arc::clone(&self.workers)
            .acquire_owned()
            .await
            .map_err(|_| ToolError::Cancelled)?;
        let (path, language, text) = source::load(
            &self.root,
            &input.path,
            input.language,
            self.limits.max_file_bytes,
        )
        .await?;
        let path = self.root.relative(&path)?;
        let limits = self.limits;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let control = extract::Control { token, deadline };
            let data = extract::outline(&text, language, &input, limits, &control)?;
            page::render(path, language, data, &input, limits, &control)
        })
        .await
        .map_err(|_| ToolError::UnsupportedResource("outline worker failed".into()))?
    }
}

// The README's example is compiled and run as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
