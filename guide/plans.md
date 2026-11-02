# Plans

Select `plan_create`, `plan_read`, `plan_list`, `plan_update` and `plan_delete`
independently. They record a plan and its declared progress through an injected
storage backend. Updating a step does not run commands, delegate work, verify
completion or switch the agent into a planning mode. The host controls those
behaviors separately.

## RoomKit Python

One shared backend connects the tools. Pass only the capabilities an agent needs:

```python
from roomkit import AIChannel
from roomkit_tools import (
    MemoryPlanBackend, PlanCreate, PlanCreateTool, PlanDelete, PlanDeleteTool, PlanList,
    PlanListTool, PlanRead, PlanReadTool, PlanUpdate, PlanUpdateTool,
)

store = MemoryPlanBackend()
channel = AIChannel(
    "assistant",
    provider=provider,  # Your configured RoomKit provider.
    tools=[PlanCreate(PlanCreateTool(store)), PlanRead(PlanReadTool(store)),
           PlanList(PlanListTool(store)), PlanUpdate(PlanUpdateTool(store)),
           PlanDelete(PlanDeleteTool(store))],
)
```

For a reader, select only `PlanRead(PlanReadTool(store))` and
`PlanList(PlanListTool(store))`. A tool object
owns configuration, not a private copy of the plans. Reuse the same backend across
turns. Each handler derives ownership from RoomKit's current room and acting AI
channel; callers cannot choose an owner in model arguments. Two conversations,
or two AI channels in one conversation, have separate plans. A delegated agent
does not automatically inherit the principal's plans.

The included `MemoryPlanBackend` uses Rust storage, has no background worker and
requires no `aclose()`. Plans disappear when the last reference is dropped or the
host process exits. Separate backend instances are independent, and their IDs
are meaningful only with that backend and scope.

## Operations

| Tool | Input | Result |
| --- | --- | --- |
| `plan_create` | `title`, `objective`, ordered `steps` | Full plan with generated `plan_id` and revision 1 |
| `plan_read` | `plan_id` | Current full plan |
| `plan_list` | `{}` | All accessible summaries: ID, revision, title, total/completed step counts |
| `plan_update` | `plan_id`, `expected_revision`, one or more of `title`, `objective`, `steps` | Full updated plan |
| `plan_delete` | `plan_id`, `expected_revision` | Receipt with ID, deleted revision and `deleted: true` |

Each step has a unique `step_id`, a `description` and a `status`:
`pending`, `in_progress`, `blocked`, `completed` or `cancelled`. Choose a short
stable ID when creating a step, and reuse it when changing its description,
status or position. Multiple steps may be in progress for parallel work; no
transition workflow is imposed. An empty steps list is valid.

An update preserves omitted/null fields. **Supplying `steps` replaces the entire
ordered list.** Include unchanged steps, keep their IDs and omit entries you want
to remove. Sending `steps: []` clears the list. There is no implicit merge of step
arrays and no automatic deletion of completed plans.

Updates and deletions require the current revision. A real change increments it;
a no-op keeps it. A stale revision is refused atomically, leaving the plan
unchanged. Read again and reconcile your intended change before retrying.
Missing and inaccessible IDs produce the same refusal. Repeated deletion is a
refusal, not a second successful receipt.

Deletion removes only the plan record. It does not cancel delegated tasks,
processes, files or other plans. Completion is a declaration, not evidence that
the described work actually happened.

## Standalone Python and custom storage

No RoomKit dependency is needed for direct calls:

```python
from roomkit_tools import (
    MemoryPlanBackend, PlanCreateTool, PlanReadTool, PlanScope, PlanUpdateTool,
)

store = MemoryPlanBackend()
scope = PlanScope(room_id="review", caller_id="assistant")  # Trusted host identity.
result = await PlanCreateTool(store).call({
    "title": "Review shipping",
    "objective": "Check the fee boundary",
    "steps": [{"step_id": "inspect", "description": "Read the source", "status": "pending"}],
}, scope=scope)
plan = result["data"]
plan["steps"][0]["status"] = "in_progress"
await PlanUpdateTool(store).call({
    "plan_id": plan["plan_id"],
    "expected_revision": plan["revision"],
    "steps": plan["steps"],
}, scope=scope)
current = await PlanReadTool(store).call({"plan_id": plan["plan_id"]}, scope=scope)
```

`PlanBackend` describes the five async host methods: `create`, `read`, `list`,
`update`, `delete`. Each receives a trusted `PlanScope` plus keyword arguments and
returns the data dictionary, without the `{data, text}` envelope. The facades
validate inputs and outputs against Rust contracts. A custom backend must enforce
ownership on every operation, atomic revision checks, capacity, unique IDs and
bounded records; declare its persistence guarantees. Implementations can use
application storage. No database backend is included in this version.

`PlanCreateTool.definition()` (and the other tools') exposes Rust-derived
input/output schemas, and `.call()` returns `{data, text}`. The RoomKit adapter
`PlanCreate(tool)` has the RoomKit `.definition` and a `.handler()` that uses
trusted RoomKit context and records structured results.

Cancellation can arrive after a mutation has committed but before its receipt
is delivered. It does not roll back that mutation. Recover with `plan_read` or
`plan_list`; blindly retrying `plan_create` can create a duplicate plan.

## Rust

Use the `roomkit-tools-plans` crate and an `Arc<dyn PlanBackend>`. Each
`Plan*Tool::new(backend)` exposes `definition()` and
`call(&scope, input, &cancellation_token)`. `MemoryPlanBackend` clones share
one store; independent instances do not. The crate has no RoomKit, Python or
filesystem dependency and does not require a particular async runtime.

`cargo run --example plan_progress` checks standalone CRUD. For RoomKit Rust,
enable the optional adapter's `plans` feature and select `PlanCreate`,
`PlanRead`, `PlanList`, `PlanUpdate` or `PlanDelete`.
Each wraps its corresponding configured native tool and derives the scope from
the room and agent of the current RoomKit tool call; a call made outside one is
refused. Dropping a call cancels it; the host-owned backend is never erased by
an adapter.

## Bounds

| Resource | Bound |
| --- | --- |
| Plans retained in memory | Default 256 total, 64 per scope; configurable total 1–4096, per scope 1–64 and no greater than total |
| Steps | 64 per plan |
| Title / objective | 256 / 16,384 UTF-8 bytes |
| Step ID / description | 64 ASCII letters, digits, `_` or `-` / 2,048 UTF-8 bytes |
| Complete plan | 64 KiB serialized JSON, also checked after merging an update |
| Revision | Positive JSON-safe integer, at most 2^53−1 |

Capacity exhaustion is explicit; there is no silent eviction. `plan_list` returns
all summaries within the bounded scope, in creation order in the memory backend.
List results are bounded below 256 KiB including JSON escaping. Unknown input
fields and invalid step IDs/statuses are rejected before storage is called.

## Try it

After installing the current source with the RoomKit extra:

```sh
python examples/roomkit_plans.py
# Optional interactive model session with your configured OPENAI_* variables:
python examples/roomkit_plans.py --live
```

The deterministic example checks actual create/read/update/list/delete calls
through RoomKit. In live chat, ask for a plan, report that an initial step is
done, ask to add/reorder steps and finally delete the plan. `/plans` reads actual
host state without calling the model; `quit` closes the conversation. These
examples are in-memory demonstrations, not durable project trackers.

[Documentation index](README.md) · [Python](python.md) · [Rust](rust.md)
