//! A portable note tool over a single host-selected memory namespace.
#![forbid(unsafe_code)]

mod files;
mod types;

pub use files::FileMemory;
pub use types::{MemoryInput, MemoryNote, MemoryResult};

use roomkit_tools_core::{Definition, Output, ToolError};
use std::{fmt::Debug, future::Future, pin::Pin, sync::Arc};
use tokio_util::sync::CancellationToken;

/// A cancellable memory operation on the configured namespace.
pub type MemoryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<MemoryResult, ToolError>> + Send + 'a>>;

/// The host binds the namespace, credentials and storage quota before registration.
pub trait MemoryBackend: Debug + Send + Sync {
    /// Execute a validated operation. Mutations must check revisions and quota atomically.
    fn execute<'a>(
        &'a self,
        input: &'a MemoryInput,
        cancel: &'a CancellationToken,
    ) -> MemoryFuture<'a>;
}

/// Read, maintain or remove durable project notes.
#[derive(Debug, Clone)]
pub struct MemoryTool {
    backend: Arc<dyn MemoryBackend>,
}

impl MemoryTool {
    /// Bind the tool to the host's selected backend.
    pub fn new(backend: Arc<dyn MemoryBackend>) -> Self {
        Self { backend }
    }

    /// The input and result schemas for the memory tool.
    pub fn definition() -> Definition {
        Definition::new::<MemoryInput, MemoryResult>(
            "memory",
            "Search project notes by words (empty query lists titles), read a note by id, save a short durable fact or forget a note. Save with id, title and text. Replacing or forgetting a note requires its current revision.",
        )
    }

    /// Validate a backend request without performing the operation.
    /// # Errors
    /// Refuses malformed metadata or fields outside the requested operation.
    pub fn validate_arguments(arguments: &MemoryInput) -> Result<(), ToolError> {
        arguments.validate()
    }

    /// Validate a backend response before handing it to a caller.
    /// # Errors
    /// Refuses invalid, oversized or unrelated records.
    pub fn validate_result(
        arguments: &MemoryInput,
        result: &MemoryResult,
    ) -> Result<(), ToolError> {
        arguments.validate()?;
        result.validate(arguments)
    }

    /// Validate both sides of the backend boundary and return the common envelope.
    /// # Errors
    /// Returns invalid arguments, invalid backend results, access, storage or cancellation errors.
    pub async fn call(
        &self,
        input: MemoryInput,
        cancel: &CancellationToken,
    ) -> Result<Output<MemoryResult>, ToolError> {
        input.validate()?;
        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let result = self.backend.execute(&input, cancel).await?;
        result.validate(&input)?;
        let text =
            serde_json::to_string(&result).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        Ok(Output::new(result, text))
    }
}
