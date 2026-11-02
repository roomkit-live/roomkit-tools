# roomkit-tools-plans

Plan tools an agent uses to record its progress: `plan_create`, `plan_read`,
`plan_list`, `plan_update` and `plan_delete`, over storage the host injects.
Plans are scoped to a room and caller the host supplies, never the model, and
updates are checked against a revision. A plan records declared progress; it
neither schedules work nor proves completion. `MemoryPlanBackend` is a
bounded in-memory store.

## Features

None.

## Example

```rust
use std::sync::Arc;

use roomkit_tools_plans::{
    MemoryPlanBackend, PlanBackend, PlanCreateInput, PlanCreateTool, PlanScope, PlanStep,
    StepStatus,
};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // One store, shared by every plan tool the host selects.
    let store: Arc<dyn PlanBackend> = Arc::new(MemoryPlanBackend::default());
    // Trusted host context, never a model argument.
    let scope = PlanScope { room_id: "support".into(), caller_id: "assistant".into() };

    let plan = PlanCreateTool::new(store)
        .call(
            &scope,
            PlanCreateInput {
                title: "Refund".into(),
                objective: "Refund order 43".into(),
                steps: vec![PlanStep {
                    step_id: "check".into(),
                    description: "Check the order".into(),
                    status: StepStatus::Pending,
                }],
            },
            &CancellationToken::new(),
        )
        .await?;
    assert_eq!(plan.data.revision, 1);
    Ok(())
}
```

## License

MIT. Part of [roomkit-tools](https://github.com/roomkit-live/roomkit-tools),
agent tools a host selects one by one, also available in Python. The
[Rust guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/rust.md)
describes every crate.
