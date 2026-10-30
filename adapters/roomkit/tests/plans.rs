//! Independently selected plan toolsets derive ownership from RoomKit context.
#![cfg(feature = "plans")]
use roomkit::model::{ChannelId, RoomId, ToolCall};
use roomkit_ai::{ToolContext, ToolError, ToolOutput, Toolset};
use roomkit_tools_plans::{
    MemoryPlanBackend, PlanCreateTool, PlanDeleteTool, PlanListTool, PlanReadTool, PlanUpdateTool,
};
use roomkit_tools_roomkit::{
    PlanCreateToolset, PlanDeleteToolset, PlanListToolset, PlanReadToolset, PlanUpdateToolset,
};
use serde_json::{Value, json};
use std::sync::Arc;

#[expect(
    clippy::unwrap_used,
    reason = "test context uses valid fixture identifiers"
)]
fn context(room: &str, agent: &str) -> ToolContext {
    ToolContext::new(RoomId::new(room).unwrap(), ChannelId::new(agent).unwrap())
}
#[expect(clippy::unwrap_used, reason = "test arguments are fixture objects")]
fn call(name: &str, arguments: Value) -> ToolCall {
    ToolCall::new("id", name, serde_json::from_value(arguments).unwrap())
}
#[expect(clippy::panic, reason = "unexpected test output must fail the test")]
fn data(output: ToolOutput) -> Value {
    let ToolOutput::Json(value) = output else {
        panic!("expected JSON");
    };
    value["data"].clone()
}
#[tokio::test]
async fn scoped_crud_adapters_preserve_store_after_close() {
    let store = Arc::new(MemoryPlanBackend::default());
    let creator = PlanCreateToolset::new(PlanCreateTool::new(store.clone()));
    let reader = PlanReadToolset::new(PlanReadTool::new(store.clone()));
    let updater = PlanUpdateToolset::new(PlanUpdateTool::new(store.clone()));
    let lister = PlanListToolset::new(PlanListTool::new(store.clone()));
    let deleter = PlanDeleteToolset::new(PlanDeleteTool::new(store));
    assert_eq!(
        creator.tools()[0].parameters,
        PlanCreateTool::definition().input_schema
    );
    let owner = context("plans", "assistant");
    let request = call(
        "plan_create",
        json!({"title":"Inspect","objective":"Check boundary","steps":[]}),
    );
    let plan = data(creator.call(request.clone(), owner.clone()).await.unwrap());
    creator.close().await;
    assert!(matches!(
        creator.call(request, owner.clone()).await,
        Err(ToolError::Refused(_))
    ));
    let request = call("plan_read", json!({"plan_id":plan["plan_id"]}));
    assert_eq!(
        data(reader.call(request.clone(), owner.clone()).await.unwrap()),
        plan
    );
    for other in [context("other", "assistant"), context("plans", "other")] {
        assert!(matches!(
            reader.call(request.clone(), other.clone()).await,
            Err(ToolError::Refused(_))
        ));
        assert_eq!(
            data(
                lister
                    .call(call("plan_list", json!({})), other)
                    .await
                    .unwrap()
            )["plans"],
            json!([])
        );
    }
    let update = call(
        "plan_update",
        json!({"plan_id":plan["plan_id"],"expected_revision":1,"title":"Changed"}),
    );
    assert_eq!(
        data(updater.call(update.clone(), owner.clone()).await.unwrap())["revision"],
        2
    );
    assert!(matches!(
        updater.call(update, owner.clone()).await,
        Err(ToolError::Refused(_))
    ));
    let deletion = call(
        "plan_delete",
        json!({"plan_id":plan["plan_id"],"expected_revision":2}),
    );
    assert_eq!(
        data(deleter.call(deletion, owner.clone()).await.unwrap())["deleted"],
        true
    );
    assert_eq!(
        data(
            lister
                .call(call("plan_list", json!({})), owner)
                .await
                .unwrap()
        )["plans"],
        json!([])
    );
}
