//! Stateless delegation facades over a host-owned backend.
//!
//! This crate neither runs models nor schedules tasks. Backends own profiles,
//! execution, scope checks, cancellation and cleanup. No runtime is required.
//! Each call takes a [`CancellationToken`], as every tool's does, and hands it
//! to the backend. [`AskUserTool`] asks the person questions through the same
//! backend.

#![forbid(unsafe_code)]

use std::{fmt::Debug, future::Future, pin::Pin, sync::Arc};

use roomkit_tools_core::{Definition, Output, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

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
pub struct DelegateTaskInput {
    /// Allowed profile name, not an instance or provider name.
    pub profile: String,
    /// Complete instructions for a fresh, isolated worker.
    pub task: String,
}

/// Inspect one owned task or all retained tasks in this scope.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskStatusInput {
    /// Omit to list retained tasks.
    pub task_id: Option<String>,
}

/// Request cancellation of one owned task.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelTaskInput {
    /// Identifier returned by `delegate_task`.
    pub task_id: String,
}

/// A new mission in an existing agent conversation.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentFollowupInput {
    /// Copy exactly from delegation or `task_status`, not a profile or task ID.
    pub agent_id: String,
    /// New instructions, at most 16 KiB. The previous conversation is retained.
    pub task: String,
}

/// A clarification for the agent's currently running mission.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentSendInput {
    /// Copy exactly from delegation or `task_status`, not a profile or task ID.
    pub agent_id: String,
    /// Plain message, at most 8 KiB. Does not change tools or permissions.
    pub message: String,
}

/// Observed admission to a running loop; not proof of model consumption.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SendState {
    /// Queued for the next steering boundary of the active loop.
    Queued,
    /// No active loop accepted the message; no new task was started.
    NotRunning,
}

/// Receipt for a message to an existing running agent.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SendResult {
    /// Addressed agent instance.
    pub agent_id: String,
    /// Mission targeted by this attempt, if any.
    pub task_id: Option<String>,
    /// Accepted message identifier, or None when no loop accepted it.
    pub message_id: Option<String>,
    /// Observed queuing outcome, not an acknowledgement from the model.
    pub status: SendState,
    /// Whether the message was also saved in the child conversation.
    pub history_recorded: bool,
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
    /// Whether the agent session is retained. A follow-up also requires it to be idle.
    #[serde(default)]
    pub agent_available: bool,
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
pub type AgentFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ToolError>> + Send + 'a>>;

/// Host extension point. Implementations must check scope on every operation,
/// bound their registries, and own cleanup independently of caller cancellation.
/// `cancel` is the caller's token: a backend may stop waiting when it fires, but
/// a task it already launched keeps running until `cancel_task` cancels it.
/// Dropping a delegate future can lose its receipt: recover with status before
/// retrying. A backend must document its own shutdown and persistence contract.
pub trait AgentBackend: Debug + Send + Sync {
    /// Resolve a permitted profile and launch a fresh instance.
    fn delegate<'a>(
        &'a self,
        scope: &'a AgentScope,
        input: &'a DelegateTaskInput,
        cancel: &'a CancellationToken,
    ) -> AgentFuture<'a, TaskInfo>;
    /// Read one or all retained task records in this scope.
    fn status<'a>(
        &'a self,
        scope: &'a AgentScope,
        input: &'a TaskStatusInput,
        cancel: &'a CancellationToken,
    ) -> AgentFuture<'a, TaskList>;
    /// Cancel an owned task, preserving cleanup if the caller goes away.
    fn cancel<'a>(
        &'a self,
        scope: &'a AgentScope,
        input: &'a CancelTaskInput,
        cancel: &'a CancellationToken,
    ) -> AgentFuture<'a, CancelResult>;
    /// Start a new mission in a retained, idle agent conversation.
    fn followup<'a>(
        &'a self,
        _scope: &'a AgentScope,
        _input: &'a AgentFollowupInput,
        _cancel: &'a CancellationToken,
    ) -> AgentFuture<'a, TaskInfo> {
        Box::pin(async {
            Err(ToolError::UnsupportedResource(
                "agent continuation is not supported by this backend".into(),
            ))
        })
    }
    /// Queue plain text for an active loop. Never silently start another mission.
    fn send<'a>(
        &'a self,
        _scope: &'a AgentScope,
        _input: &'a AgentSendInput,
        _cancel: &'a CancellationToken,
    ) -> AgentFuture<'a, SendResult> {
        Box::pin(async {
            Err(ToolError::UnsupportedResource(
                "agent messages are not supported by this backend".into(),
            ))
        })
    }
    /// Show the person the questions and wait for their answers. Return the
    /// answers the person gave, one per question; refuse with
    /// `AccessRefused` and the reason when nobody can answer.
    fn ask<'a>(
        &'a self,
        _scope: &'a AgentScope,
        _input: &'a AskUserInput,
        _cancel: &'a CancellationToken,
    ) -> AgentFuture<'a, AskUserResult> {
        Box::pin(async {
            Err(ToolError::UnsupportedResource(
                "asking the user is not supported by this backend".into(),
            ))
        })
    }
}

/// Validate and render the same typed contract in native and foreign hosts.
#[doc(hidden)]
pub trait AgentContract: Serialize + for<'de> Deserialize<'de> + JsonSchema {
    /// Check semantic limits, in addition to Serde's structural validation.
    /// # Errors
    /// Returns invalid-input or limit errors for unusable values.
    fn validate(&self) -> Result<(), ToolError>;
}

fn bounded(value: &str, name: &str, max: usize) -> Result<(), ToolError> {
    if value.trim().is_empty() {
        return Err(ToolError::InvalidInput(format!("{name} must not be empty")));
    }
    if value.len() > max {
        return Err(ToolError::LimitExceeded(format!(
            "{name} exceeds {max} bytes"
        )));
    }
    Ok(())
}

impl AgentScope {
    /// Validate trusted identifiers before contacting the backend.
    /// # Errors
    /// Rejects empty or oversized identities.
    #[doc(hidden)]
    pub fn validate(&self) -> Result<(), ToolError> {
        bounded(&self.room_id, "room_id", 256)?;
        bounded(&self.caller_id, "caller_id", 256)
    }
}

impl AgentContract for DelegateTaskInput {
    fn validate(&self) -> Result<(), ToolError> {
        bounded(&self.profile, "profile", 128)?;
        bounded(&self.task, "task", 16_384)
    }
}
impl AgentContract for TaskStatusInput {
    fn validate(&self) -> Result<(), ToolError> {
        self.task_id
            .as_ref()
            .map_or(Ok(()), |id| bounded(id, "task_id", 256))
    }
}
impl AgentContract for CancelTaskInput {
    fn validate(&self) -> Result<(), ToolError> {
        bounded(&self.task_id, "task_id", 256)
    }
}
impl AgentContract for AgentFollowupInput {
    fn validate(&self) -> Result<(), ToolError> {
        bounded(&self.agent_id, "agent_id", 256)?;
        bounded(&self.task, "task", 16_384)
    }
}
impl AgentContract for AgentSendInput {
    fn validate(&self) -> Result<(), ToolError> {
        bounded(&self.agent_id, "agent_id", 256)?;
        bounded(&self.message, "message", 8192)
    }
}
impl AgentContract for SendResult {
    fn validate(&self) -> Result<(), ToolError> {
        bounded(&self.agent_id, "agent_id", 256)?;
        if let Some(id) = &self.task_id {
            bounded(id, "task_id", 256)?;
        }
        if let Some(id) = &self.message_id {
            bounded(id, "message_id", 256)?;
        }
        if (self.status == SendState::Queued
            && (self.task_id.is_none() || self.message_id.is_none()))
            || (self.status == SendState::NotRunning
                && (self.message_id.is_some() || self.history_recorded))
        {
            return Err(ToolError::InvalidInput(
                "inconsistent message receipt".into(),
            ));
        }
        Ok(())
    }
}
impl AgentContract for TaskInfo {
    fn validate(&self) -> Result<(), ToolError> {
        bounded(&self.task_id, "task_id", 256)?;
        bounded(&self.agent_id, "agent_id", 256)?;
        bounded(&self.profile, "profile", 128)?;
        if self.result.as_ref().is_some_and(|s| s.len() > 8192) {
            return Err(ToolError::LimitExceeded(
                "retained result exceeds 8192 bytes".into(),
            ));
        }
        if self.status == TaskState::Running && (self.result.is_some() || self.result_truncated) {
            return Err(ToolError::InvalidInput(
                "running task cannot have a final result".into(),
            ));
        }
        Ok(())
    }
}
impl AgentContract for TaskList {
    fn validate(&self) -> Result<(), ToolError> {
        if self.pending_launches > 64 || self.tasks.len() > 64 - self.pending_launches {
            return Err(ToolError::LimitExceeded("at most 64 retained tasks".into()));
        }
        let mut ids = std::collections::HashSet::new();
        for task in &self.tasks {
            task.validate()?;
            if !ids.insert(&task.task_id) {
                return Err(ToolError::InvalidInput("duplicate task_id".into()));
            }
        }
        Ok(())
    }
}
impl AgentContract for CancelResult {
    fn validate(&self) -> Result<(), ToolError> {
        self.task.validate()?;
        if self.cancelled && self.task.status != TaskState::Cancelled {
            return Err(ToolError::InvalidInput(
                "cancelled=true requires cancelled state".into(),
            ));
        }
        Ok(())
    }
}

/// Decode bounded JSON into the actual implementation type and validate it.
/// # Errors
/// Rejects malformed, oversized or semantically invalid input.
#[doc(hidden)]
pub fn decode<T: AgentContract>(json: &str) -> Result<T, ToolError> {
    if json.len() > 1_048_576 {
        return Err(ToolError::LimitExceeded("agent JSON exceeds 1 MiB".into()));
    }
    let value: T =
        serde_json::from_str(json).map_err(|e| ToolError::InvalidInput(clipped(e.to_string())))?;
    value.validate()?;
    Ok(value)
}

/// A refusal a model can read whole: serde quotes the offending value, which
/// may be as long as the call, between what went wrong and what was expected.
/// Both ends stay; the middle of the quote goes.
fn clipped(message: String) -> String {
    const KEPT_BYTES: usize = 256;
    if message.len() <= 2 * KEPT_BYTES {
        return message;
    }
    let mut head = KEPT_BYTES;
    while !message.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = message.len() - KEPT_BYTES;
    while !message.is_char_boundary(tail) {
        tail += 1;
    }
    let (start, end) = (message.get(..head), message.get(tail..));
    format!("{}…{}", start.unwrap_or_default(), end.unwrap_or_default())
}

/// Render normalized bounded data without a second, divergent text contract.
/// # Errors
/// Rejects malformed backend results or serialization failures.
#[doc(hidden)]
pub fn render<T: AgentContract>(data: T) -> Result<Output<T>, ToolError> {
    data.validate()?;
    let text = serde_json::to_string(&data).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
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
            /// Execute within a host-provided scope, as Python's
            /// `call(arguments, scope=...)`.
            /// # Errors
            /// Validation, cancellation, access, limits and backend execution failures.
            pub async fn call(
                &self,
                scope: &AgentScope,
                input: $input,
                cancel: &CancellationToken,
            ) -> Result<Output<$output>, ToolError> {
                scope.validate()?;
                input.validate()?;
                if cancel.is_cancelled() {
                    return Err(ToolError::Cancelled);
                }
                render(self.backend.$method(scope, &input, cancel).await?)
            }
        }
    };
}

tool!(
    DelegateTaskTool,
    DelegateTaskInput,
    TaskInfo,
    delegate,
    "delegate_task",
    "Start a background agent from an allowed profile with a complete task; it has no other context. Only a returned task ID proves the start. Results come back later."
);
tool!(
    TaskStatusTool,
    TaskStatusInput,
    TaskList,
    status,
    "task_status",
    "Show task states and retained results; omit `task_id` to list all. Worker completion and result delivery are separate states."
);
tool!(
    CancelTaskTool,
    CancelTaskInput,
    CancelResult,
    cancel,
    "cancel_task",
    "Cancel a background task; an ended task is unchanged. Check cancelled and the returned state."
);
tool!(
    AgentFollowupTool,
    AgentFollowupInput,
    TaskInfo,
    followup,
    "agent_followup",
    "Start a new mission in a retained idle agent's conversation; returns a NEW `task_id` for the SAME `agent_id`. Copy `agent_id` exactly. An unknown ID is not proof of expiry: check `task_status` and correct it before retrying. Busy or expired agents are refused."
);
tool!(
    AgentSendTool,
    AgentSendInput,
    SendResult,
    send,
    "agent_send",
    "Send a clarification to an agent's running mission. Copy `agent_id` exactly. queued means accepted for a later tool round, not yet read. `not_running` starts nothing: use `agent_followup` for an idle agent. Check `history_recorded` before assuming later missions keep the message."
);

// After the `tool!` macro, which it uses.
mod ask;
pub use ask::{
    AskUserAnswer, AskUserInput, AskUserOption, AskUserQuestion, AskUserResult, AskUserTool,
};

// The README's example is compiled and run as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
