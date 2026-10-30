//! Stateless delegation facades over a host-owned backend.
//!
//! This crate neither runs models nor schedules tasks. Backends own profiles,
//! execution, scope checks, cancellation and cleanup. No runtime is required.

use std::{fmt::Debug, future::Future, pin::Pin, sync::Arc};

use roomkit_tools_core::{Definition, Error, Output};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Trusted host context, never accepted as a model argument.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentScope {
    /// Conversation owning the task.
    pub room_id: String,
    /// Principal allowed to manage the task.
    pub caller_id: String,
}

/// Select a host-defined profile and submit one task.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DelegateInput {
    /// Allowed profile name, not an instance or provider name.
    pub profile: String,
    /// Complete instructions for a fresh, isolated worker.
    pub task: String,
}

/// Inspect one owned task or all retained tasks in this scope.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatusInput {
    /// Omit to list retained tasks.
    pub task_id: Option<String>,
}

/// Request cancellation of one owned task.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelInput {
    /// Identifier returned by `delegate_task`.
    pub task_id: String,
}

/// Observed worker state, independent of delivery to the principal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// The worker has not ended.
    Running,
    /// The worker returned a result.
    Completed,
    /// Execution failed.
    Failed,
    /// Execution was cancelled.
    Cancelled,
}

/// Observed result delivery; completion alone does not imply a principal reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// Delivery has not completed.
    Pending,
    /// The framework reports successful delivery and a completed principal turn.
    Delivered,
    /// The delivery or principal turn failed.
    Failed,
    /// Delivery was explicitly suppressed, for example during shutdown.
    Suppressed,
}

/// Bounded receipt; task, instance and profile identities remain distinct.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskInfo {
    /// Opaque task identifier.
    pub task_id: String,
    /// Isolated execution instance, not the reusable profile.
    pub agent_id: String,
    /// Selected host profile.
    pub profile: String,
    /// Observed worker state.
    pub status: TaskState,
    /// Observed return-to-principal state.
    pub delivery: DeliveryState,
    /// Worker output or safe failure explanation, at most 8192 UTF-8 bytes.
    pub result: Option<String>,
    /// Whether the backend clipped the retained result.
    pub result_truncated: bool,
}

/// Retained tasks, limited to 64; no hidden eviction or pagination.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskList {
    /// Tasks visible in the trusted scope.
    pub tasks: Vec<TaskInfo>,
    /// Accepted launches still resolving configuration, without a task ID yet.
    #[serde(default)]
    pub pending_launches: usize,
}

/// Observed cancellation outcome; cancelling an ended task is harmless.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelResult {
    /// Current task state.
    pub task: TaskInfo,
    /// Whether this operation actually cancelled execution.
    pub cancelled: bool,
}

/// Object-safe future without choosing a runtime.
pub type AgentFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

/// Host extension point. Implementations must check scope on every operation,
/// bound their registries, and own cleanup independently of caller cancellation.
/// Dropping a delegate future can lose its receipt: recover with status before
/// retrying. A backend must document its own shutdown and persistence contract.
pub trait AgentBackend: Debug + Send + Sync {
    /// Resolve a permitted profile and launch a fresh instance.
    fn delegate<'a>(
        &'a self,
        scope: &'a AgentScope,
        input: &'a DelegateInput,
    ) -> AgentFuture<'a, TaskInfo>;
    /// Read one or all retained task records in this scope.
    fn status<'a>(
        &'a self,
        scope: &'a AgentScope,
        input: &'a StatusInput,
    ) -> AgentFuture<'a, TaskList>;
    /// Cancel an owned task, preserving cleanup if the caller goes away.
    fn cancel<'a>(
        &'a self,
        scope: &'a AgentScope,
        input: &'a CancelInput,
    ) -> AgentFuture<'a, CancelResult>;
}

/// Validate and render the same typed contract in native and foreign hosts.
pub trait AgentContract: Serialize + for<'de> Deserialize<'de> + JsonSchema {
    /// Check semantic limits, in addition to Serde's structural validation.
    /// # Errors
    /// Returns invalid-input or limit errors for unusable values.
    fn validate(&self) -> Result<(), Error>;
}

fn bounded(value: &str, name: &str, max: usize) -> Result<(), Error> {
    if value.trim().is_empty() {
        return Err(Error::InvalidInput(format!("{name} must not be empty")));
    }
    if value.len() > max {
        return Err(Error::LimitExceeded(format!("{name} exceeds {max} bytes")));
    }
    Ok(())
}

impl AgentScope {
    /// Validate trusted identifiers before contacting the backend.
    /// # Errors
    /// Rejects empty or oversized identities.
    pub fn validate(&self) -> Result<(), Error> {
        bounded(&self.room_id, "room_id", 256)?;
        bounded(&self.caller_id, "caller_id", 256)
    }
}

impl AgentContract for DelegateInput {
    fn validate(&self) -> Result<(), Error> {
        bounded(&self.profile, "profile", 128)?;
        bounded(&self.task, "task", 16_384)
    }
}
impl AgentContract for StatusInput {
    fn validate(&self) -> Result<(), Error> {
        self.task_id
            .as_ref()
            .map_or(Ok(()), |id| bounded(id, "task_id", 256))
    }
}
impl AgentContract for CancelInput {
    fn validate(&self) -> Result<(), Error> {
        bounded(&self.task_id, "task_id", 256)
    }
}
impl AgentContract for TaskInfo {
    fn validate(&self) -> Result<(), Error> {
        bounded(&self.task_id, "task_id", 256)?;
        bounded(&self.agent_id, "agent_id", 256)?;
        bounded(&self.profile, "profile", 128)?;
        if self.result.as_ref().is_some_and(|s| s.len() > 8192) {
            return Err(Error::LimitExceeded(
                "retained result exceeds 8192 bytes".into(),
            ));
        }
        if self.status == TaskState::Running && (self.result.is_some() || self.result_truncated) {
            return Err(Error::InvalidInput(
                "running task cannot have a final result".into(),
            ));
        }
        Ok(())
    }
}
impl AgentContract for TaskList {
    fn validate(&self) -> Result<(), Error> {
        if self.pending_launches > 64 || self.tasks.len() > 64 - self.pending_launches {
            return Err(Error::LimitExceeded("at most 64 retained tasks".into()));
        }
        let mut ids = std::collections::HashSet::new();
        for task in &self.tasks {
            task.validate()?;
            if !ids.insert(&task.task_id) {
                return Err(Error::InvalidInput("duplicate task_id".into()));
            }
        }
        Ok(())
    }
}
impl AgentContract for CancelResult {
    fn validate(&self) -> Result<(), Error> {
        self.task.validate()?;
        if self.cancelled && self.task.status != TaskState::Cancelled {
            return Err(Error::InvalidInput(
                "cancelled=true requires cancelled state".into(),
            ));
        }
        Ok(())
    }
}

/// Decode bounded JSON into the actual implementation type and validate it.
/// # Errors
/// Rejects malformed, oversized or semantically invalid input.
pub fn decode<T: AgentContract>(json: &str) -> Result<T, Error> {
    if json.len() > 1_048_576 {
        return Err(Error::LimitExceeded("agent JSON exceeds 1 MiB".into()));
    }
    let value: T = serde_json::from_str(json).map_err(|e| Error::InvalidInput(e.to_string()))?;
    value.validate()?;
    Ok(value)
}

/// Render normalized bounded data without a second, divergent text contract.
/// # Errors
/// Rejects malformed backend results or serialization failures.
pub fn render<T: AgentContract>(data: T) -> Result<Output<T>, Error> {
    data.validate()?;
    let text = serde_json::to_string(&data).map_err(|e| Error::InvalidInput(e.to_string()))?;
    Ok(Output::new(data, text))
}

macro_rules! tool {
    ($name:ident, $input:ty, $output:ty, $method:ident, $wire:literal, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Debug)]
        pub struct $name {
            backend: Arc<dyn AgentBackend>,
        }
        impl $name {
            /// Select this tool with an explicit shared backend. No I/O occurs.
            pub fn new(backend: Arc<dyn AgentBackend>) -> Self {
                Self { backend }
            }
            /// Derive schemas from the types used by calls and backends.
            pub fn definition() -> Definition {
                Definition::new::<$input, $output>($wire, $description)
            }
            /// Execute within a host-provided scope.
            /// # Errors
            /// Validation, access, limits and backend execution failures.
            pub async fn call(
                &self,
                scope: &AgentScope,
                input: &$input,
            ) -> Result<Output<$output>, Error> {
                scope.validate()?;
                input.validate()?;
                render(self.backend.$method(scope, input).await?)
            }
        }
    };
}

tool!(
    DelegateTaskTool,
    DelegateInput,
    TaskInfo,
    delegate,
    "delegate_task",
    "Start a fresh background agent from an allowed profile. Provide the complete task. The task ID confirms execution; results return to the principal. Never claim a start without calling this tool."
);
tool!(
    TaskStatusTool,
    StatusInput,
    TaskList,
    status,
    "task_status",
    "Read observed task states and retained results. Omit `task_id` to list this backend's tasks. Worker completion and result delivery are separate states."
);
tool!(
    CancelTaskTool,
    CancelInput,
    CancelResult,
    cancel,
    "cancel_task",
    "Cancel an owned background task. Already ended tasks are unchanged. Inspect cancelled and the returned state; do not infer success."
);
