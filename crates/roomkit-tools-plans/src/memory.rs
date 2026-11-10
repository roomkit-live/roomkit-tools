use crate::{
    PlanBackend, PlanContract, PlanFuture, cancelled,
    types::{
        Plan, PlanCreateInput, PlanDeleteInput, PlanDeleteResult, PlanListInput, PlanListResult,
        PlanReadInput, PlanScope, PlanSummary, PlanUpdateInput, StepStatus,
    },
};
use roomkit_tools_core::ToolError;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard},
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Default)]
struct State {
    sequence: u64,
    plans: BTreeMap<String, (PlanScope, Plan)>,
}

/// Capacity bounds of a [`MemoryPlanBackend`], Python's keywords of
/// `MemoryPlanBackend(*, max_plans=256, max_plans_per_scope=64)`. Each plan
/// record is at most 64 KiB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryPlanLimits {
    /// Plans retained across every scope, 1 to 4096. Default: 256.
    pub max_plans: usize,
    /// Plans retained in one scope, 1 to 64 and at most `max_plans`. Default: 64.
    pub max_plans_per_scope: usize,
}
impl Default for MemoryPlanLimits {
    fn default() -> Self {
        Self {
            max_plans: 256,
            max_plans_per_scope: 64,
        }
    }
}

/// In-memory storage. Clones share state; separate instances do not. No disk I/O,
/// scheduler, background worker or automatic eviction. Drop all clones to release.
/// Listings return plans in creation order.
/// Operations take a short mutex and check cancellation before committing.
#[derive(Clone, Debug)]
pub struct MemoryPlanBackend {
    state: Arc<Mutex<State>>,
    max_plans: usize,
    max_plans_per_scope: usize,
}
impl Default for MemoryPlanBackend {
    fn default() -> Self {
        let limits = MemoryPlanLimits::default();
        Self {
            state: Arc::default(),
            max_plans: limits.max_plans,
            max_plans_per_scope: limits.max_plans_per_scope,
        }
    }
}
impl MemoryPlanBackend {
    /// Configure independent global/per-owner record counts.
    /// # Errors
    /// Rejects zero bounds, more than 4096 total or more than 64 plans per scope.
    pub fn new(limits: MemoryPlanLimits) -> Result<Self, ToolError> {
        let MemoryPlanLimits {
            max_plans,
            max_plans_per_scope,
        } = limits;
        if !(1..=4096).contains(&max_plans)
            || !(1..=64).contains(&max_plans_per_scope)
            || max_plans_per_scope > max_plans
        {
            return Err(ToolError::InvalidInput(
                "invalid plan capacity limits".into(),
            ));
        }
        Ok(Self {
            max_plans,
            max_plans_per_scope,
            ..Default::default()
        })
    }
    fn lock(&self) -> Result<MutexGuard<'_, State>, ToolError> {
        self.state.lock().map_err(|_| {
            ToolError::UnsupportedResource("plan store unavailable after a worker failure".into())
        })
    }
}
fn owned<'a>(state: &'a State, scope: &PlanScope, id: &str) -> Result<&'a Plan, ToolError> {
    state
        .plans
        .get(id)
        .filter(|(owner, _)| owner == scope)
        .map(|(_, plan)| plan)
        .ok_or_else(|| ToolError::AccessRefused("unknown plan in this scope".into()))
}
fn check_revision(plan: &Plan, expected: u64) -> Result<(), ToolError> {
    if plan.revision == expected {
        Ok(())
    } else {
        Err(ToolError::AccessRefused(
            "plan revision conflict; use plan_read before retrying".into(),
        ))
    }
}
impl PlanBackend for MemoryPlanBackend {
    fn create<'a>(
        &'a self,
        scope: &'a PlanScope,
        input: &'a PlanCreateInput,
        cancel: &'a CancellationToken,
    ) -> PlanFuture<'a, Plan> {
        Box::pin(async move {
            scope.validate()?;
            input.validate()?;
            cancelled(cancel)?;
            let mut state = self.lock()?;
            if state.plans.len() >= self.max_plans
                || state
                    .plans
                    .values()
                    .filter(|(owner, _)| owner == scope)
                    .count()
                    >= self.max_plans_per_scope
            {
                return Err(ToolError::LimitExceeded(
                    "plan capacity reached; delete an owned plan before creating another".into(),
                ));
            }
            let sequence = state
                .sequence
                .checked_add(1)
                .ok_or_else(|| ToolError::LimitExceeded("plan identifiers exhausted".into()))?;
            let plan = Plan {
                plan_id: format!("plan-{sequence}"),
                revision: 1,
                title: input.title.clone(),
                objective: input.objective.clone(),
                steps: input.steps.clone(),
            };
            plan.validate()?;
            cancelled(cancel)?;
            state.sequence = sequence;
            state
                .plans
                .insert(plan.plan_id.clone(), (scope.clone(), plan.clone()));
            Ok(plan)
        })
    }
    fn read<'a>(
        &'a self,
        scope: &'a PlanScope,
        input: &'a PlanReadInput,
        cancel: &'a CancellationToken,
    ) -> PlanFuture<'a, Plan> {
        Box::pin(async move {
            scope.validate()?;
            input.validate()?;
            cancelled(cancel)?;
            let state = self.lock()?;
            cancelled(cancel)?;
            Ok(owned(&state, scope, &input.plan_id)?.clone())
        })
    }
    fn list<'a>(
        &'a self,
        scope: &'a PlanScope,
        input: &'a PlanListInput,
        cancel: &'a CancellationToken,
    ) -> PlanFuture<'a, PlanListResult> {
        Box::pin(async move {
            scope.validate()?;
            input.validate()?;
            cancelled(cancel)?;
            let state = self.lock()?;
            cancelled(cancel)?;
            let mut scoped: Vec<_> = state
                .plans
                .iter()
                .filter(|(_, (owner, _))| owner == scope)
                .collect();
            scoped.sort_by_key(|&(id, _)| roomkit_tools_core::identifiers::creation_order(id));
            let plans = scoped
                .into_iter()
                .map(|(_, (_, p))| PlanSummary {
                    plan_id: p.plan_id.clone(),
                    revision: p.revision,
                    title: p.title.clone(),
                    total_steps: p.steps.len(),
                    completed_steps: p
                        .steps
                        .iter()
                        .filter(|s| s.status == StepStatus::Completed)
                        .count(),
                })
                .collect();
            Ok(PlanListResult { plans })
        })
    }
    fn update<'a>(
        &'a self,
        scope: &'a PlanScope,
        input: &'a PlanUpdateInput,
        cancel: &'a CancellationToken,
    ) -> PlanFuture<'a, Plan> {
        Box::pin(async move {
            scope.validate()?;
            input.validate()?;
            cancelled(cancel)?;
            let mut state = self.lock()?;
            let current = owned(&state, scope, &input.plan_id)?;
            check_revision(current, input.expected_revision)?;
            let input = &crate::resolve_update(current, input.clone())?;
            let mut next = current.clone();
            if let Some(title) = &input.title {
                next.title.clone_from(title);
            }
            if let Some(objective) = &input.objective {
                next.objective.clone_from(objective);
            }
            if let Some(steps) = &input.steps {
                next.steps.clone_from(steps);
            }
            if next != *current {
                next.revision = current
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| ToolError::LimitExceeded("plan revision exhausted".into()))?;
            }
            next.validate()?;
            cancelled(cancel)?;
            state
                .plans
                .insert(next.plan_id.clone(), (scope.clone(), next.clone()));
            Ok(next)
        })
    }
    fn delete<'a>(
        &'a self,
        scope: &'a PlanScope,
        input: &'a PlanDeleteInput,
        cancel: &'a CancellationToken,
    ) -> PlanFuture<'a, PlanDeleteResult> {
        Box::pin(async move {
            scope.validate()?;
            input.validate()?;
            cancelled(cancel)?;
            let mut state = self.lock()?;
            let plan = owned(&state, scope, &input.plan_id)?;
            check_revision(plan, input.expected_revision)?;
            let result = PlanDeleteResult {
                plan_id: plan.plan_id.clone(),
                revision: plan.revision,
                deleted: true,
            };
            cancelled(cancel)?;
            state.plans.remove(&input.plan_id);
            Ok(result)
        })
    }
}
