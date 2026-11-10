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

/// Declared progress, not proof that work ran.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    /// Not started.
    Pending,
    /// Underway; several steps may be.
    InProgress,
    /// Blocked.
    Blocked,
    /// Declared done.
    Completed,
    /// Abandoned.
    Cancelled,
}

/// A step; keep its `step_id` across updates.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanStep {
    /// Unique in the plan: 1..64 ASCII letters, digits, `_` or `-`.
    pub step_id: String,
    /// What the step does, at most 2048 bytes.
    pub description: String,
    /// Declared progress.
    pub status: StepStatus,
}

/// A plan to create.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanCreateInput {
    /// Nonempty, at most 256 bytes.
    pub title: String,
    /// Desired outcome, at most 16 KiB.
    pub objective: String,
    /// Ordered steps, at most 64.
    pub steps: Vec<PlanStep>,
}

/// A plan to read.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanReadInput {
    /// From `plan_create` or `plan_list`.
    pub plan_id: String,
}

/// No arguments.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanListInput {}

/// Changes to a plan, applied atomically.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanUpdateInput {
    /// From `plan_create` or `plan_list`.
    pub plan_id: String,
    /// Current revision; a stale one is refused.
    pub expected_revision: u64,
    /// New title.
    pub title: Option<String>,
    /// New objective.
    pub objective: Option<String>,
    /// The ENTIRE new ordered step list.
    pub steps: Option<Vec<PlanStep>>,
    /// Changes to existing steps by `step_id`, instead of `steps`: the
    /// other steps and the order are kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_changes: Option<Vec<StepChange>>,
}

/// A change to one existing step; give `status`, `description` or both.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepChange {
    /// The step to change.
    pub step_id: String,
    /// New declared progress.
    #[serde(default)]
    pub status: Option<StepStatus>,
    /// New description, at most 2048 bytes.
    #[serde(default)]
    pub description: Option<String>,
}

impl PlanUpdateInput {
    /// Change nothing yet in `plan_id` at `expected_revision`, as Python's
    /// `update(scope, plan_id=..., expected_revision=...)`: set `title`,
    /// `objective`, `steps` or `step_changes` by struct update; `None`
    /// preserves each.
    pub fn new(plan_id: impl Into<String>, expected_revision: u64) -> Self {
        Self {
            plan_id: plan_id.into(),
            expected_revision,
            title: None,
            objective: None,
            steps: None,
            step_changes: None,
        }
    }
}

/// A plan to delete.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanDeleteInput {
    /// Owned plan to delete.
    pub plan_id: String,
    /// Current revision; a stale one is refused.
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
