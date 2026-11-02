//! Plan storage and cross-language contract checks.
use roomkit_tools_plans::{
    MemoryPlanBackend, PlanCreateTool, PlanDeleteTool, PlanListTool, PlanReadTool, PlanScope,
    PlanUpdateTool, decode,
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
    assert_eq!(outputs[3]["data"]["steps"][0]["step_id"], "test");
    assert_eq!(outputs[4]["data"]["revision"], 2); // No-op keeps the revision.
    assert_eq!(outputs[5]["data"]["plans"][0]["completed_steps"], 1);
    assert_eq!(outputs[6]["data"]["steps"].as_array().unwrap().len(), 1);
    assert_eq!(outputs[7]["data"]["deleted"], true);
    assert_eq!(outputs[8]["data"]["plans"], serde_json::json!([]));
    if let Ok(path) = std::env::var("ROOMKIT_PLAN_RESULTS") {
        std::fs::write(path, serde_json::to_vec(&outputs).unwrap()).unwrap();
    }
}
