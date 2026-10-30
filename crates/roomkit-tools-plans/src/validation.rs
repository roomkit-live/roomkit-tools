use crate::PlanContract;
use crate::types::{
    Plan, PlanCreateInput, PlanDeleteInput, PlanDeleteResult, PlanListInput, PlanListResult,
    PlanReadInput, PlanScope, PlanStep, PlanUpdateInput,
};
use roomkit_tools_core::Error;
use std::collections::BTreeSet;

pub(crate) fn bounded(value: &str, name: &str, max: usize) -> Result<(), Error> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err(Error::InvalidInput(format!(
            "{name} must be non-empty and contain no NUL"
        )));
    }
    if value.len() > max {
        return Err(Error::LimitExceeded(format!(
            "{name} exceeds {max} UTF-8 bytes"
        )));
    }
    Ok(())
}
fn revision(value: u64) -> Result<(), Error> {
    if value == 0 || value > 9_007_199_254_740_991 {
        return Err(Error::InvalidInput(
            "revision must be a positive JSON-safe integer".into(),
        ));
    }
    Ok(())
}
fn steps(values: &[PlanStep]) -> Result<(), Error> {
    if values.len() > 64 {
        return Err(Error::LimitExceeded(
            "a plan supports at most 64 steps".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    for step in values {
        bounded(&step.step_id, "step_id", 64)?;
        if !step
            .step_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(Error::InvalidInput(
                "step_id accepts ASCII letters, digits, '_' and '-'".into(),
            ));
        }
        if !ids.insert(&step.step_id) {
            return Err(Error::InvalidInput("duplicate step_id in plan".into()));
        }
        bounded(&step.description, "step description", 2048)?;
    }
    Ok(())
}
pub(crate) fn size<T: serde::Serialize>(value: &T) -> Result<(), Error> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| Error::InvalidInput("invalid plan JSON".into()))?;
    if bytes.len() > 64 * 1024 {
        return Err(Error::LimitExceeded(
            "plan record exceeds 64 KiB JSON".into(),
        ));
    }
    Ok(())
}
impl PlanScope {
    /// Validate the trusted host identity.
    /// # Errors
    /// Rejects empty or oversized scope identifiers.
    pub fn validate(&self) -> Result<(), Error> {
        bounded(&self.room_id, "room_id", 256)?;
        bounded(&self.caller_id, "caller_id", 256)
    }
}
impl PlanContract for PlanCreateInput {
    fn validate(&self) -> Result<(), Error> {
        bounded(&self.title, "title", 256)?;
        bounded(&self.objective, "objective", 16 * 1024)?;
        steps(&self.steps)?;
        size(self)
    }
}
impl PlanContract for PlanReadInput {
    fn validate(&self) -> Result<(), Error> {
        bounded(&self.plan_id, "plan_id", 128)
    }
}
impl PlanContract for PlanListInput {
    fn validate(&self) -> Result<(), Error> {
        Ok(())
    }
}
impl PlanContract for PlanUpdateInput {
    fn validate(&self) -> Result<(), Error> {
        bounded(&self.plan_id, "plan_id", 128)?;
        revision(self.expected_revision)?;
        if self.title.is_none() && self.objective.is_none() && self.steps.is_none() {
            return Err(Error::InvalidInput(
                "provide title, objective or the complete steps list".into(),
            ));
        }
        if let Some(value) = &self.title {
            bounded(value, "title", 256)?;
        }
        if let Some(value) = &self.objective {
            bounded(value, "objective", 16 * 1024)?;
        }
        if let Some(value) = &self.steps {
            steps(value)?;
        }
        size(self)
    }
}
impl PlanContract for PlanDeleteInput {
    fn validate(&self) -> Result<(), Error> {
        bounded(&self.plan_id, "plan_id", 128)?;
        revision(self.expected_revision)
    }
}
impl PlanContract for Plan {
    fn validate(&self) -> Result<(), Error> {
        bounded(&self.plan_id, "plan_id", 128)?;
        revision(self.revision)?;
        bounded(&self.title, "title", 256)?;
        bounded(&self.objective, "objective", 16 * 1024)?;
        steps(&self.steps)?;
        size(self)
    }
}
impl PlanContract for PlanListResult {
    fn validate(&self) -> Result<(), Error> {
        if self.plans.len() > 64 {
            return Err(Error::LimitExceeded(
                "scope supports at most 64 plans".into(),
            ));
        }
        let mut ids = BTreeSet::new();
        for plan in &self.plans {
            bounded(&plan.plan_id, "plan_id", 128)?;
            bounded(&plan.title, "title", 256)?;
            revision(plan.revision)?;
            if !ids.insert(&plan.plan_id)
                || plan.total_steps > 64
                || plan.completed_steps > plan.total_steps
            {
                return Err(Error::InvalidInput("invalid plan summary".into()));
            }
        }
        // Count and field limits bound the full list below 256 KiB, even with JSON escaping.
        Ok(())
    }
}
impl PlanContract for PlanDeleteResult {
    fn validate(&self) -> Result<(), Error> {
        bounded(&self.plan_id, "plan_id", 128)?;
        revision(self.revision)?;
        if !self.deleted {
            return Err(Error::InvalidInput(
                "deletion receipt must confirm deletion".into(),
            ));
        }
        Ok(())
    }
}
