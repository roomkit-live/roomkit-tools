# Python and RoomKit integration

See [installation](installation.md) for source and wheel setup. The distribution
name is `roomkit-tools`; the import name is `roomkit_tools`.

## Two ways to call the tools

| Use | Example | Result |
| --- | --- | --- |
| Standalone async API | `await read.read("notes.txt")` on a `ReadTool` | Rust `data`/`text` envelope as a Python dictionary |
| RoomKit tool selection | `tools=[Read(root="./project")]` | RoomKit calls the tool with the same Rust-derived input schema |

Most tools have a short RoomKit class (`Read`, `Grep`, `ViewImage`) and a
standalone implementation wrapper (`ReadTool`, `GrepTool`, `ViewImageTool`).
Process tools share a `ProcessSession`; output tools share an `OutputStore`.
Agent tools use an injected backend and are described in [their own guide](agents.md).

## Minimal standalone call

```python
import asyncio
import tempfile
from pathlib import Path
from roomkit_tools import ReadTool

async def main():
    with tempfile.TemporaryDirectory() as root:
        Path(root, "notes.txt").write_text("Status: ready\n", encoding="utf-8")
        read = await ReadTool.create(root)
        result = await read.read("notes.txt", offset=1, limit=5)
        print(result["data"]["lines"])
        print(result["text"])
        print(read.definition()["input_schema"])

asyncio.run(main())
```

`call(arguments)` accepts a JSON-shaped dictionary validated against the actual
Rust input type. Host configuration, such as root directories and byte budgets,
is supplied at construction, not by the model. The tool definition also exposes
an output schema. Results report their own truncation or pagination; inspect
those fields before treating a preview as complete.

## Selecting tools in RoomKit

```python
from roomkit import AIChannel
from roomkit_tools import Edit, Read, Workspace, Write

workspace = Workspace("./project")
agent = AIChannel(
    "assistant",
    provider=provider,  # A RoomKit provider configured by your application.
    tools=[Read(workspace), Write(workspace), Edit(workspace)],
)
```

This is a fragment for an existing host. For room creation, channel attachment,
input delivery and cleanup, run the complete [read example](../examples/roomkit_read.py).

Tool construction captures an absolute root. The first call resolves it and
validates bounds asynchronously in Rust; the directory must exist by then.
Failed or cancelled initialization can be retried. Each tool is selected
explicitly. RoomKit retains argument validation, policies, hooks and turn
cancellation. No RoomKit Rust checkout or extra tool server is required.

Sharing `Workspace` coordinates `Write`, `Edit` and `Patch` with one native
mutation lock. Read-only tools use its path without that lock. Passing equal
path strings to separate writers does not create a shared lock. External
editors and shell commands are not coordinated by the workspace.

## Results and errors

Errors inherit from `ToolError`: `InvalidInputError`, `AccessRefusedError`,
`LimitExceededError`, `UnsupportedResourceError` and `ToolIOError`. I/O messages
omit OS diagnostics and host paths. Cancellation remains `asyncio.CancelledError`.
Async construction keeps root resolution off the Python event loop. Each native
tool call has its own cancellation scope; native wrappers can be reused across
event loops. The optional `RoomKitAgentBackend` is tied to one asyncio loop.
JSON argument encoding can also raise Python `TypeError` or `ValueError` before
Rust validation.

At normal interpreter exit, an automatic hook cancels outstanding native calls,
refuses new calls and drains Python-facing tasks and completion callbacks before
Python finalizes. The hook releases the GIL while waiting; abandoned or cancelled
awaiters cannot leave callbacks attaching to a finalized interpreter. This does
not replace explicit `ProcessSession` and `OutputStore` cleanup or handle abrupt termination.
Already-issued blocking I/O can delay exit until it returns.

The RoomKit classes expose the Rust-derived input schema through its `Tool` protocol.
Text tools return both result representations; `ViewImage` returns typed text and
image parts instead of embedding pixels in ordinary text. Refusal and resource failure stay
distinct: every RoomKit adapter raises `ToolRefusedError` for `InvalidInputError` and
`AccessRefusedError`, `ToolFailedError` for `LimitExceededError` and
`UnsupportedResourceError`, and lets `ToolIOError` and other errors propagate for
RoomKit to report without their message. The lower-level `roomkit_tools.roomkit.ReadToolAdapter` remains
available for an already-created `ReadTool`. Importing `roomkit_tools` and
configuring these tools does not import or require RoomKit; calling their
RoomKit handlers does.

## Owning resources

| Resource | Host responsibility |
| --- | --- |
| Rooted file/code/image tool | Choose a workspace and limits; no explicit close is required |
| `ProcessSession` | Keep alive for the conversation; close to stop jobs and release records |
| `OutputStore` | Close after command/session shutdown; logs are temporary |
| `RoomKitAgentBackend` | Close before `RoomKit`; it owns child instances/providers and task cleanup |

Prefer `async with` for resources that support it. A process or output-store
`close()` result of `False` means cleanup is still pending, not successful
shutdown. Their context managers raise on incomplete cleanup. See
[process lifecycle](processes.md) and [agent lifecycle](agents.md) for cancellation
and retry semantics. A cancelled mutation or start can already have effects;
observe state before retrying.

## Schemas and customization

Rust types define model inputs, output schemas and validation. Python does not
maintain independent filesystem or process implementations. Customize the host
configuration or select fewer tools to restrict an agent's capabilities.

Search uses an open Rust `SearchBackend` interface with built-in Python factories.
Agent execution accepts Rust or Python backends. Adding a new Rust search backend
to Python requires a binding factory; a custom Python agent backend implements
the async protocol directly. Neither interface configures the main AI provider.

[Documentation index](README.md) · [Project overview](../README.md)
