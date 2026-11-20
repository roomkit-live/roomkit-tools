//! What a model reads of each plan result: lines, not the JSON of `data`.

use std::fmt::Write as _;

use crate::types::{Plan, PlanDeleteResult, PlanListResult, PlanStep, StepChange, StepStatus};

/// A status as the schema spells it.
fn status(value: StepStatus) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// One step, its description quoted so that no line it holds reads as a
/// step of its own.
fn step_line(text: &mut String, number: usize, step: &PlanStep) {
    let _written = writeln!(
        text,
        "{number}. {} [{}]: {:?}",
        step.step_id,
        status(step.status),
        step.description
    );
}

/// A plan: its heading, objective and every step in order.
pub(crate) fn plan(plan: &Plan) -> String {
    let mut text = format!(
        "{} (revision {}): {:?}\nObjective: {:?}\n",
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

/// One step without its description, which the model already has.
fn status_line(text: &mut String, number: usize, step: &PlanStep) {
    let _written = writeln!(text, "{number}. {} [{}]", step.step_id, status(step.status));
}

/// A creation: the plan's ID and revision, and its steps without the title,
/// objective and descriptions the model just wrote.
pub(crate) fn created(plan: &Plan) -> String {
    let mut text = format!("{} (revision {}) created\n", plan.plan_id, plan.revision);
    if plan.steps.is_empty() {
        text.push_str("no steps\n");
    }
    for (index, step) in plan.steps.iter().enumerate() {
        status_line(&mut text, index + 1, step);
    }
    text
}

/// An update by step: the new revision and only the steps it changed, at
/// their place in the plan; a step whose status alone changed, without its
/// description.
pub(crate) fn changed(plan: &Plan, changes: &[StepChange]) -> String {
    let count = changes.len();
    let mut text = format!(
        "{} (revision {}): {count} step{} changed\n",
        plan.plan_id,
        plan.revision,
        if count == 1 { "" } else { "s" }
    );
    for (index, step) in plan.steps.iter().enumerate() {
        match changes.iter().find(|change| change.step_id == step.step_id) {
            Some(change) if change.description.is_some() => {
                step_line(&mut text, index + 1, step);
            }
            Some(_) => status_line(&mut text, index + 1, step),
            None => {}
        }
    }
    text
}

/// An update by step that changed nothing: the revision stays.
pub(crate) fn unchanged(plan: &Plan) -> String {
    format!("{} (revision {}): no change\n", plan.plan_id, plan.revision)
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
            "{} (revision {}): {:?}, {}/{} steps completed",
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
