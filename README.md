# roomkit-tools

Individually selectable tools for coding agents, implemented in Rust and
intended for use from Rust, Python and RoomKit.

**Status: filesystem, shell and persistent process tools in Rust and RoomKit.** The workspace provides
`read`, `list`, `glob`, `grep`, `write`, `edit`, `diff`, `patch`, `shell`, `process_start`, `process_status`, `process_read`,
`process_write` and `process_stop`, each independently selectable.
Optional RoomKit Rust adapters expose each tool through the `Toolset` interface.
Python exposes `read`, `list`, `glob`, `grep`, `write` and `edit`, directly selectable in RoomKit.
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
temporary directory, including grep context. `edit_orders` creates, adds, changes
and removes customer orders, carrying a content revision between mutations.
`shell_command` reads two temporary files through a
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
| `roomkit-tools-python` | Optional CPython extension in `bindings/python`, with an `asyncio` API in `python/roomkit_tools`. Exposes `read`, `list`, `glob`, `grep`, `write` and `edit`. |

There is no global tool registry. A host instantiates the tools it needs
and chooses which definitions to expose to its model.

`ReadTool` returns complete lines, one-based numbers and a continuation
offset. Defaults are 2,000 lines, 50 KiB of rendered output and 8 MiB of
file input. It rejects non-text files, files above the ingestion bound,
and an individual line that cannot fit in a page. It scans in 64 KiB chunks,
retaining only the page and a bounded current line. Raise
`ReadLimits::max_file_bytes` explicitly for larger files; the default remains
8 MiB. The entire file is still scanned to validate UTF-8/NUL and compute a
`revision`, including when only a small page was requested. Cancellation can
interrupt the scan between chunks. The host owns the root and limits.

`list` returns a directory's immediate entries and types. `glob` searches paths
recursively; `grep` searches matching text lines with an optional file glob.
Results are sorted and paginated. Hidden entries are included, ignore files
are not applied, and recursive scans do not follow child symlinks. Traversal,
ingestion and output have explicit bounds; exceeding a scan bound is an error.
`grep` accepts optional `context_before` and `context_after` line counts (zero
by default, at most `GrepLimits::max_context_lines`, default 20 per side). Each
match carries its own numbered context; overlapping windows may repeat lines.
Offsets count matching lines only. A match and its context fit together in a
page or are deferred together; an oversized single group is an explicit error.
In text output, matches use `path:line:` and context uses `path-line-`.

`write` creates complete UTF-8 files and requires `overwrite: true` to replace
existing text. `edit` replaces exact text, refusing zero or multiple matches
unless `replace_all: true` permits multiple matches. Both cap files at 8 MiB
by default. Mutations stage a sibling temporary file before committing; they
require existing parent directories and refuse leaf symlinks. Share one
`MutationWorkspace` across selected writers/editors to serialize their work.
Cancellation before commit leaves the destination unchanged; a commit that has
already started may finish. Root and snapshot checks do not prevent hostile
concurrent path replacement or guarantee coordination with external writers.

`read`, `write` and `edit` return a content `revision` (`sha256:` plus the digest
of all raw file bytes, including BOM and line endings). Pass it as
`expected_revision` to `write` or `edit` to refuse a stale mutation, even when
the target passage still matches or the file's timestamp did not change.
The check runs under the shared mutation lock. Overwriting still requires
`overwrite: true`; a revision also requires the destination to exist. Omitting
it preserves the original behavior. Successful mutations return the resulting
revision, so hosts can carry it forward without hidden per-agent read state.
This is optimistic content checking, not a filesystem compare-and-swap or a
snapshot guarantee during external concurrent writes. `patch` already uses
exact expected source contents and retains that contract.

```rust,ignore
let page = read.read(ReadInput::new("notes.txt"), &cancel).await?;
let mut input = EditInput::new("notes.txt", "count=1", "count=2");
input.expected_revision = Some(page.data.revision);
let result = edit.edit(input, &cancel).await?;
```

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

The `0.1.0a3` preview exposes **read, list, glob, grep, write and edit**. Its prebuilt wheel supports ordinary
CPython 3.12+ on Linux x86_64 with glibc 2.34 or newer, tested on Python 3.12 and
3.14. Download it from the [GitHub release](https://github.com/roomkit-live/roomkit-tools/releases/tag/v0.1.0a3)
using an authenticated GitHub CLI, then install it without a Rust compiler:

```sh
gh release download v0.1.0a3 --repo roomkit-live/roomkit-tools \
  --pattern '*.whl' --dir dist
python3 -m venv .venv
.venv/bin/python -m pip install \
  './dist/roomkit_tools-0.1.0a3-cp312-abi3-manylinux_2_34_x86_64.whl[roomkit]'
```

The `[roomkit]` extra installs the supported RoomKit dependency. Omit it for
standalone use. The repository and release are currently private; this version
is not on PyPI.

Select the tools directly in an existing RoomKit agent:

```python
from roomkit import AIChannel
from roomkit_tools import Edit, Glob, Grep, List, Read, Workspace, Write

workspace = Workspace("./workspace")
agent = AIChannel(
    "agent",
    provider=provider,  # Your configured RoomKit AI provider.
    tools=[
        Read(root=workspace), List(root=workspace), Glob(root=workspace),
        Grep(root=workspace), Write(root=workspace), Edit(root=workspace),
    ],
)
```

Each class includes its RoomKit adapter. Construction captures the absolute root;
the first call resolves the directory and validates limits asynchronously in
Rust. The directory must exist by then. Failed or cancelled initialization can
be retried. Each tool is selected explicitly; no other tools are registered.
RoomKit retains schema validation, permissions, hooks and turn cancellation.
No RoomKit Rust checkout or separate server is needed.

`Workspace` is an explicit shared root and mutation scope. Passing the same
object to `Write` and `Edit` serializes their read/prepare/commit operations,
including concurrent first calls and reuse across event loops. Read-only tools
accept it as a path and do not acquire that lock. A plain path also works, but
each mutation tool then owns an independent scope; equal path strings do not
share a global lock. External editors and separately created workspaces are not
coordinated by this object.

For direct Python use without RoomKit:

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

The extension calls the existing Rust tools; Python does not reimplement
file access or schemas. `call(arguments)` accepts the same JSON-shaped input as
Rust. Results are dictionaries containing both `data` and `text`. Cancelling an
`asyncio` task cancels its Rust token and future; already-issued filesystem I/O
may still finish in Tokio's blocking pool. Mutations also retain the Rust commit
boundary described below.

The standalone discovery tools use the same construction and result shape:

```python
from roomkit_tools import GlobTool, GrepTool, ListTool

# Inside an async function:
listing = await ListTool.create("./workspace", max_entries=100)
glob = await GlobTool.create("./workspace")
grep = await GrepTool.create("./workspace", max_context_lines=5)

directories = await listing.list()
paths = await glob.glob("**/*.txt")
matches = await grep.grep("blue", include="**/*.txt", context_before=2)
```

`list`, `glob` and `grep` accept a one-based `offset` and optional `limit`;
resume with `data.next_offset`. Grep offsets count matching lines, not context.
Paths in discovered entries and matches are workspace-relative and usable by
`read`. Every page rescans; results are not snapshots across external edits.
Hidden entries are included, ignore files are not applied, and child symlinks
are not traversed. Grep supports Rust regex syntax, `literal=True`,
`ignore_case=True`, `include`, `context_before` and `context_after`.
It skips binary/non-UTF-8 files and reports `data.skipped_files`.

Host bounds are explicit constructor keywords for both APIs:

| Tool | Host bounds and defaults |
| --- | --- |
| `List` / `ListTool` | `max_entries=200`, `max_output_bytes=51200`, `max_scan_entries=10000` |
| `Glob` / `GlobTool` | `max_matches=200`, `max_output_bytes=51200`, `max_scan_entries=10000`, `max_depth=32` |
| `Grep` / `GrepTool` | Same glob bounds, plus `max_file_bytes=8388608`, `max_total_bytes=33554432`, `max_context_lines=20` per side |

Omitted bounds retain Rust defaults. Bounds must be positive, except
`max_context_lines=0` disables context. Traversal or ingestion limits fail the
call; output pagination is reported explicitly. Run the standalone
[discovery example](examples/discover_files.py) with
`.venv/bin/python examples/discover_files.py`; it needs no RoomKit or model.

For standalone mutations, share one workspace and pass the latest revision:

```python
from roomkit_tools import EditTool, ReadTool, Workspace, WriteTool

# Inside an async function:
workspace = Workspace("./workspace")
read = await ReadTool.create(workspace)
write = await WriteTool.create(workspace)
edit = await EditTool.create(workspace)

created = await write.write("notes.txt", "Status: draft\n")
before = await read.read("notes.txt")
updated = await edit.edit(
    "notes.txt", "draft", "ready", expected_revision=before["data"]["revision"]
)
```

`write(path, content, overwrite=False, expected_revision=None)` writes complete
UTF-8 content. Replacing an existing file requires `overwrite=True`, including
when a matching revision is supplied. `edit(path, old_text, new_text,
replace_all=False, expected_revision=None)` replaces an exact nonempty passage.
Multiple matches require `replace_all=True`; no matches are refused. Empty
`new_text` removes the passage and keeps the file. Both preserve raw line endings
and BOM, refuse leaf symlinks and NUL/binary content, and require existing parent
directories. No directories are created implicitly.

Both constructors accept `max_file_bytes` (8 MiB by default), bounding source,
arguments and result as applicable. Results contain changed/created flags or
replacement counts, byte counts and the resulting content revision. The optional
`expected_revision` checks the destination inside the shared mutation lock before
writing; a stale revision is refused even when the selected text still matches.
It requires an existing destination, so omit it for creation. This is not an OS
compare-and-swap against external editors. Read before editing or overwriting,
then use the revision returned by read/write/edit.

Cancellation requests stopping before commit. A commit that has already started
may finish even if Python receives `CancelledError` or RoomKit times out; the
Rust worker keeps the shared lock until completion. Read the file before retrying
an interrupted mutation. See the standalone [mutation example](examples/edit_file.py),
run with `.venv/bin/python examples/edit_file.py`, for a complete flow including
stale-revision refusal.

Errors inherit from `ToolError`: `InvalidInputError`, `AccessRefusedError`,
`LimitExceededError`, `UnsupportedResourceError` and `ToolIOError`. I/O messages
omit OS diagnostics and host paths. Cancellation remains `asyncio.CancelledError`.
Async construction keeps root resolution off the Python event loop. Each call
has its own cancellation scope, and tools can be reused across event loops.

The RoomKit classes expose the Rust-derived input schema through its `Tool` protocol
and return both result representations. Refusal and resource failure stay
distinct. The lower-level `roomkit_tools.roomkit.ReadToolAdapter` remains
available for an already-created `ReadTool`. Importing `roomkit_tools` and
configuring these tools does not import or require RoomKit; calling their
RoomKit handlers does.

Run the complete [RoomKit read example](examples/roomkit_read.py) from a checkout
with the wheel installed:

```sh
.venv/bin/python examples/roomkit_read.py
```

It creates a temporary `notes.txt`, sends a message through a RoomKit room,
prints the actual tool result and the assistant reply, and cleans up afterwards.
The default model is scripted, so no server or credentials are needed; the file
is read by Rust. This mode is checked by `make python-check` and CI.

For a running vLLM server with tool calling configured:

```sh
.venv/bin/python -m pip install 'roomkit[vllm]>=0.110,<0.112'
export OPENAI_BASE_URL=http://localhost:8000/v1
export OPENAI_MODEL=your-served-model
# Set OPENAI_API_KEY too if the server requires authentication.
.venv/bin/python examples/roomkit_read.py --live
```

Live mode uses RoomKit's vLLM provider with the environment settings above.
Only the model connection changes; the registered Rust tool stays the same.
It runs once in the console; it does not start an HTTP server or a chat page.

The native binding is a separate Cargo workspace and uses the stable Python ABI
from 3.12. The existing read binding uses the same chunked Rust scan and returns
its revision. Diff/patch, process tools, other platforms and free-threaded builds remain outside
the validated Python slice.

To build from this checkout instead, install Rust and [uv](https://docs.astral.sh/uv/):

```sh
uv venv
uv pip install '.[roomkit]'
```

## License

MIT. See [LICENSE](LICENSE).
