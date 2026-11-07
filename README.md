# RoomKit Tools

**Composable tools for AI coding agents, written in Rust, with an asyncio Python API.**

[![CI](https://github.com/roomkit-live/roomkit-tools/actions/workflows/ci.yml/badge.svg)](https://github.com/roomkit-live/roomkit-tools/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/roomkit-live/roomkit-tools/blob/main/LICENSE)
[![Python 3.12+](https://img.shields.io/badge/python-3.12%2B-blue.svg)](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/installation.md)
[![MSRV: 1.94](https://img.shields.io/badge/MSRV-1.94-orange.svg)](https://github.com/roomkit-live/roomkit-tools/blob/main/Cargo.toml)

RoomKit Tools gives an agent what it needs to work on code: read, search and
edit files, run commands and long-lived processes, fetch and search the web,
look at images, read documentation, keep a plan and delegate work to other
agents. Each tool is a separate component with a typed contract. You choose
the tools an agent gets, and your application decides what they can reach.

Every tool is implemented once, in Rust. Python calls the same implementation
through a native extension, and each tool can be passed directly to a
[RoomKit](https://github.com/roomkit-live/roomkit) agent. The tools also work on
their own, without RoomKit or a model.

> **Early development (alpha).** Preview wheels for Linux x86_64 are attached to
> [GitHub releases](https://github.com/roomkit-live/roomkit-tools/releases). The
> Rust library crates are on crates.io as pre-releases; the Python packages are
> not on PyPI yet. The APIs will change before 1.0. Feedback and contributions
> are welcome.

## Features

- **Select exactly what an agent gets.** Each tool is chosen individually.
  There is no global registry and no tool server: an agent given `read` and
  `grep` can do nothing else.
- **The host sets the limits, the model fills in the arguments.** Workspace
  roots, byte and line limits, deadlines, allowed origins, credentials and
  lifetimes are host configuration. The model sees only each tool's input
  schema.
- **Bounded results.** Reads, searches, command output and downloads all have
  explicit limits. Results report truncation and pagination in structured
  fields, so a partial result is never mistaken for a complete one.
- **Edits that detect conflicts.** Reads return a content revision. Writes,
  edits and patches can require it and refuse to overwrite a file that changed
  in the meantime. A shared `Workspace` serializes mutations from several tools.
- **One implementation, two languages.** Input and output schemas are derived
  from the Rust types the tools use. Python returns the same `data` and `text`
  envelope, checked against Rust with shared fixtures.
- **Typed failures and cancellation.** Invalid input, refused access, exceeded
  limits and unsupported content are distinct errors. Cancelling an asyncio
  task cancels the native call, and stops the processes a command started.
- **Process control.** Commands run in their own session, detached from the
  host's terminal, with a deadline, bounded capture of the latest output and only the
  environment variables the host chooses. Persistent processes keep their stdin
  and stdout across calls, and long output can be archived for later reading.
- **Guarded web access.** `web_fetch` refuses private and loopback addresses
  unless the host allows that exact origin, checks every redirect again and
  connects to the address it validated. `web_search` uses an Exa, Ollama or
  OpenAI backend chosen by the host.

## Available tools

| Capability | Model-facing tools | Guide |
| --- | --- | --- |
| Files | `read`, `list`, `glob`, `grep`, `write`, `edit`, `diff`, `patch` | [Filesystem](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/filesystem.md) |
| Code structure | `code_outline` for Rust and Python | [Code](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/code.md) |
| Commands | `shell` | [Processes](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/processes.md) |
| Persistent processes | `process_start`, `process_status`, `process_read`, `process_write`, `process_stop` | [Processes](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/processes.md) |
| Output archives | `output_read`, `output_forget` | [Processes](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/processes.md#optional-command-output-retention) |
| Web | `web_fetch`, `web_search` | [Web](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/web.md) |
| Images | `view_image` for PNG, JPEG and WebP | [Images](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/images.md) |
| Text recognition | `ocr`, with a separately installed local engine | [OCR](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/ocr.md) |
| Documentation | `docs_list`, `docs_search`, `docs_read` | [Documentation](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/documentation.md) |
| Planning | `plan_create`, `plan_read`, `plan_list`, `plan_update`, `plan_delete` | [Plans](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/plans.md) |
| Delegation | `delegate_task`, `task_status`, `cancel_task`, `agent_followup`, `agent_send` | [Agents](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/agents.md) |

Every tool is available in Python and in RoomKit Python. The optional RoomKit
Rust adapter covers the file, code, command, process, web, image, OCR, plan and
documentation tools; agent delegation is not available there yet.

## Quick start

You need CPython 3.12 or later on Linux x86_64 (glibc 2.34+) to install a
preview wheel:

```sh
python3 -m venv .venv
source .venv/bin/activate
python -m pip install "roomkit-tools[roomkit] @ https://github.com/roomkit-live/roomkit-tools/releases/download/v0.1.0a16/roomkit_tools-0.1.0a16-cp312-abi3-manylinux_2_34_x86_64.whl"
```

Omit `[roomkit]` to use the tools without RoomKit: the base package has no
Python dependencies. On other platforms, or to work on the project, build from
a checkout; this needs [Rust](https://rustup.rs) to compile the extension:

```sh
git clone https://github.com/roomkit-live/roomkit-tools
cd roomkit-tools
python3 -m venv .venv
source .venv/bin/activate
python -m pip install '.[roomkit]'
python examples/roomkit_read.py
```

`roomkit_read.py` runs a real RoomKit conversation in which a scripted model
calls the Rust `read` tool. It needs no API key and no model server. The
[examples index](https://github.com/roomkit-live/roomkit-tools/blob/main/examples/README.md)
lists the other scenarios, including a coding task, background processes and
agent delegation, and how to run them against your own model.

## Use it with RoomKit

Pass the tools you choose to an AI channel:

```python
from roomkit import AIChannel
from roomkit_tools import (
    Edit, EditTool, Grep, GrepTool, Read, ReadTool, Workspace, Write, WriteTool,
)

workspace = Workspace("./project")  # An existing directory.
agent = AIChannel(
    "assistant",
    provider=provider,  # Your configured RoomKit AI provider.
    tools=[
        Read(await ReadTool.create(workspace)),
        Grep(await GrepTool.create(workspace)),
        Write(await WriteTool.create(workspace)),
        Edit(await EditTool.create(workspace)),
    ],
)
```

The agent gets these four tools and nothing else. Each RoomKit adapter
(`Read`, `Grep`, ...) takes a built tool, as in Rust. Each tool checks its
arguments against its Rust input type; RoomKit runs the calls inside its turns,
hooks and policies. Building `Write` and `Edit` from the same `Workspace` makes
their changes to the project happen one at a time.

## Use it without RoomKit

Each tool has a standalone asynchronous counterpart (`ReadTool`, `GrepTool`,
`ShellTool`, ...) that returns the result as a dictionary:

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
        print(result["data"]["revision"])  # Pass it to a later write or edit.


asyncio.run(main())
```

`result["data"]` holds the structured result and `result["text"]` the text
presented to a model. Errors derive from `ToolError` (`InvalidInputError`,
`AccessRefusedError`, `LimitExceededError`, `UnsupportedResourceError`,
`ToolIOError`), and cancelling the task raises `asyncio.CancelledError`. The
[Python guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/python.md)
covers schemas, errors and resource ownership.

## Use it from Rust

The library crates are on crates.io as pre-releases. Depend on the ones you
need with their exact version: a plain `"0.1"` requirement does not match a
pre-release.

```toml
[dependencies]
roomkit-tools-fs = "0.1.0-alpha.16"
tokio = { version = "1", features = ["macros", "rt"] }
tokio-util = "0.7"
```

```rust
use roomkit_tools_fs::{ReadInput, ReadTool};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let read = ReadTool::new(".").await?;
    let result = read
        .read(ReadInput::new("README.md"), &CancellationToken::new())
        .await?;
    print!("{}", result.text);
    Ok(())
}
```

`cargo run --example read_file` runs the self-checking version from a checkout.
The [Rust guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/rust.md)
describes each crate and the optional RoomKit Rust adapter. The adapter is not
on crates.io: take it through Git, pinned to a commit, with the tool crates it
wraps from the same commit so that your code and the adapter share their types:

```toml
[dependencies]
roomkit-tools-roomkit = { git = "https://github.com/roomkit-live/roomkit-tools", rev = "<commit>" }
roomkit-tools-fs = { git = "https://github.com/roomkit-live/roomkit-tools", rev = "<commit>" }
```

## How it works

```text
                 tool call (JSON arguments)
┌────────┐      ┌───────────────────────┐      ┌───────────────────────────┐
│ Model  │ ───► │ Host                  │ ───► │ roomkit-tools             │
│        │      │ RoomKit, or your own  │      │ validates the arguments,  │
│        │ ◄─── │ agent loop            │ ◄─── │ runs within host limits   │
└────────┘      └───────────────────────┘      └───────────────────────────┘
          text           │ configures: roots,       data + text │
                         │ limits, origins,                     ▼
                         │ credentials, lifetimes     files, processes, network
```

- **Tools hold no global state.** A tool is created with its host configuration
  and owns nothing else. Process sessions, output stores and agent backends are
  explicit objects the host creates, shares between tools and closes.
- **Results carry both forms.** `data` is for the host; `text` is for the model.
  Pagination, truncation and omitted bytes appear in `data`, never only in prose.
- **Errors are values the host can classify.** A refused path, a limit, an
  unsupported file and an I/O failure are different error types; a command that
  exits non-zero or times out is a normal result.
- **Cancellation is explicit.** Each Rust call takes a cancellation token, and
  Python's task cancellation reaches it. Cancelling stops a command's process
  group; it does not undo changes already written.

## Execution boundaries

These tools give an agent real capabilities. Choose them as you would choose
permissions:

- **Path checks are not a sandbox.** Filesystem tools resolve every path inside
  the host's workspace root, assuming that nothing else replaces paths in it
  concurrently. Run untrusted agents in a container or VM.
- **Commands run with the host's permissions.** `shell` and the process tools
  run as the host's user. They receive only a minimal set of environment
  variables (`PATH`, `HOME`, locale and a few others) plus what the host passes
  explicitly, but a working directory does not limit what a command can access.
- **Cancellation does not roll back.** A cancelled write or command may already
  have taken effect.
- **Retained state is in memory.** Process sessions, output archives, plans and
  agent tasks do not survive a host restart.
- **Fetched content is untrusted.** Web pages, search results, files and command
  output can contain instructions aimed at the model. Treat them as data.

## Repository layout

| Path | Contents |
| --- | --- |
| [`crates/`](https://github.com/roomkit-live/roomkit-tools/tree/main/crates) | One Rust crate per tool family, plus `roomkit-tools-core` for definitions, results and errors (no I/O, no async runtime) |
| [`python/roomkit_tools`](https://github.com/roomkit-live/roomkit-tools/tree/main/python/roomkit_tools) | The Python package and its RoomKit integration |
| [`bindings/python`](https://github.com/roomkit-live/roomkit-tools/tree/main/bindings/python) | The PyO3 native extension |
| [`adapters/roomkit`](https://github.com/roomkit-live/roomkit-tools/tree/main/adapters/roomkit) | Optional adapter for the RoomKit Rust engine |
| [`backends/rusto/python`](https://github.com/roomkit-live/roomkit-tools/tree/main/backends/rusto/python) | Optional local OCR engine, packaged separately |
| [`examples/`](https://github.com/roomkit-live/roomkit-tools/tree/main/examples) | Rust and Python examples that check their own results |
| [`guide/`](https://github.com/roomkit-live/roomkit-tools/tree/main/guide) | User documentation |

## Platform support

| Component | Validated |
| --- | --- |
| Python | CPython 3.12 and 3.14; the wheel targets the 3.12+ stable ABI |
| Preview wheels | Linux x86_64, glibc 2.34+ |
| Rust | 1.94 or later |
| RoomKit Python | `roomkit>=0.110,<0.112` |
| Commands and processes | Linux. Other Unix systems run the same code without validation; non-Unix systems return an unsupported error |

macOS, Windows, ARM64, musl, PyPy and free-threaded Python have no validated
builds yet. Source builds may work there, but are not supported.

## Development

You need the toolchain pinned in `rust-toolchain.toml` (rustup installs it on
first use), [`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny), and for
the Python checks CPython 3.12+ with [uv](https://docs.astral.sh/uv/).

```sh
make all            # formatting, Clippy, tests, docs, dependency policy, examples
make msrv           # the workspace on the minimum supported Rust version
make python-check   # wheel and source installs, Python tests, RoomKit adapter
```

The default checks need no network service and no credentials. Generate the
API documentation with `cargo doc --workspace --no-deps --open`.

## Stability

The Python package follows an alpha sequence (`0.1.0aN`), and the Rust library
crates a pre-release sequence (`0.1.0-alpha.N`) on crates.io. Until 1.0, any
preview may change the API, and the
[changelog](https://github.com/roomkit-live/roomkit-tools/blob/main/CHANGELOG.md)
says how.

## Roadmap

- Publication on PyPI.
- Wheels for more platforms (macOS, ARM64).
- Agent delegation in the RoomKit Rust adapter.

## Contributing

Contributions are welcome: bug reports, documentation, fixes and features.
Read [CONTRIBUTING.md](https://github.com/roomkit-live/roomkit-tools/blob/main/CONTRIBUTING.md)
for the design rules, the checks to run and the commit conventions. For a large
change, open an issue first so we can agree on the approach.

## Security

Please do not report security issues in public issues. Use GitHub's
[private vulnerability reporting](https://github.com/roomkit-live/roomkit-tools/security/advisories/new)
instead; see [SECURITY.md](https://github.com/roomkit-live/roomkit-tools/blob/main/SECURITY.md).

## License

RoomKit Tools is licensed under the
[MIT License](https://github.com/roomkit-live/roomkit-tools/blob/main/LICENSE).
The Python package bundles the license notices of its native dependencies.
