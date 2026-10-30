use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Trusted owner supplied by the host, outside model arguments.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PlanScope {
    /// Conversation containing the plan.
    pub room_id: String,
    /// Agent allowed to manage it.
    pub caller_id: String,
}

/// Declared progress, not evidence that work or validation actually ran.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    /// Work has not started.
    Pending,
    /// Work is underway. Parallel active steps are allowed.
    InProgress,
    /// Work is blocked.
    Blocked,
    /// The agent declares this step complete.
    Completed,
    /// The step has been abandoned.
    Cancelled,
}

/// A stable, caller-named step. Reuse its ID when editing or reordering it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanStep {
    /// Unique within this plan; 1..64 ASCII letters, digits, underscores or hyphens.
    pub step_id: String,
    /// What this step should accomplish, at most 2048 UTF-8 bytes.
    pub description: String,
    /// Declared progress; changing it never starts or stops work.
    pub status: StepStatus,
}

/// Create a new plan in the trusted scope. Recover a lost receipt with `plan_list`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanCreateInput {
    /// Non-empty title, at most 256 UTF-8 bytes.
    pub title: String,
    /// Desired outcome, at most 16 KiB UTF-8.
    pub objective: String,
    /// Complete ordered list of up to 64 steps, each with a unique stable ID.
    pub steps: Vec<PlanStep>,
}

/// Read a plan owned by this conversation and agent.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanReadInput {
    /// Copy the identifier returned by create or list.
    pub plan_id: String,
}

/// List all retained summaries in this scope (at most 64); no hidden eviction.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanListInput {}

/// Modify an existing plan atomically using its current revision.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanUpdateInput {
    /// Copy the identifier returned by create or list.
    pub plan_id: String,
    /// Current revision from read/create/update/list; stale values are refused.
    pub expected_revision: u64,
    /// Replace the title; omit/null to preserve it.
    pub title: Option<String>,
    /// Replace the objective; omit/null to preserve it.
    pub objective: Option<String>,
    /// Replace the ENTIRE ordered step list; omit/null to preserve it. Include
    /// unchanged steps too, reuse their `step_id`, and omit steps to remove them.
    pub steps: Option<Vec<PlanStep>>,
}

/// Delete one plan. Does not cancel any process or delegated agent task.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanDeleteInput {
    /// Owned plan to delete.
    pub plan_id: String,
    /// Current revision; stale deletes are refused without removing anything.
    pub expected_revision: u64,
}

/// Current plan snapshot, limited to 64 KiB serialized JSON.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    /// Backend-issued identifier, scoped to the backend instance and owner.
    pub plan_id: String,
    /// Positive version incremented on actual changes; no-op updates preserve it.
    pub revision: u64,
    /// Plan title.
    pub title: String,
    /// Desired outcome.
    pub objective: String,
    /// Complete ordered steps.
    pub steps: Vec<PlanStep>,
}

/// Bounded discovery record; use `plan_read` to obtain the full plan.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanSummary {
    /// Backend-issued identifier.
    pub plan_id: String,
    /// Current version, usable in update/delete.
    pub revision: u64,
    /// Title.
    pub title: String,
    /// Number of steps.
    pub total_steps: usize,
    /// Number declared completed.
    pub completed_steps: usize,
}

/// All summaries retained in the trusted scope.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanListResult {
    /// Complete list, at most 64 entries; no pagination or silent truncation.
    pub plans: Vec<PlanSummary>,
}

/// Receipt of actual deletion, not an instruction to cancel related work.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanDeleteResult {
    /// Removed identifier.
    pub plan_id: String,
    /// Revision of the removed snapshot.
    pub revision: u64,
    /// True only after deletion has committed.
    pub deleted: bool,
}
