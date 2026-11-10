//! What a model reads of each plan result: lines, not the JSON of `data`.

use std::fmt::Write as _;

use crate::types::{Plan, PlanDeleteResult, PlanListResult, PlanStep, StepStatus};

fn status(value: StepStatus) -> &'static str {
    match value {
        StepStatus::Pending => "pending",
        StepStatus::InProgress => "in_progress",
        StepStatus::Blocked => "blocked",
        StepStatus::Completed => "completed",
        StepStatus::Cancelled => "cancelled",
    }
}

fn step_line(text: &mut String, number: usize, step: &PlanStep) {
    let _written = writeln!(
        text,
        "{number}. {} [{}]: {}",
        step.step_id,
        status(step.status),
        step.description
    );
}

/// A plan: its heading, objective and every step in order.
pub(crate) fn plan(plan: &Plan) -> String {
    let mut text = format!(
        "{} (revision {}): {}\nObjective: {}\n",
        plan.plan_id, plan.revision, plan.title, plan.objective
    );
    if plan.steps.is_empty() {
        text.push_str("no steps\n");
    }
    for (index, step) in plan.steps.iter().enumerate() {
        step_line(&mut text, index + 1, step);
    }
    text
}

/// An update by step: the new revision and only the steps it changed, at
/// their place in the plan.
pub(crate) fn changed(plan: &Plan, changed: &[String]) -> String {
    let count = changed.len();
    let mut text = format!(
        "{} (revision {}): {count} step{} changed\n",
        plan.plan_id,
        plan.revision,
        if count == 1 { "" } else { "s" }
    );
    for (index, step) in plan.steps.iter().enumerate() {
        if changed.contains(&step.step_id) {
            step_line(&mut text, index + 1, step);
        }
    }
    text
}

/// Every plan of the scope, one line each.
pub(crate) fn list(result: &PlanListResult) -> String {
    let mut text = String::new();
    if result.plans.is_empty() {
        text.push_str("no plans\n");
    }
    for plan in &result.plans {
        let _written = writeln!(
            text,
            "{} (revision {}): {}, {}/{} steps completed",
            plan.plan_id, plan.revision, plan.title, plan.completed_steps, plan.total_steps
        );
    }
    text
}

/// The deletion receipt.
pub(crate) fn deleted(result: &PlanDeleteResult) -> String {
    format!(
        "{} (revision {}) deleted\n",
        result.plan_id, result.revision
    )
}
