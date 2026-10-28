# roomkit-tools

Individually selectable tools for coding agents, implemented in Rust and
intended for use from Rust, Python and RoomKit.

**Status: filesystem, shell and persistent process tools in Rust and RoomKit.** The workspace provides
`read`, `list`, `glob`, `grep`, `write`, `edit`, `diff`, `patch`, `shell`, `process_start`, `process_status`, `process_read`,
`process_write` and `process_stop`, each independently selectable.
Optional RoomKit Rust adapters expose each tool through the `Toolset` interface.
Python currently exposes `read`, with an optional RoomKit Python adapter.
Bindings for the other tools and large-file streaming are planned.
Nothing is published to crates.io or PyPI yet.

## Start here

The layout and development gates follow `roomkit-rs`: a Cargo workspace,
central dependency versions and lints, a pinned toolchain, a separate
minimum supported Rust version, checked examples and dependency policy.

```sh
cargo run --example read_file
cargo run --example read_file -- README.md
cargo run --example discover_files
cargo run --example edit_orders
cargo run --example shell_command
cargo run --example patch_hours
cargo run --example process_session
make fmt-check clippy test doc check-core-deps examples
```

Without arguments, `read_file` uses a temporary directory and checks its result.
With a path, it reads that file beneath the current directory and prints the
first bounded page. `discover_files` checks `list → glob → grep → read` in a
temporary directory. `edit_orders` creates, adds, changes and removes customer
orders in a temporary file. `shell_command` reads two temporary files through a
real shell and checks both output streams. `patch_hours` compares a proposal,
previews and applies a batch, then verifies equality. `process_session` sends an
order through persistent stdin, reads the response and releases the job. No example needs a model
or RoomKit installation.

The toolchain is pinned in `rust-toolchain.toml`; the MSRV is Rust 1.92.
Once `cargo-deny` is installed, `make all` runs all local gates. See
[CONTRIBUTING.md](CONTRIBUTING.md) for setup and checks.

## What exists

| Crate | Purpose |
| --- | --- |
| `roomkit-tools-core` | Definitions with derived JSON schemas, structured output and typed errors; no I/O or async runtime. |
| `roomkit-tools-fs` | `ReadTool`, `ListTool`, `GlobTool`, `GrepTool`, `WriteTool`, `EditTool`, `DiffTool`, `PatchTool`: bounded operations beneath an explicit root. |
| `roomkit-tools-process` | `ShellTool` for foreground commands; `ProcessManager` for owned persistent jobs, paged output, stdin and cleanup. |
| `roomkit-tools-examples` | Standalone examples that verify what they show. |
| `roomkit-tools-roomkit` | Optional adapters in `adapters/roomkit`: `ReadToolset`, `ListToolset`, `GlobToolset`, `GrepToolset`, `WriteToolset`, `EditToolset`, `DiffToolset`, `PatchToolset`, `ShellToolset` and the five `Process*Toolset` adapters, registered individually. |
| `roomkit-tools-python` | Optional CPython extension in `bindings/python`, with an `asyncio` API in `python/roomkit_tools`. Currently exposes `read`. |

There is no global tool registry. A host instantiates the tools it needs
and chooses which definitions to expose to its model.

`ReadTool` returns complete lines, one-based numbers and a continuation
offset. Defaults are 2,000 lines, 50 KiB of rendered output and 8 MiB of
file input. It rejects non-text files, files above the ingestion bound,
and an individual line that cannot fit in a page. The implementation
currently reads the bounded file before paging it; streaming large-file
reads are planned. The host owns the root and limits.

`list` returns a directory's immediate entries and types. `glob` searches paths
recursively; `grep` searches matching text lines with an optional file glob.
Results are sorted and paginated. Hidden entries are included, ignore files
are not applied, and recursive scans do not follow child symlinks. Traversal,
ingestion and output have explicit bounds; exceeding a scan bound is an error.

`write` creates complete UTF-8 files and requires `overwrite: true` to replace
existing text. `edit` replaces exact text, refusing zero or multiple matches
unless `replace_all: true` permits multiple matches. Both cap files at 8 MiB
by default. Mutations stage a sibling temporary file before committing; they
require existing parent directories and refuse leaf symlinks. Share one
`MutationWorkspace` across selected writers/editors to serialize their work.
Cancellation before commit leaves the destination unchanged; a commit that has
already started may finish. Root and snapshot checks do not prevent hostile
concurrent path replacement or guarantee coordination with external writers.

`shell` uses `/bin/sh -c`, closed stdin and the host's environment and permissions.
It retains 32 KiB per output stream by default, preserving the beginning and end.
The default runtime is 10 seconds, with a host cap of 120 seconds. Cancellation,
timeout and normal shell exit terminate remaining jobs in its Unix process group.
The initial directory check is **not shell confinement**: commands can access
other host paths. Cleanup targets the Unix process group; processes that leave
that group are outside its guarantees. Keep the Tokio runtime alive for cleanup.

`diff` compares two existing text files without writing, returning complete hunks
and a unified-style preview. `patch` prepares or applies structured JSON batches
of add/update/delete/move operations. Preview is the default; application requires
`dry_run: false` and exact complete expected source text. It reports committed
effects on partial failure and shares a mutation scope with write/edit. It does
not parse Git-style text patches or provide multi-file rollback. File parents
must exist; moves create the destination before removing the source.

The five `process_*` tools manage jobs across calls with trusted host owner keys.
RoomKit scopes them to the conversation and agent. Output has byte cursors and
explicit retention gaps; stop can also release the record. Output is retained in
bounded memory, with no durable full-output archive. Reading does not consume
output or close stdin. Send EOF explicitly through `process_write` when needed.

Linux is the initial validation platform. Root canonicalization is an
access check for a host-controlled workspace, not an OS sandbox.
Sandbox execution is outside this workspace.

## API documentation and integration

Run `cargo doc --workspace --no-deps --open` for tool inputs, outputs and limits.
Schemas are derived from the Rust types used by each implementation. The core
depends only on Serde, serde_json, Schemars and thiserror; filesystem and process
I/O and the Tokio runtime belong to the implementation crates.

The optional `adapters/roomkit` workspace currently uses local path dependencies
on a sibling `roomkit-rs` checkout. The standalone tools do not require it. Hosts
choose which adapters to register and supply their roots, limits and lifecycle.
RoomKit retains argument validation, policies, hooks and turn cancellation.

## Python

The first binding supports CPython 3.12+ and is validated on Linux. Install from
this checkout with Rust and [uv](https://docs.astral.sh/uv/) available:

```sh
uv venv
uv pip install .
```

```python
import asyncio
from roomkit_tools import ReadTool

async def main():
    tool = await ReadTool.create(".", max_lines=20)
    result = await tool.read("README.md", limit=5)
    print(result["text"])
    print(result["data"]["next_offset"])
    print(tool.definition()["input_schema"])

asyncio.run(main())
```

The extension calls the existing Rust `ReadTool`; Python does not reimplement
file access or schemas. `call(arguments)` accepts the same JSON-shaped input as
Rust. Results are dictionaries containing both `data` and `text`. Cancelling an
`asyncio` task cancels its Rust token and future; already-issued filesystem I/O
may still finish in Tokio's blocking pool. The tool performs no writes.

Errors inherit from `ToolError`: `InvalidInputError`, `AccessRefusedError`,
`LimitExceededError`, `UnsupportedResourceError` and `ToolIOError`. I/O messages
omit OS diagnostics and host paths. Cancellation remains `asyncio.CancelledError`.
Async construction keeps root resolution off the Python event loop. Each call
has its own cancellation scope, and tools can be reused across event loops.

To opt into RoomKit Python, install `uv pip install '.[roomkit]'` and register
`ReadToolAdapter(await ReadTool.create(root))` from `roomkit_tools.roomkit` in an
`AIChannel`'s `tools` list. The adapter exposes the derived input schema, returns
both result representations, and distinguishes refusal from resource failure.
RoomKit retains validation, permissions, hooks and turn cancellation. Importing
the standalone package does not import or require RoomKit.

The native binding is a separate Cargo workspace and uses the stable Python ABI
from 3.12. Other tools, platforms, free-threaded builds and streaming large-file
reads remain outside the validated Python slice. Nothing is published to PyPI.

## License

MIT. See [LICENSE](LICENSE).
