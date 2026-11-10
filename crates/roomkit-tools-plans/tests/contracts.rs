//! Plan storage and cross-language contract checks.
use roomkit_tools_plans::{
    MemoryPlanBackend, Plan, PlanBackend, PlanCreateInput, PlanCreateTool, PlanDeleteInput,
    PlanDeleteResult, PlanDeleteTool, PlanFuture, PlanListInput, PlanListResult, PlanListTool,
    PlanReadInput, PlanReadTool, PlanScope, PlanStep, PlanUpdateInput, PlanUpdateTool, StepStatus,
    decode,
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
        "plan-1 (revision 1): \"Review shipping\"\nObjective: \"Validate the fee boundary\"\n\
         1. inspect [in_progress]: \"Read the source\"\n2. test [pending]: \"Test the boundary\"\n"
    );
    assert_eq!(outputs[3]["data"]["steps"][0]["step_id"], "test");
    assert_eq!(outputs[4]["data"]["revision"], 2); // No-op keeps the revision.
    assert_eq!(outputs[5]["data"]["plans"][0]["completed_steps"], 1);
    assert_eq!(outputs[6]["data"]["steps"].as_array().unwrap().len(), 1);
    // A change by step names only that step; data holds the whole plan.
    assert_eq!(outputs[7]["data"]["revision"], 4);
    assert_eq!(
        outputs[7]["text"],
        "plan-1 (revision 4): 1 step changed\n1. inspect [pending]: \"Reread the source\"\n"
    );
    assert_eq!(outputs[8]["data"]["deleted"], true);
    assert_eq!(outputs[8]["text"], "plan-1 (revision 4) deleted\n");
    assert_eq!(outputs[9]["data"]["plans"], serde_json::json!([]));
    assert_eq!(outputs[9]["text"], "no plans\n");
    if let Ok(path) = std::env::var("ROOMKIT_PLAN_RESULTS") {
        std::fs::write(path, serde_json::to_vec(&outputs).unwrap()).unwrap();
    }
}

/// `step_changes` changes only the steps it names, and the text names them
/// alone.
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
        "plan-1 (revision 2): 1 step changed\n3. s3 [completed]: \"Step 3\"\n"
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

/// A host backend that checks nothing: the tool checks what it hands it.
#[derive(Debug)]
struct Lax {
    plan: Plan,
    updates: std::sync::atomic::AtomicUsize,
}

impl PlanBackend for Lax {
    fn create<'a>(
        &'a self,
        _: &'a PlanScope,
        _: &'a PlanCreateInput,
        _: &'a CancellationToken,
    ) -> PlanFuture<'a, Plan> {
        Box::pin(async { Ok(self.plan.clone()) })
    }
    fn read<'a>(
        &'a self,
        _: &'a PlanScope,
        _: &'a PlanReadInput,
        _: &'a CancellationToken,
    ) -> PlanFuture<'a, Plan> {
        Box::pin(async { Ok(self.plan.clone()) })
    }
    fn list<'a>(
        &'a self,
        _: &'a PlanScope,
        _: &'a PlanListInput,
        _: &'a CancellationToken,
    ) -> PlanFuture<'a, PlanListResult> {
        Box::pin(async { Ok(PlanListResult { plans: Vec::new() }) })
    }
    fn update<'a>(
        &'a self,
        _: &'a PlanScope,
        input: &'a PlanUpdateInput,
        _: &'a CancellationToken,
    ) -> PlanFuture<'a, Plan> {
        self.updates
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut plan = self.plan.clone();
        plan.steps = input.steps.clone().unwrap_or_default();
        plan.revision += 1;
        Box::pin(async { Ok(plan) })
    }
    fn delete<'a>(
        &'a self,
        _: &'a PlanScope,
        _: &'a PlanDeleteInput,
        _: &'a CancellationToken,
    ) -> PlanFuture<'a, PlanDeleteResult> {
        Box::pin(async { Err(roomkit_tools_core::ToolError::Cancelled) })
    }
}

#[tokio::test]
async fn step_changes_are_checked_before_a_host_backend_receives_them() {
    let steps = (1..=64)
        .map(|i| PlanStep {
            step_id: format!("s{i}"),
            description: "d".repeat(900),
            status: StepStatus::Pending,
        })
        .collect();
    let plan = Plan {
        plan_id: "plan-1".into(),
        revision: 1,
        title: "Release".into(),
        objective: "Ship".into(),
        steps,
    };
    let backend = Arc::new(Lax {
        plan,
        updates: 0.into(),
    });
    let shared: Arc<dyn PlanBackend> = Arc::<Lax>::clone(&backend);
    let tool = PlanUpdateTool::new(shared);
    let scope = PlanScope {
        room_id: "plans".into(),
        caller_id: "assistant".into(),
    };
    let cancel = CancellationToken::new();
    let changes: Vec<Value> = (1..=20)
        .map(|i| serde_json::json!({"step_id": format!("s{i}"), "description": "e".repeat(2048)}))
        .collect();
    let call = |expected_revision: u64, changes: &[Value]| {
        let arguments = serde_json::json!({"plan_id": "plan-1",
            "expected_revision": expected_revision, "step_changes": changes});
        decode::<PlanUpdateInput>(&arguments.to_string()).unwrap()
    };
    // The plan they make is past the 64 KiB a plan record holds.
    let refused = tool.call(&scope, call(1, &changes), &cancel).await;
    assert!(
        matches!(&refused, Err(roomkit_tools_core::ToolError::LimitExceeded(m))
            if m == "plan record exceeds 64 KiB JSON"),
        "{refused:?}"
    );
    // A stale revision is a conflict, whatever the steps of the current one.
    let stale = tool.call(&scope, call(7, &changes[..1]), &cancel).await;
    assert!(
        matches!(&stale, Err(roomkit_tools_core::ToolError::AccessRefused(m))
            if m == "plan revision conflict; use plan_read before retrying"),
        "{stale:?}"
    );
    assert_eq!(
        backend.updates.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[tokio::test]
async fn a_step_change_that_changes_nothing_says_so() {
    let backend: Arc<dyn PlanBackend> = Arc::new(MemoryPlanBackend::default());
    let scope = PlanScope {
        room_id: "plans".into(),
        caller_id: "assistant".into(),
    };
    let cancel = CancellationToken::new();
    let create = serde_json::json!({"title": "Release", "objective": "Ship",
        "steps": [{"step_id": "s1", "description": "Step 1", "status": "pending"}]});
    PlanCreateTool::new(Arc::clone(&backend))
        .call(&scope, decode(&create.to_string()).unwrap(), &cancel)
        .await
        .unwrap();
    let same = serde_json::json!({"plan_id": "plan-1", "expected_revision": 1,
        "step_changes": [{"step_id": "s1", "status": "pending"}]});
    let result = PlanUpdateTool::new(backend)
        .call(&scope, decode(&same.to_string()).unwrap(), &cancel)
        .await
        .unwrap();
    assert_eq!(result.text, "plan-1 (revision 1): no change\n");
}

#[tokio::test]
async fn the_plan_step_changes_make_is_checked_before_a_host_backend_stores_it() {
    let steps = (1..=64)
        .map(|i| PlanStep {
            step_id: format!("s{i}"),
            description: "d".repeat(600),
            status: StepStatus::Pending,
        })
        .collect();
    let plan = Plan {
        plan_id: "plan-1".into(),
        revision: 1,
        title: "Release".into(),
        objective: "o".repeat(16 * 1024),
        steps,
    };
    let backend = Arc::new(Lax {
        plan,
        updates: 0.into(),
    });
    let shared: Arc<dyn PlanBackend> = Arc::<Lax>::clone(&backend);
    let changes: Vec<Value> = (1..=8)
        .map(|i| serde_json::json!({"step_id": format!("s{i}"), "description": "e".repeat(2048)}))
        .collect();
    let arguments = serde_json::json!({"plan_id": "plan-1", "expected_revision": 1,
        "step_changes": changes});
    let scope = PlanScope {
        room_id: "plans".into(),
        caller_id: "assistant".into(),
    };
    // The update fits its own bound; the plan it makes, with its objective, does not.
    let refused = PlanUpdateTool::new(shared)
        .call(
            &scope,
            decode(&arguments.to_string()).unwrap(),
            &CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(&refused, Err(roomkit_tools_core::ToolError::LimitExceeded(m))
            if m == "plan record exceeds 64 KiB JSON"),
        "{refused:?}"
    );
    assert_eq!(
        backend.updates.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[tokio::test]
async fn a_step_change_with_a_new_title_shows_the_whole_plan() {
    let backend: Arc<dyn PlanBackend> = Arc::new(MemoryPlanBackend::default());
    let scope = PlanScope {
        room_id: "plans".into(),
        caller_id: "assistant".into(),
    };
    let cancel = CancellationToken::new();
    let create = serde_json::json!({"title": "Release", "objective": "Ship",
        "steps": [{"step_id": "s1", "description": "Step 1", "status": "pending"}]});
    PlanCreateTool::new(Arc::clone(&backend))
        .call(&scope, decode(&create.to_string()).unwrap(), &cancel)
        .await
        .unwrap();
    let change = serde_json::json!({"plan_id": "plan-1", "expected_revision": 1, "title": "Ship it",
        "step_changes": [{"step_id": "s1", "status": "completed"}]});
    let result = PlanUpdateTool::new(backend)
        .call(&scope, decode(&change.to_string()).unwrap(), &cancel)
        .await
        .unwrap();
    assert_eq!(
        result.text,
        "plan-1 (revision 2): \"Ship it\"\nObjective: \"Ship\"\n1. s1 [completed]: \"Step 1\"\n"
    );
}
