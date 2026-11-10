//! Plan storage and cross-language contract checks.
use roomkit_tools_plans::{
    MemoryPlanBackend, PlanBackend, PlanCreateTool, PlanDeleteTool, PlanListTool, PlanReadTool,
    PlanScope, PlanUpdateTool, decode,
};
use serde_json::Value;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn shared_crud_scenario() {
    let backend = Arc::new(MemoryPlanBackend::default());
    let scope = PlanScope {
        room_id: "plans".into(),
        caller_id: "assistant".into(),
    };
    let cancel = CancellationToken::new();
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/plans.json")).unwrap();
    let mut outputs = Vec::new();
    for case in cases {
        let input = case["arguments"].to_string();
        macro_rules! run {
            ($tool:ident) => {
                serde_json::to_value(
                    $tool::new(backend.clone())
                        .call(&scope, decode(&input).unwrap(), &cancel)
                        .await
                        .unwrap(),
                )
                .unwrap()
            };
        }
        outputs.push(match case["tool"].as_str().unwrap() {
            "plan_create" => run!(PlanCreateTool),
            "plan_read" => run!(PlanReadTool),
            "plan_list" => run!(PlanListTool),
            "plan_update" => run!(PlanUpdateTool),
            "plan_delete" => run!(PlanDeleteTool),
            _ => panic!("unknown fixture tool"),
        });
    }
    assert_eq!(outputs[2]["data"]["revision"], 1);
    // The model reads the plan as lines, not the JSON of data.
    assert_eq!(
        outputs[2]["text"],
        "plan-1 (revision 1): Review shipping\nObjective: Validate the fee boundary\n\
         1. inspect [in_progress]: Read the source\n2. test [pending]: Test the boundary\n"
    );
    assert_eq!(outputs[3]["data"]["steps"][0]["step_id"], "test");
    assert_eq!(outputs[4]["data"]["revision"], 2); // No-op keeps the revision.
    assert_eq!(outputs[5]["data"]["plans"][0]["completed_steps"], 1);
    assert_eq!(outputs[6]["data"]["steps"].as_array().unwrap().len(), 1);
    // A change by step names only that step; data holds the whole plan.
    assert_eq!(outputs[7]["data"]["revision"], 4);
    assert_eq!(
        outputs[7]["text"],
        "plan-1 (revision 4): 1 step changed\n1. inspect [pending]: Reread the source\n"
    );
    assert_eq!(outputs[8]["data"]["deleted"], true);
    assert_eq!(outputs[8]["text"], "plan-1 (revision 4) deleted\n");
    assert_eq!(outputs[9]["data"]["plans"], serde_json::json!([]));
    assert_eq!(outputs[9]["text"], "no plans\n");
    if let Ok(path) = std::env::var("ROOMKIT_PLAN_RESULTS") {
        std::fs::write(path, serde_json::to_vec(&outputs).unwrap()).unwrap();
    }
}

/// Changing one status used to send the whole step list each way
/// (RMK-1639): `step_changes` changes only the steps it names.
#[tokio::test]
async fn step_changes_change_only_the_steps_they_name() {
    let backend: Arc<dyn PlanBackend> = Arc::new(MemoryPlanBackend::default());
    let scope = PlanScope {
        room_id: "plans".into(),
        caller_id: "assistant".into(),
    };
    let cancel = CancellationToken::new();
    let steps: Vec<Value> = (1..=18)
        .map(|i| serde_json::json!({"step_id": format!("s{i}"), "description": format!("Step {i}"), "status": "pending"}))
        .collect();
    let create = serde_json::json!({"title": "Release", "objective": "Ship", "steps": steps});
    PlanCreateTool::new(Arc::clone(&backend))
        .call(&scope, decode(&create.to_string()).unwrap(), &cancel)
        .await
        .unwrap();
    let update = PlanUpdateTool::new(backend);
    let call = |arguments: Value| {
        let update = update.clone();
        let scope = scope.clone();
        let cancel = cancel.clone();
        async move {
            update
                .call(&scope, decode(&arguments.to_string())?, &cancel)
                .await
        }
    };
    let changed = call(serde_json::json!({
        "plan_id": "plan-1",
        "expected_revision": 1,
        "step_changes": [{"step_id": "s3", "status": "completed"}]
    }))
    .await
    .unwrap();
    assert_eq!(changed.data.revision, 2);
    assert_eq!(changed.data.steps.len(), 18);
    assert_eq!(
        serde_json::to_value(&changed.data.steps[2]).unwrap(),
        serde_json::json!({"step_id": "s3", "description": "Step 3", "status": "completed"})
    );
    assert_eq!(
        serde_json::to_value(&changed.data.steps[3]).unwrap()["status"],
        "pending"
    );
    assert_eq!(
        changed.text,
        "plan-1 (revision 2): 1 step changed\n3. s3 [completed]: Step 3\n"
    );
    for (arguments, refusal) in [
        (
            serde_json::json!({"plan_id": "plan-1", "expected_revision": 2,
                "step_changes": [{"step_id": "s99", "status": "completed"}]}),
            "no step s99 in plan plan-1; plan_read lists its steps",
        ),
        (
            serde_json::json!({"plan_id": "plan-1", "expected_revision": 2,
                "steps": [], "step_changes": [{"step_id": "s1", "status": "completed"}]}),
            "give steps or step_changes, not both",
        ),
        (
            serde_json::json!({"plan_id": "plan-1", "expected_revision": 2,
                "step_changes": [{"step_id": "s1"}]}),
            "a step change gives status, description or both",
        ),
    ] {
        let result = call(arguments).await;
        assert!(
            matches!(&result, Err(roomkit_tools_core::ToolError::InvalidInput(message)) if message == refusal),
            "{result:?}"
        );
    }
}
