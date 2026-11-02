# roomkit-tools-agents

Delegation tools over an agent backend the host implements: `delegate_task`,
`task_status`, `cancel_task`, `agent_followup` and `agent_send`. The tools
validate the model's arguments and the backend's receipts, and render them;
the `AgentBackend` resolves profiles, runs the agents and owns their tasks.
This crate runs no model and schedules nothing.

## Features

None.

## Example

```rust
use std::sync::Arc;

use roomkit_tools_agents::{AgentBackend, AgentScope, DelegateTaskInput, DelegateTaskTool};
use tokio_util::sync::CancellationToken;

/// Start a task on the host's backend; returns its identifier.
async fn delegate(backend: Arc<dyn AgentBackend>) -> Result<String, Box<dyn std::error::Error>> {
    let tool = DelegateTaskTool::new(backend);
    // Trusted host context, never a model argument.
    let scope = AgentScope { room_id: "support".into(), caller_id: "assistant".into() };
    let input = DelegateTaskInput { profile: "reader".into(), task: "Summarize CHANGELOG.md".into() };
    let receipt = tool.call(&scope, input, &CancellationToken::new()).await?;
    Ok(receipt.data.task_id)
}
```

## License

MIT. Part of [roomkit-tools](https://github.com/roomkit-live/roomkit-tools),
agent tools a host selects one by one, also available in Python. The
[Rust guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/rust.md)
describes every crate.
