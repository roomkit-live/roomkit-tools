# Agent delegation

`delegate_task`, `task_status`, `cancel_task`, `agent_followup` and `agent_send`
are independently selectable.
Their Rust crate, `roomkit-tools-agents`, derives schemas from its actual input
and output types and exposes an open `AgentBackend` trait with boxed futures.
It runs no async runtime and has no model provider or RoomKit dependency. Tool
objects only hold the injected backend; the backend owns execution and state.
Each Rust call takes a `CancellationToken`, as every tool's does, and passes it
to the backend; Python cancels by cancelling the awaiting task. A native RoomKit
Rust backend is not implemented yet.

| Tool | Model arguments | Purpose |
| --- | --- | --- |
| `delegate_task` | `profile`, `task` | Launch a fresh instance from a host-approved profile |
| `task_status` | Optional `task_id` | Inspect one task or list the caller's retained tasks |
| `cancel_task` | `task_id` | Request cancellation and observe the resulting state |
| `agent_followup` | `agent_id`, `task` | Start another mission on an idle retained instance |
| `agent_send` | `agent_id`, `message` | Queue a clarification for a running instance |

Python exposes the same validated contracts through `DelegateTaskTool`,
`TaskStatusTool`, `CancelTaskTool`, `AgentFollowupTool` and `AgentSendTool`. A custom Python backend implements
the corresponding async methods in `roomkit_tools.agents.AgentBackend`; no Rust
callback implementation is needed. Backends lacking continuation/message support
return an explicit unsupported error when those tools are selected.
`tool.call(arguments, scope=AgentScope(...))` works without RoomKit.
`tool.definition()` exposes both schemas. The RoomKit adapters `DelegateTask`,
`TaskStatus`, `CancelTask`, `AgentFollowup` and `AgentSend` take a built tool,
`DelegateTask(DelegateTaskTool(backend))`, and implement RoomKit's Tool
protocol. Inputs and normalized outputs are checked
by Rust, including unknown fields, UTF-8 byte limits and terminal states.

## RoomKit backend

The optional `RoomKitAgentBackend` uses RoomKit's existing delegation runner,
child rooms and `kit.deliver()` relay. Configure it once and select tools:

```python
from roomkit import AIChannel
from roomkit_tools import (
    AgentFollowup, AgentFollowupTool, AgentScope, AgentSend, AgentSendTool, CancelTask,
    CancelTaskTool, DelegateTask, DelegateTaskTool, Read, ReadTool, TaskStatus,
    TaskStatusTool,
)
from roomkit_tools.roomkit_agents import AgentConfig, RoomKitAgentBackend

# kit and its "conversation" room already exist. The host supplies factories
# returning fresh AIProvider instances, configured with its model/credentials.
reader = await ReadTool.create("./project")  # Built once; each agent gets an adapter.
async with RoomKitAgentBackend(
    kit,
    scope=AgentScope("conversation", "assistant"),
    profiles={
        "researcher": AgentConfig(
            provider_factory=make_worker_provider,
            system_prompt="Analyze source using the supplied tools. Do not modify files.",
            tools_factory=lambda: [Read(reader)],
        ),
    },
) as agents:
    assistant = AIChannel(
        "assistant",
        provider=make_principal_provider(),
        tools=[
            DelegateTask(DelegateTaskTool(agents)), TaskStatus(TaskStatusTool(agents)),
            CancelTask(CancelTaskTool(agents)), AgentFollowup(AgentFollowupTool(agents)),
            AgentSend(AgentSendTool(agents)),
        ],
    )
    # Register/attach assistant and run the host conversation here.
    # Exit this context before closing kit.
```

For a complete small example, run [roomkit_delegate.py](../examples/roomkit_delegate.py).
It registers only `DelegateTask` on the principal and `Read` on the researcher,
then waits for the report before closing the owned backend. The default run
uses scripted providers, a real RoomKit conversation and an actual Rust file read:

```sh
python examples/roomkit_delegate.py
# With roomkit[vllm], OPENAI_BASE_URL and OPENAI_MODEL configured:
python examples/roomkit_delegate.py --live
```

## Profiles and agent instances

A static profile is a reusable blueprint, not a shared live agent. Each
delegation creates a unique agent instance and a new child room, with only
the task as initial context. Provider factories must return fresh instances;
RoomKit closes each provider when its instance is released. Tools with their
own sessions remain host-owned. `agent_followup(agent_id, task)` starts a new
mission on an idle instance, retaining its child room, provider and tools. The
resolver is not called again. The task ID changes; the agent ID stays the same.
Prior conversation remains in the store; RoomKit's context limits still govern
what the model sees. Busy or expired instances refuse follow-ups.

`agent_send(agent_id, message)` queues a clarification to a running loop using
RoomKit steering. `queued` means accepted for a future tool-round boundary, not
already read or acted upon. It also records the text in the child conversation
without starting a new turn, reporting `history_recorded` separately. If that
write fails, the steering remains accepted: do not blindly resend it. An idle
agent returns `not_running` and starts nothing; use `agent_followup` instead.
A final model response may finish before the next steering boundary.

Follow-ups use the public `kit.task_runner.submit()` with the retained child room;
initial delegations use `kit.delegate()`. Manage these missions through this
backend's status/cancel tools, not RoomKit's built-in room task catalogue, which
does not register submissions made directly to the runner. Reports use RoomKit's
hand-back helper and `kit.deliver()` to let the principal respond naturally.

## Dynamic configuration

For dynamic configuration, replace `profiles` with a resolver and an explicit
allow list; the tools remain the same:

```python
async def resolve_agent(request):
    # request.profile, request.task and request.scope are immutable.
    # Look up tenant configuration or select an approved model/toolset here.
    return AgentConfig(
        provider_factory=make_worker_provider,
        system_prompt="Inspect the requested source and report observed facts.",
        tools_factory=lambda: [Read(reader)],  # A ReadTool the host built once.
    )

agents = RoomKitAgentBackend(
    kit,
    scope=AgentScope("conversation", "assistant"),
    resolver=resolve_agent,
    allowed_profiles=("researcher", "reviewer"),
)
```

Resolution runs once per launch, after access and capacity checks. The host
controls provider, credentials, tools, workspace and deadlines. Delegation arguments
contain only `profile` and `task`; allowed profiles also appear in the schema.
Follow-ups and messages address an existing `agent_id` without changing its configuration.
The RoomKit handler reads the actual calling channel and room from its tool
context, then the backend verifies its fixed conversation/principal scope on
every operation. These checks do not authenticate end users or provide an OS
sandbox. Apply end-user policy in the host before invoking tools.

## Resource ownership and limits

Defaults allow two concurrent instances, eight retained tasks, ten seconds for
resolution and 180 seconds for the worker tool loop. Idle instances expire after
300 seconds (`idle_timeout`); availability is reported as `agent_available`.
Messages are limited to 8 KiB and 32 accepted messages per instance (`max_messages`).
The maximum retained task count is 64; ended receipts are retained without silent eviction. Task text is
limited to 16 KiB and retained result text to 8 KiB, with `result_truncated`
explicitly reported. This bound covers the registry's copy, not RoomKit's own
history or the full report delivered by its relay. A backend is tied to one
asyncio loop and does not persist across restarts.

Agent IDs are opaque: copy them verbatim from receipts or `task_status`.
An unknown ID is an exact lookup failure, not an availability observation.
The RoomKit backend refuses it without side effects and returns a bounded list
of known IDs after checking the caller's scope. It never guesses a replacement.
Correct the ID before retrying; a new delegation does not resume the same instance.

`task_id`, `agent_id` and `profile` are distinct. `status` describes worker
execution; `delivery` is independently `pending`, `delivered`, `failed` or
`suppressed`. A completed worker does not by itself confirm a principal reply.

## Result delivery and shutdown

The principal receives the report as an internal instruction and answers in
the conversation. Cancellation suppresses a duplicate cancellation relay; an
already completed task is unchanged. A delivery failure is reported without
automatically retrying or rerunning the worker.

Cancelling a caller waiting for delegation does not revoke its accepted launch.
Recover with `task_status` before retrying: `pending_launches` reports outstanding
configuration resolutions that do not yet have task IDs. Cancellation and
`aclose()` own shielded cleanup tasks. Always close the backend before RoomKit;
close stops its launches/tasks and releases channels/providers. Child-room history
remains in the RoomKit store. Cleanup failures are reported. Close waits for a principal turn already receiving a report; configure
that channel's provider/turn deadlines as well. Abrupt process termination is
outside this in-memory contract.

## Validation examples

Start with [roomkit_delegate.py](../examples/roomkit_delegate.py) for a complete,
small integration. The [agent lifecycle example](../examples/README.md#agent-lifecycle)
adds static/dynamic profiles, parallel tasks, messages, cancellation and follow-ups.

[Documentation index](README.md) · [Project overview](../README.md)
