# roomkit-tools

Composable tools for coding agents, implemented in Rust and available through
an asynchronous Python API. Use them directly, or pass individual tools to a
[RoomKit](https://github.com/roomkit-live/roomkit) agent.

Choose the capabilities your agent needs: files, code outlines, commands,
persistent processes, web access, images and delegated work. The host controls
workspaces, limits, credentials and resource lifetimes.

See [installation and compatibility](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/installation.md)
for available artifacts and supported platforms.

## Install from a checkout

Requires CPython 3.12+ and Rust to compile the extension. Run from the repository
root; prebuilt GitHub wheels are also described in the installation guide.

```sh
python3 -m venv .venv
source .venv/bin/activate
python -m pip install '.[roomkit]'
```

Omit `[roomkit]` for standalone use. The base Python package has no Python runtime
dependencies; it includes the native Rust extension. No model server is required
to use its filesystem, command, code or image preparation APIs.

## Use with RoomKit

Add tools to an existing RoomKit AI channel:

```python
from roomkit import AIChannel
from roomkit_tools import Edit, Grep, Read, Workspace, Write

workspace = Workspace("./project")  # An existing directory.
agent = AIChannel(
    "assistant",
    provider=provider,  # Your configured RoomKit AI provider.
    tools=[
        Read(root=workspace),
        Grep(root=workspace),
        Write(root=workspace),
        Edit(root=workspace),
    ],
)
```

Only those four tools are exposed. Sharing `Workspace` coordinates mutations
made through its tools. RoomKit manages the conversation and tool execution;
`roomkit-tools` provides the implementations and Rust-derived schemas.

For a complete example with no API key or server:

```sh
python examples/roomkit_read.py
```

It uses a scripted model with a real RoomKit conversation and a real Rust file
read. The [examples index](https://github.com/roomkit-live/roomkit-tools/blob/main/examples/README.md)
includes live-model configuration, a coding workflow and agent delegation.

## Use without RoomKit

```python
import asyncio
import tempfile
from pathlib import Path

from roomkit_tools import ReadTool

async def main():
    with tempfile.TemporaryDirectory() as directory:
        Path(directory, "notes.txt").write_text("Status: ready\n", encoding="utf-8")
        read = await ReadTool.create(directory)
        result = await read.read("notes.txt")
        print(result["text"])  # 1: Status: ready

asyncio.run(main())
```

Python calls the same Rust implementation. Results include structured `data`
and a readable `text` representation. Schemas, typed errors and cancellation
are documented in the [Python guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/python.md).

Rust applications can depend on individual workspace crates. See the
[Rust integration guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/rust.md)
and the checked examples in `examples/`. No crates are published to crates.io yet.

## Available tools

| Capability | Model-facing tools | Guide |
| --- | --- | --- |
| Files | `read`, `list`, `glob`, `grep`, `write`, `edit`, `diff`, `patch` | [Filesystem](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/filesystem.md) |
| Code structure | `code_outline` for Rust and Python | [Code](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/code.md) |
| Commands | `shell` | [Processes](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/processes.md) |
| Persistent processes | `process_start`, `process_status`, `process_read`, `process_write`, `process_stop` | [Processes](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/processes.md) |
| Output archives | `output_read`, `output_forget` | [Processes](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/processes.md#optional-command-output-retention) |
| Web | `web_fetch`, `web_search` with Exa, Ollama or OpenAI backends | [Web](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/web.md) |
| Images | `view_image` for static PNG, JPEG and WebP | [Images](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/images.md) |
| Text recognition | `ocr` with an optional, separately installed RustO backend | [OCR](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/ocr.md) |
| Delegation | `delegate_task`, `task_status`, `cancel_task`, `agent_followup`, `agent_send` | [Agents](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/agents.md) |

All are independently selectable in RoomKit Python. The agent tools use portable
Rust contracts and an injected execution backend; a RoomKit Python backend is
included. The optional RoomKit Rust adapter supports the file, code, command and
web tools, plus OCR behind its `ocr` feature; native agent orchestration and image-result transport are not yet
implemented there.

## Execution boundaries

- Filesystem roots and byte limits are host configuration. Path checks assume a
  host-controlled workspace; they are not an operating-system sandbox.
- Commands run with the host's permissions and environment. A working directory
  does not restrict what a shell command can access.
- Mutations can carry content revisions to reject stale writes. Cancellation
  does not undo effects already committed.
- Process sessions, output stores and agent backends need explicit cleanup.
  Their retained state is not durable across host restarts.
- Image interpretation requires a vision-capable model. Web search requires a
  configured service; its provider can differ from the main agent's provider.

## Documentation and development

Start with the [documentation index](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/README.md).
It covers installation, each tool family, integration boundaries and resource
ownership. The [changelog](https://github.com/roomkit-live/roomkit-tools/blob/main/CHANGELOG.md)
tracks user-visible changes.

For development, see [CONTRIBUTING](https://github.com/roomkit-live/roomkit-tools/blob/main/CONTRIBUTING.md).
`make all` checks the standalone Rust workspace; `make python-check` validates
independent wheel and source installations, including the RoomKit adapter.

## License

MIT. See [LICENSE](https://github.com/roomkit-live/roomkit-tools/blob/main/LICENSE).
Native dependency notices are bundled with the Python package.
