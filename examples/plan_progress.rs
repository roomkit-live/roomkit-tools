//! Independently selected plan tools share an explicit host-owned memory store.
#![forbid(unsafe_code)]
#![expect(clippy::print_stdout, reason = "standalone demonstration output")]
use roomkit_tools_plans::{
    MemoryPlanBackend, PlanBackend, PlanCreateInput, PlanCreateTool, PlanDeleteInput,
    PlanDeleteTool, PlanListInput, PlanListTool, PlanReadInput, PlanReadTool, PlanScope, PlanStep,
    PlanUpdateInput, PlanUpdateTool, StepStatus,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store: Arc<dyn PlanBackend> = Arc::new(MemoryPlanBackend::default());
    let scope = PlanScope {
        room_id: "review".into(),
        caller_id: "assistant".into(),
    };
    let cancel = CancellationToken::new();
    let mut plan = PlanCreateTool::new(Arc::clone(&store))
        .execute(
            &scope,
            PlanCreateInput {
                title: "Review shipping".into(),
                objective: "Check boundary behavior".into(),
                steps: vec![PlanStep {
                    step_id: "inspect".into(),
                    description: "Inspect the source".into(),
                    status: StepStatus::Pending,
                }],
            },
            &cancel,
        )
        .await?
        .data;
    let read = PlanReadTool::new(Arc::clone(&store))
        .execute(
            &scope,
            PlanReadInput {
                plan_id: plan.plan_id.clone(),
            },
            &cancel,
        )
        .await?;
    assert_eq!(read.data, plan);
    // This example records a declaration only; no source inspection is executed.
    if let Some(step) = plan.steps.first_mut() {
        step.status = StepStatus::InProgress;
    }
    let edited = PlanUpdateTool::new(Arc::clone(&store))
        .execute(
            &scope,
            PlanUpdateInput {
                steps: Some(plan.steps),
                ..PlanUpdateInput::new(plan.plan_id.clone(), plan.revision)
            },
            &cancel,
        )
        .await?
        .data;
    assert_eq!(edited.revision, 2);
    assert_eq!(
        PlanListTool::new(Arc::clone(&store))
            .execute(&scope, PlanListInput {}, &cancel)
            .await?
            .data
            .plans
            .len(),
        1
    );
    PlanDeleteTool::new(Arc::clone(&store))
        .execute(
            &scope,
            PlanDeleteInput {
                plan_id: edited.plan_id,
                expected_revision: edited.revision,
            },
            &cancel,
        )
        .await?;
    assert!(
        PlanListTool::new(store)
            .execute(&scope, PlanListInput {}, &cancel)
            .await?
            .data
            .plans
            .is_empty()
    );
    println!("Verified: scoped plan CRUD, revision update and deletion.");
    Ok(())
}
