//! Selectable CRUD tools for plans. Storage, persistence and scope are host-owned.
//!
//! Plans record declared progress; they neither schedule work nor prove completion.
//! The bundled memory backend is bounded and shared by explicit clones only.
#![forbid(unsafe_code)]

mod memory;
mod types;
mod validation;
pub use memory::{MemoryPlanBackend, MemoryPlanLimits};
pub use types::*;

use roomkit_tools_core::{Definition, Output, ToolError};
use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};
use std::{fmt::Debug, future::Future, pin::Pin, sync::Arc};
use tokio_util::sync::CancellationToken;

/// Object-safe host operation, independent of a specific async runtime.
pub type PlanFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ToolError>> + Send + 'a>>;

/// Injected storage. Implementations enforce scope on every call and atomic
/// revision checks on update/delete. IDs must never be reused within a backend.
/// Calls may commit before cancellation loses the receipt: read/list to recover,
/// never blindly retry create. No-op updates preserve revisions. Document durability.
pub trait PlanBackend: Debug + Send + Sync {
    /// Create a new owned plan.
    fn create<'a>(
        &'a self,
        scope: &'a PlanScope,
        input: &'a PlanCreateInput,
        cancel: &'a CancellationToken,
    ) -> PlanFuture<'a, Plan>;
    /// Read one owned snapshot. Missing and inaccessible IDs have the same refusal.
    fn read<'a>(
        &'a self,
        scope: &'a PlanScope,
        input: &'a PlanReadInput,
        cancel: &'a CancellationToken,
    ) -> PlanFuture<'a, Plan>;
    /// List all owned summaries, bounded to 64 without silent eviction.
    fn list<'a>(
        &'a self,
        scope: &'a PlanScope,
        input: &'a PlanListInput,
        cancel: &'a CancellationToken,
    ) -> PlanFuture<'a, PlanListResult>;
    /// Commit a partial field update atomically against `expected_revision`.
    fn update<'a>(
        &'a self,
        scope: &'a PlanScope,
        input: &'a PlanUpdateInput,
        cancel: &'a CancellationToken,
    ) -> PlanFuture<'a, Plan>;
    /// Delete the matching owned version. Does not cancel any execution.
    fn delete<'a>(
        &'a self,
        scope: &'a PlanScope,
        input: &'a PlanDeleteInput,
        cancel: &'a CancellationToken,
    ) -> PlanFuture<'a, PlanDeleteResult>;
}

/// Shared semantic validation for native and foreign hosts.
#[doc(hidden)]
pub trait PlanContract: Serialize + DeserializeOwned + JsonSchema {
    /// Validate fields and bounded payloads.
    /// # Errors
    /// Rejects invalid or oversized values.
    fn validate(&self) -> Result<(), ToolError>;
}

/// Decode and validate the implementation's actual input/output types.
/// # Errors
/// Rejects malformed, unknown or oversized fields.
#[doc(hidden)]
pub fn decode<T: PlanContract>(json: &str) -> Result<T, ToolError> {
    if json.len() > 256 * 1024 {
        return Err(ToolError::LimitExceeded(
            "plan input exceeds 256 KiB".into(),
        ));
    }
    let value: T =
        serde_json::from_str(json).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
    value.validate()?;
    Ok(value)
}

/// Render a validated record; structured data carries the full plan.
/// # Errors
/// Rejects a backend result that violates the common contract.
#[doc(hidden)]
pub fn render<T: PlanContract>(data: T) -> Result<Output<T>, ToolError> {
    data.validate()?;
    let text = serde_json::to_string(&data).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
    Ok(Output::new(data, text))
}

pub(crate) fn cancelled(cancel: &CancellationToken) -> Result<(), ToolError> {
    if cancel.is_cancelled() {
        Err(ToolError::Cancelled)
    } else {
        Ok(())
    }
}

macro_rules! tool {
    ($name:ident, $input:ty, $output:ty, $method:ident, $wire:literal, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Debug)]
        pub struct $name {
            backend: Arc<dyn PlanBackend>,
        }
        impl $name {
            /// Select this capability over an explicitly shared backend.
            pub fn new(backend: Arc<dyn PlanBackend>) -> Self {
                Self { backend }
            }
            /// Rust-derived input and output schemas.
            pub fn definition() -> Definition {
                Definition::new::<$input, $output>($wire, $description)
            }
            /// Validate, execute and return a structured receipt, as
            /// Python's `call(arguments, scope=...)`.
            /// # Errors
            /// Propagates invalid inputs, access/revision refusals, limits and backend errors.
            pub async fn call(
                &self,
                scope: &PlanScope,
                input: $input,
                cancel: &CancellationToken,
            ) -> Result<Output<$output>, ToolError> {
                scope.validate()?;
                input.validate()?;
                cancelled(cancel)?;
                render(self.backend.$method(scope, &input, cancel).await?)
            }
        }
    };
}
tool!(
    PlanCreateTool,
    PlanCreateInput,
    Plan,
    create,
    "plan_create",
    "Create a plan: title, objective and ordered steps with unique, stable `step_id` values. Returns `plan_id` and revision. If the reply is lost, check `plan_list` before creating again."
);
tool!(
    PlanReadTool,
    PlanReadInput,
    Plan,
    read,
    "plan_read",
    "Read a plan's steps and current revision."
);
tool!(
    PlanListTool,
    PlanListInput,
    PlanListResult,
    list,
    "plan_list",
    "List this conversation's plans as summaries; `plan_read` gives the steps."
);
tool!(
    PlanUpdateTool,
    PlanUpdateInput,
    Plan,
    update,
    "plan_update",
    "Update a plan at its current `expected_revision`; omitted fields are kept. `steps` replaces the ENTIRE ordered list: include unchanged steps with their `step_id`; left-out steps are removed. After a revision conflict, read the plan again."
);
tool!(
    PlanDeleteTool,
    PlanDeleteInput,
    PlanDeleteResult,
    delete,
    "plan_delete",
    "Delete a plan at its current `expected_revision`. Processes, delegated tasks and files are not affected."
);

// The README's example is compiled and run as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
