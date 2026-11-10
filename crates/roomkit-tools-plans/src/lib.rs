//! Selectable CRUD tools for plans. Storage, persistence and scope are host-owned.
//!
//! Plans record declared progress; they neither schedule work nor prove completion.
//! The bundled memory backend is bounded and shared by explicit clones only.
#![forbid(unsafe_code)]

mod memory;
mod text;
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
    /// The lines a model reads of this value as a result, or `None` for a
    /// type rendered as its JSON.
    fn text(&self) -> Option<String> {
        None
    }
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
    let text = match data.text() {
        Some(text) => text,
        None => serde_json::to_string(&data).map_err(|e| ToolError::InvalidInput(e.to_string()))?,
    };
    Ok(Output::new(data, text))
}

/// Render an update's result: the whole plan, or with `step_changes`, its
/// revision and the steps they changed.
/// # Errors
/// Rejects a backend result that violates the common contract.
#[doc(hidden)]
pub fn render_update(input: &PlanUpdateInput, plan: Plan) -> Result<Output<Plan>, ToolError> {
    let Some(changes) = &input.step_changes else {
        return render(plan);
    };
    if input.title.is_some() || input.objective.is_some() {
        return render(plan);
    }
    plan.validate()?;
    let text = if plan.revision == input.expected_revision {
        text::unchanged(&plan)
    } else {
        let step_ids: Vec<String> = changes.iter().map(|c| c.step_id.clone()).collect();
        text::changed(&plan, &step_ids)
    };
    Ok(Output::new(plan, text))
}

/// The update `input` asks of `plan`: its `step_changes` applied to the
/// plan's steps into the complete `steps`, its other fields kept, and
/// checked as any update with `steps` is.
/// # Errors
/// Refuses an invalid plan, a revision other than the plan's, a change
/// naming a step the plan does not have, and a result past the bounds.
#[doc(hidden)]
pub fn resolve_update(
    plan: &Plan,
    mut input: PlanUpdateInput,
) -> Result<PlanUpdateInput, ToolError> {
    let Some(changes) = input.step_changes.take() else {
        return Ok(input);
    };
    plan.validate()?;
    validation::check_revision(plan, input.expected_revision)?;
    let mut steps = plan.steps.clone();
    for change in changes {
        let step = steps
            .iter_mut()
            .find(|step| step.step_id == change.step_id)
            .ok_or_else(|| {
                ToolError::InvalidInput(format!(
                    "no step {} in plan {}; plan_read lists its steps",
                    change.step_id, plan.plan_id
                ))
            })?;
        if let Some(status) = change.status {
            step.status = status;
        }
        if let Some(description) = change.description {
            step.description = description;
        }
    }
    input.steps = Some(steps);
    input.validate()?;
    resulting(plan, &input).validate()?;
    Ok(input)
}

/// The plan `input` makes of `plan`, for its bounds to be checked before
/// any backend stores it.
fn resulting(plan: &Plan, input: &PlanUpdateInput) -> Plan {
    let mut next = plan.clone();
    if let Some(steps) = &input.steps {
        next.steps.clone_from(steps);
    }
    if let Some(title) = &input.title {
        next.title.clone_from(title);
    }
    if let Some(objective) = &input.objective {
        next.objective.clone_from(objective);
    }
    next
}

pub(crate) fn cancelled(cancel: &CancellationToken) -> Result<(), ToolError> {
    if cancel.is_cancelled() {
        Err(ToolError::Cancelled)
    } else {
        Ok(())
    }
}

macro_rules! tool {
    (@facade $name:ident, $input:ty, $output:ty, $wire:literal, $description:literal) => {
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
        }
    };
    // Every tool's call: the same checks, then the tool's own serving of
    // the validated input by its backend.
    (@call $name:ident, $input:ty, $output:ty, $serve:expr) => {
        impl $name {
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
                $serve(&*self.backend, scope, input, cancel).await
            }
        }
    };
    ($name:ident, $input:ty, $output:ty, $method:ident, $wire:literal, $description:literal) => {
        tool!(@facade $name, $input, $output, $wire, $description);
        tool!(
            @call $name,
            $input,
            $output,
            async |backend: &dyn PlanBackend,
                   scope: &PlanScope,
                   input: $input,
                   cancel: &CancellationToken|
                   -> Result<Output<$output>, ToolError> {
                render(backend.$method(scope, &input, cancel).await?)
            }
        );
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
    @facade PlanUpdateTool,
    PlanUpdateInput,
    Plan,
    "plan_update",
    "Update a plan at its current `expected_revision`; omitted fields are kept. To change a step's status or description, give `step_changes`: only those steps change, and the result lists them. `steps` replaces the ENTIRE ordered list instead: include unchanged steps with their `step_id`; left-out steps are removed. After a revision conflict, read the plan again."
);
tool!(@call PlanUpdateTool, PlanUpdateInput, Plan, serve_update);

/// Serve a validated update: its `step_changes` resolved against the plan
/// the backend reads, so the backend receives the complete `steps`.
async fn serve_update(
    backend: &dyn PlanBackend,
    scope: &PlanScope,
    input: PlanUpdateInput,
    cancel: &CancellationToken,
) -> Result<Output<Plan>, ToolError> {
    let resolved = if input.step_changes.is_some() {
        let read = PlanReadInput {
            plan_id: input.plan_id.clone(),
        };
        let current = backend.read(scope, &read, cancel).await?;
        resolve_update(&current, input.clone())?
    } else {
        input.clone()
    };
    render_update(&input, backend.update(scope, &resolved, cancel).await?)
}
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
