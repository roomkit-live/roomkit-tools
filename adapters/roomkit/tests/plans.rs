//! Independently selected plan adapters derive ownership from RoomKit context.
#![cfg(feature = "plans")]

mod call;

use roomkit::model::Metadata;
use roomkit_ai::{Tool, ToolError, ToolResult};
use roomkit_tools_plans::{
    MemoryPlanBackend, PlanCreateTool, PlanDeleteTool, PlanListTool, PlanReadTool, PlanUpdateTool,
};
use roomkit_tools_roomkit::{PlanCreate, PlanDelete, PlanList, PlanRead, PlanUpdate};
use serde_json::{Value, json};
use std::sync::Arc;

#[expect(clippy::panic, reason = "unexpected test output must fail the test")]
fn data(output: Result<ToolResult, ToolError>) -> Value {
    let Ok(ToolResult::Json(value)) = output else {
        panic!("expected JSON, got {output:?}");
    };
    value["data"].clone()
}
#[tokio::test]
async fn scoped_crud_adapters_share_the_host_store_by_room_and_agent() {
    let store = Arc::new(MemoryPlanBackend::default());
    let creator = PlanCreate::new(PlanCreateTool::new(store.clone()));
    let reader = PlanRead::new(PlanReadTool::new(store.clone()));
    let updater = PlanUpdate::new(PlanUpdateTool::new(store.clone()));
    let lister = PlanList::new(PlanListTool::new(store.clone()));
    let deleter = PlanDelete::new(PlanDeleteTool::new(store));
    assert_eq!(
        creator.definition().parameters,
        PlanCreateTool::definition().input_schema
    );
    let (room, agent) = ("plans", "assistant");
    let request = json!({"title":"Inspect","objective":"Check boundary","steps":[]});
    let plan = data(call::run(&creator, room, agent, "plan_create", request).await);
    let request = json!({"plan_id":plan["plan_id"]});
    assert_eq!(
        data(call::run(&reader, room, agent, "plan_read", request.clone()).await),
        plan
    );
    for (other_room, other_agent) in [("other", "assistant"), ("plans", "other")] {
        assert!(matches!(
            call::run(
                &reader,
                other_room,
                other_agent,
                "plan_read",
                request.clone()
            )
            .await,
            Err(ToolError::Refused(_))
        ));
        assert_eq!(
            data(call::run(&lister, other_room, other_agent, "plan_list", json!({})).await)["plans"],
            json!([])
        );
    }
    let update = json!({"plan_id":plan["plan_id"],"expected_revision":1,"title":"Changed"});
    assert_eq!(
        data(call::run(&updater, room, agent, "plan_update", update.clone()).await)["revision"],
        2
    );
    assert!(matches!(
        call::run(&updater, room, agent, "plan_update", update).await,
        Err(ToolError::Refused(_))
    ));
    let deletion = json!({"plan_id":plan["plan_id"],"expected_revision":2});
    assert_eq!(
        data(call::run(&deleter, room, agent, "plan_delete", deletion).await)["deleted"],
        true
    );
    assert_eq!(
        data(call::run(&lister, room, agent, "plan_list", json!({})).await)["plans"],
        json!([])
    );
}

#[tokio::test]
async fn plan_adapters_refuse_calls_outside_a_roomkit_tool_call() {
    let store = Arc::new(MemoryPlanBackend::default());
    let creator = PlanCreate::new(PlanCreateTool::new(store));
    let arguments: Metadata =
        serde_json::from_value(json!({"title":"Inspect","objective":"Check boundary","steps":[]}))
            .unwrap();
    let refused = Err(ToolError::refused(
        "plan tools require a conversation and an acting agent",
    ));
    assert_eq!(
        creator.handler("plan_create".into(), arguments).await,
        refused
    );
    // The caller is checked before the arguments, as Python's adapter does.
    let invalid = Metadata::new();
    assert_eq!(
        creator.handler("plan_create".into(), invalid).await,
        refused
    );
}

/// Changing one status of an 18-step plan used to send and return the whole
/// list (RMK-1639): `step_changes` names one step, and so does the text.
#[tokio::test]
async fn an_agent_changes_one_step_and_reads_only_that_step() {
    let store = Arc::new(MemoryPlanBackend::default());
    let creator = PlanCreate::new(PlanCreateTool::new(store.clone()));
    let updater = PlanUpdate::new(PlanUpdateTool::new(store));
    let (room, agent) = ("plans", "assistant");
    let steps: Vec<Value> = (1..=18)
        .map(|i| json!({"step_id": format!("s{i}"), "description": format!("Step {i}"), "status": "pending"}))
        .collect();
    let request = json!({"title": "Release", "objective": "Ship", "steps": steps});
    data(call::run(&creator, room, agent, "plan_create", request).await);
    let change = json!({"plan_id": "plan-1", "expected_revision": 1,
        "step_changes": [{"step_id": "s3", "status": "completed"}]});
    let Ok(ToolResult::Json(envelope)) =
        call::run(&updater, room, agent, "plan_update", change.clone()).await
    else {
        unreachable!("plan_update returns its envelope");
    };
    assert_eq!(
        envelope["text"],
        "plan-1 (revision 2): 1 step changed\n3. s3 [completed]: \"Step 3\"\n"
    );
    assert_eq!(envelope["data"]["steps"].as_array().unwrap().len(), 18);
    assert!(change.to_string().len() < 150);
}
