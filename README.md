# roomkit-tools

Individually selectable tools for coding agents, implemented in Rust and
intended for use from Rust, Python and RoomKit.

**Status: filesystem, process, output archive, web fetch and search tools in Rust and RoomKit.** The workspace provides
`read`, `list`, `glob`, `grep`, `write`, `edit`, `diff`, `patch`, `shell`, `process_start`, `process_status`, `process_read`,
`process_write`, `process_stop`, `output_read`, `output_forget`, `web_fetch` and `web_search`, each independently selectable.
Optional RoomKit Rust adapters expose each tool through the `Toolset` interface.
Python exposes the same eighteen tools, directly selectable in RoomKit.
Shared contracts and lifecycle regressions cover the implemented capabilities.
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
cargo run --example read_output
cargo run --example fetch_page
cargo run --example search_web
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
order through persistent stdin, reads the response and releases the job.
`read_output` retrieves a diagnostic from the middle of an archived log without
rerunning the command. `fetch_page` follows a local HTTP redirect and checks
the Markdown result. `search_web` checks a local search response; `--live` explicitly
uses Exa instead. These Rust examples need no model or RoomKit installation.

The toolchain is pinned in `rust-toolchain.toml`; the MSRV is Rust 1.92.
Once `cargo-deny` is installed, `make all` runs all local gates. See
[CONTRIBUTING.md](CONTRIBUTING.md) for setup and checks.

## What exists

| Crate | Purpose |
| --- | --- |
| `roomkit-tools-core` | Definitions with derived JSON schemas, structured output and typed errors; no I/O or async runtime. |
| `roomkit-tools-fs` | `ReadTool`, `ListTool`, `GlobTool`, `GrepTool`, `WriteTool`, `EditTool`, `DiffTool`, `PatchTool`: bounded operations beneath an explicit root. |
| `roomkit-tools-process` | `ShellTool` for foreground commands; `ProcessManager` for owned persistent jobs, paged output, stdin and cleanup; optional `OutputStore` for disk copies, reads and release. |
| `roomkit-tools-web` | Independent `WebFetchTool`: HTTP(S) GET, host origin policy, Markdown/text/source conversion, deadlines and byte limits. |
| `roomkit-tools-http` | Shared DNS-pinned HTTP clients, exact-origin policy and bounded body reads. |
| `roomkit-tools-search` | `WebSearchTool` and an open `SearchBackend` trait; optional `exa`, `ollama` and `openai` features, all off by default. |
| `roomkit-tools-examples` | Standalone examples that verify what they show. |
| `roomkit-tools-roomkit` | Optional adapters in `adapters/roomkit`: `ReadToolset`, `ListToolset`, `GlobToolset`, `GrepToolset`, `WriteToolset`, `EditToolset`, `DiffToolset`, `PatchToolset`, `ShellToolset` the five `Process*Toolset` adapters, `OutputReadToolset`, `OutputForgetToolset` `WebFetchToolset` and `WebSearchToolset`, registered individually. |
| `roomkit-tools-python` | Optional CPython extension in `bindings/python`, with an `asyncio` API in `python/roomkit_tools`. Exposes the eight filesystem tools, shell, five persistent-process tools two output-archive tools, web fetch and search. |

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
The Rust RoomKit adapters scope them to the conversation and agent; Python hosts
provide a separate `ProcessSession` per conversation/agent. Output has byte cursors and
explicit retention gaps; stop can also release the record. Output is retained in
bounded memory by default. An optional `OutputStore` also keeps a bounded raw
copy on disk, readable through `output_read` and released through `output_forget`.
It is temporary storage, without persistence across restarts. Reading does not consume
output or close stdin. Send EOF explicitly through `process_write` when needed.

Linux is the initial validation platform. Root canonicalization is an
access check for a host-controlled workspace, not an OS sandbox.
Sandbox execution is outside this workspace.

## Web fetching

`web_fetch` makes HTTP(S) GET requests and returns Markdown by default, visible
text with `format="text"`, or the source with `format="html"`. HTML conversion
preserves code and resolves links against the final URL. Script/style/hidden
subtrees are omitted from Markdown/text. There is no JavaScript execution,
browser session or web search. Non-HTML text is preserved in every format.
This first version supports UTF-8/ASCII textual MIME types and gzip. Other
encodings and binary resources produce an explicit unsupported-content error.

```python
from roomkit_tools import WebFetch

# Add just this capability to an existing RoomKit agent.
agent = AIChannel("agent", provider=provider, tools=[WebFetch()])

# Standalone async Python, with the same native implementation:
from roomkit_tools import WebFetchTool
result = await WebFetchTool().fetch("https://example.com", format="markdown")
print(result["data"]["final_url"], result["data"]["http_ok"])
print(result["text"])
```

The host can restrict `allowed_origins` to exact scheme/host/effective-port
origins. `None` allows public destinations; `[]` denies all. Private, loopback,
link-local and reserved addresses are refused unless the host explicitly adds
the exact origin to `private_origins`; this never bypasses `allowed_origins`.
Each redirect repeats origin and DNS checks. Validated IP addresses are pinned
for the connection while TLS checks the original hostname. Ambient proxies,
cookies and URL credentials are disabled. No custom authentication/header API
is provided. These controls apply to this tool, not other selected capabilities.

Default bounds: 8 KiB URL, 5 MiB decoded body, 5 MiB converted text, 50 KiB
returned content, 10 redirects, 30-second total deadline (host maximum 120
seconds). The deadline includes DNS, redirects, downloads and conversion.
Hosts configure these through `WebFetchLimits` or Python constructor keywords.
An oversized download is refused, including streaming/decompressed responses;
a preview is clipped only after successful bounded ingestion and conversion.
`response_bytes`, `content_bytes`, `omitted_bytes` and `truncated` distinguish
these stages. A clipped preview has no archive or continuation cursor.
`status`, `http_ok` and `final_url` describe the actual final HTTP response;
non-2xx responses remain data if their body is supported. Transport failures,
refused destinations, limits and unsupported content are typed tool errors.

HTML conversion runs outside the async executor with two slots per tool (shared
by clones). It checks at most 100,000 visited nodes, 128 levels, expanded link
bytes and converted bytes. The HTML parser builds a DOM and the final Markdown
size is checked after conversion: these are work/output bounds, not strict peak
memory limits. Cancellation drops network work; issued DNS and blocking HTML
work may finish later. Fetched content remains untrusted reference data.

Run `python examples/roomkit_web.py` for a self-checking local demonstration,
or add `--live` with `OPENAI_BASE_URL` and `OPENAI_MODEL` for vLLM. The local
server supplies sample HTML only; HTTP fetching and conversion execute in Rust.

## API documentation and integration

Run `cargo doc --workspace --no-deps --open` for tool inputs, outputs and limits.
Schemas are derived from the Rust types used by each implementation. The core
depends only on Serde, serde_json, Schemars and thiserror; filesystem, process and network
I/O and the Tokio runtime belong to the implementation crates.

The optional `adapters/roomkit` workspace pins an immutable Git revision of
`roomkit-rs`; it does not require a sibling checkout. Fetching the private engine
currently requires Git read access. The standalone tools do not require it. Hosts
choose which adapters to register and supply their roots, limits and lifecycle.
RoomKit retains argument validation, policies, hooks and turn cancellation.

## Modular web search

Choose a search backend independently of your agent model. The Rust tool is
`WebSearchTool::new(Arc<dyn SearchBackend>)`. Implement `SearchBackend` in another
crate to add a service; there is no closed provider registry. Built-in backends
are enabled with Cargo features `exa`, `ollama` and/or `openai`. The RoomKit Rust adapter
accepts the generic tool without enabling a provider feature itself.

For RoomKit Python:

```python
from roomkit import AIChannel
from roomkit_tools import WebSearch, WebFetch
from roomkit_tools.search import ExaSearch, OllamaSearch

agent = AIChannel(
    "agent",
    provider=vllm_provider,  # Your existing model/provider.
    tools=[WebSearch(backend=ExaSearch()), WebFetch()],
)
# Or select the hosted Ollama search API with an explicit account key:
# tools=[WebSearch(backend=OllamaSearch(api_key=search_key))]
```

`ExaSearch` uses [Exa's hosted MCP endpoint](https://exa.ai/mcp) with optional `api_key`; anonymous
access is subject to service limits. `OllamaSearch` uses the [hosted Ollama REST
API](https://docs.ollama.com/capabilities/web-search) and requires an explicit account key. A local Ollama installation does not
provide this web index. No backend reads keys from environment variables.
No search server or second local model needs to be started.

Without RoomKit, use `WebSearchTool(backend=...)` and
`await tool.search("query", limit=5, timeout_ms=30000)`. It returns the same
`data`/`text` envelope as Rust. `backend.capabilities` exposes supported result
kinds and service limits. A new native backend needs a binding factory to be
available in Python; there is no Python HTTP fallback.

Input is only `query`, `limit` and `timeout_ms`. The host selects credentials,
endpoint, origin policy and budgets. Exa and Ollama return
`data.payload.kind == "results"` with ranked URL/title/excerpt entries.
`partial`, `omitted_results` and per-excerpt `omitted_bytes` expose known
omissions. Zero results is a valid response, distinct from HTTP/quota failures,
MCP errors and unsupported formats. Exa uses a bounded parser for its current
text blocks over JSON or SSE; an upstream format change can require an adapter
update. This is not a general MCP client.

The contract also supports `answer` with sources and UTF-8 citation offsets,
and `not_performed` when a model did not search. Common validation checks
source indexes and character boundaries; oversized answers fail without
clipping citations. Optional `usage` contains provider-reported counters only.
`OpenAISearch` implements these shapes through a separate
[Responses web-search request](https://developers.openai.com/api/docs/guides/tools-web-search):

```python
from roomkit_tools import WebSearch
from roomkit_tools.search import OpenAISearch

tools = [WebSearch(backend=OpenAISearch(
    api_key=search_key,
    model=search_model,          # Explicit model supporting Responses web search.
    max_tool_calls=1,
    max_output_tokens=2048,
))]
```

The model performs a required web search and generates an answer. The adapter
only returns `answer` after observing a completed `search` action. A completed
response without one becomes `not_performed`; it does not forward an unverified
answer as web evidence. Failed/unfinished tool calls and malformed payloads
raise errors. Incomplete responses with a completed search and usable text are
marked `partial`; missing URL citations are also reported explicitly.

One synchronous request uses `store=false`, no history and no streaming,
background execution, retry or continuation. `max_tool_calls` (1–16) and
`max_output_tokens` (1–1,000,000) are request ceilings, not monetary budgets.
A compatible model and funded API access are the host's responsibility; the
library never selects a model or reads environment variables. Local cancellation
stops waiting but does not guarantee remote computation or billing has stopped.
No OpenAI SDK dependency is required.

URL annotations use Unicode character indexes, converted to UTF-8 byte offsets
before concatenating text parts; `data.payload.answer` keeps the original text.
The `text` rendition inserts visible Markdown citation links. Hosts displaying
answers should preserve clickable citations. `limit` bounds retained sources:
all cited sources are kept, uncited extras are omitted with a count, and a limit
below the number of cited sources fails explicitly. Citation source indexes are
remapped when uncited sources are omitted. No generated excerpt is invented.
`usage.search_requests` counts observed search actions, while token counters are
copied from the provider when present; absent token usage remains unknown.

**Anthropic, Gemini and SearXNG backends are not implemented yet.** Search inside
the main AI generation remains a separate RoomKit provider integration. OpenAI
protocol handling is covered by local response fixtures; these automated tests
do not establish account/model availability or replace a live API validation.

Defaults: 4096 query bytes, at most 10 requested results, 4096 excerpt bytes per
source, 128 KiB complete serialized output, 30 s default / 120 s maximum total
deadline. Each built-in backend caps decoded HTTP input at 512 KiB (host maximum
16 MiB) and shares two blocking decoding slots. Network I/O is cancelled when
the call ends; issued DNS and bounded decoding may finish later. These are work
and output bounds, not a hard peak-memory guarantee.

Built-in transports share `web_fetch`'s origin/DNS rules, disable inherited
proxies and refuse redirects to avoid forwarding credentials or queries.
Endpoints require HTTPS unless the host explicitly permits a private origin.
Response error bodies and credentials are not included in public error
messages. There are no automatic retries, service fallback or implicit page
fetches. Pages, excerpts and provider notices remain untrusted reference data.

Run `python examples/roomkit_search.py` for an offline fixture with real native
HTTP calls and a scripted RoomKit model. Add `--live` for your configured agent
model (install `roomkit[vllm]` and set `OPENAI_BASE_URL` / `OPENAI_MODEL`); add `--service exa` for real web search, or `--service ollama` with
`OLLAMA_API_KEY`. The example application reads these variables explicitly
(`EXA_API_KEY` is optional). It verifies that a fetched URL came from the actual
search response. A demo-only RoomKit hook rejects a fetch of a different URL
and asks the model to copy the complete source URL, including query parameters.
Verification tolerates HTTP URL normalization and fragments, but never drops
tracking or other query parameters. An incomplete sequence exits with an
observed diagnostic instead of a traceback.
`--service fixture-openai` exercises the Responses backend
against a local fixture with either the scripted model or `--live` vLLM.
`--live --service openai` calls the actual OpenAI API and requires
`OPENAI_SEARCH_API_KEY` and `OPENAI_SEARCH_MODEL`. These search settings are
separate from `OPENAI_BASE_URL` / `OPENAI_MODEL` for the main vLLM agent.
Live services are excluded from automated checks.

## Python

The `0.1.0a11` preview exposes **read, list, glob, grep, write, edit, diff, patch,
shell, the five process tools, output_read, output_forget, web_fetch and web_search**. Its prebuilt wheel supports ordinary
CPython 3.12+ on Linux x86_64 with glibc 2.34 or newer, tested on Python 3.12 and
3.14. Download it from the [GitHub release](https://github.com/roomkit-live/roomkit-tools/releases/tag/v0.1.0a11)
using an authenticated GitHub CLI, then install it without a Rust compiler:

```sh
gh release download v0.1.0a11 --repo roomkit-live/roomkit-tools \
  --pattern '*.whl' --dir dist
python3 -m venv .venv
.venv/bin/python -m pip install \
  './dist/roomkit_tools-0.1.0a11-cp312-abi3-manylinux_2_34_x86_64.whl[roomkit]'
```

The `[roomkit]` extra installs the supported RoomKit dependency. Omit it for
standalone use. The repository and release are currently private; this version
is not on PyPI.

Select the tools directly in an existing RoomKit agent:

```python
from roomkit import AIChannel
from roomkit_tools import Diff, Edit, Glob, Grep, List, Patch, Read, Shell, Workspace, Write

workspace = Workspace("./workspace")
agent = AIChannel(
    "agent",
    provider=provider,  # Your configured RoomKit AI provider.
    tools=[
        Read(root=workspace), List(root=workspace), Glob(root=workspace),
        Grep(root=workspace), Write(root=workspace), Edit(root=workspace),
        Diff(root=workspace), Patch(root=workspace), Shell(root=workspace),
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
object to `Write`, `Edit` and `Patch` serializes their read/prepare/commit operations,
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

Compare two existing files and preview or apply a batch:

```python
from roomkit_tools import DiffTool, PatchTool, Workspace

workspace = Workspace("./workspace")
diff = await DiffTool.create(workspace)
patch = await PatchTool.create(workspace)
comparison = await diff.diff("current.txt", "proposed.txt", context_lines=3)
print(comparison["text"])

changes = [
    {"op": "update", "path": "current.txt",
     "old_content": "count=2\n", "new_content": "count=3\n"},
    {"op": "add", "path": "summary.txt", "content": "Count increased.\n"},
]
preview = await patch.patch(changes)  # dry_run=True: no files are changed.
print(preview["text"])
result = await patch.patch(changes, dry_run=False)
print(result["data"]["completed"], result["data"]["applied"], result["data"]["failure"])
```

`DiffTool` returns complete structured hunks and unified-style text. It compares
exact UTF-8 bytes, including BOM and line endings, without consulting Git or
modifying files. Its two reads are not a transactional snapshot. Defaults:
8 MiB/file, 20,000 lines/file, 1,000,000 comparison cells, 50 KiB rendered output
and 100 context lines/side. Constructor keywords `max_file_bytes`, `max_lines`,
`max_comparison_cells`, `max_output_bytes` and `max_context_lines` set those
bounds; context may be zero. Oversized comparisons fail instead of clipping.

`PatchTool` accepts structured `changes`, not textual Git/Codex patches or the
text returned by `diff`. Operations are `add` (`path`, `content`), `update`
(`path`, `old_content`, `new_content`), `delete` (`path`, `old_content`) and
`move` (`from`, `to`, `old_content`, optional `new_content`). Omitted/null move
content preserves the source bytes. Existing sources must match the complete
`old_content` exactly; this is patch's stale-content check, rather than an
`expected_revision` argument. A numbered or truncated `read` display cannot be
used verbatim as that content. Paths must be distinct, parents must exist and
add/move destinations must be absent.

All changes are prepared before writing. An omitted `dry_run` means preview;
explicit `False` applies a freshly validated batch. Previews do not reserve files.
Defaults are 32 operations, 16 MiB combined argument bytes, plus the diff bounds
above; `max_output_bytes` caps the combined prepared previews. Constructor
keywords also include `max_changes` and `max_input_bytes`. Preview context is
three lines capped by `max_context_lines`.

Inspect `completed`, `applied` and `failure`: batches do not promise multi-file
rollback. A move records destination creation and source removal separately.
Cancellation may leave committed effects; reread affected files before retrying.
The shared Workspace serializes patch with write/edit, including preparation
and preview. See [the runnable comparison/patch example](examples/patch_files.py):
`.venv/bin/python examples/patch_files.py`.

Execute a foreground command:

```python
from roomkit_tools import ShellTool

shell = await ShellTool.create("./workspace")
result = await shell.shell("printf ready; printf warning >&2; exit 7", timeout_ms=1000)
print(result["data"]["stdout"]["text"])  # ready
print(result["data"]["stderr"]["text"])  # warning
print(result["data"]["exit_code"])       # 7: a command result, not a tool error.
```

`Shell(root=...)` is the direct RoomKit form. The standalone method is
`ShellTool.shell(command, cwd=None, timeout_ms=None)`; `.call(arguments)` accepts
the Rust schema. The root and per-call cwd select the initial working directory.
Commands run with `/bin/sh -c`, inherit the Python host's environment and
permissions, and can access paths outside the root. This is not a sandbox.
Passing a `Workspace` supplies its path only; shell writes do not share the
filesystem mutation lock. There is no login shell, PTY, interactive stdin or
persistent session; stdin receives EOF.

Host constructor bounds are `max_command_bytes` (16 KiB), `max_output_bytes`
(32 KiB raw bytes per stream), `default_timeout_ms` (10,000) and `max_timeout_ms`
(120,000). All must be positive; default timeout cannot exceed the maximum,
and the maximum cannot exceed 24 hours. A requested runtime is capped by the
host. It starts after spawn, excluding path resolution; termination/reaping
may extend total elapsed time.

The result preserves separate stdout/stderr, exit code or signal, effective
timeout and `timed_out`. Each stream retains head/tail output while continuing
to drain, with `total_bytes`, `omitted_bytes` and `truncated`. Text uses lossy
UTF-8; replacement characters, omission markers and metadata are additional to
the raw retention bound. `output_complete` reports whether both pipes reached
EOF; false means capture stopped early. Nonzero exits and timeouts are normal
tool results; timeouts retain the output captured before termination.

Timeout, cancellation and normal shell exit stop the Unix process group and
reap the direct child. Processes that deliberately leave that group are outside
this cleanup mechanism. Cancelling a Python task raises `CancelledError` and
requests cleanup in the owned Rust worker; cleanup can finish after that
exception is delivered. Keep the host process/native runtime alive for cleanup;
already-performed effects are not rolled back. Linux is validated. The existing
Rust implementation returns an unsupported error on non-Unix platforms.
See [the standalone shell example](examples/run_shell.py):
`.venv/bin/python examples/run_shell.py`.

For commands that must remain alive across calls, share an explicit session:

```python
from roomkit_tools import (
    ProcessSession, ProcessStart, ProcessStatus, ProcessRead, ProcessWrite, ProcessStop,
)

# Inside an async function; keep this scope alive for the conversation.
async with ProcessSession(root="./workspace") as processes:
    agent = AIChannel(
        "agent",
        provider=provider,
        tools=[
            ProcessStart(processes), ProcessStatus(processes), ProcessRead(processes),
            ProcessWrite(processes), ProcessStop(processes),
        ],
    )
    # Register the agent and run the conversation here, closing RoomKit before
    # leaving this scope. Only the selected tools are exposed.
```

`ProcessSession` owns a distinct Rust manager with one host-selected owner.
Create one session per conversation/agent requiring isolation: the Python Tool
handler does not receive a trusted conversation identity. Sharing a session
deliberately shares all its jobs. IDs such as `p1` are local record identifiers,
not OS PIDs or secrets, and can coincide across sessions. There is no global
registry or cross-session quota. Passing a `Workspace` supplies only its path;
processes do not acquire the filesystem mutation lock or run in a sandbox.

Standalone callers use `await processes.start("cat")`, `.status(process_id)`,
`.read(process_id, stdout_offset=0, stderr_offset=0, max_bytes=None)`,
`.write(process_id, "Hello\n", close_stdin=False)` and `.stop(process_id, forget=True)`.
All return the existing Rust `data`/`text` dictionaries. `.call(name, arguments)`
and `.definition(name)` expose each `process_*` contract and its derived schemas.
`status()` lists retained jobs, including completed ones. `start` returns an ID
with open stdin, not proof of command completion. Commands use `/bin/sh -c`
with the same host permissions, environment and initial-cwd boundary as shell.

`read` immediately observes available output without consuming it or sending
EOF. Use each stream's `next_offset` separately. Empty output while running
means nothing new is available yet. The journal retains the most recent bytes;
`omitted_bytes` reports data already evicted. Concatenate the raw `bytes` arrays
before decoding UTF-8 split across pages; each page's `text` is a lossy view.
`write` sends exactly the supplied UTF-8 bytes, including only explicitly supplied
newlines. Inspect `complete`, `bytes_written`, `stdin_closed` and `error`: timeout,
cancellation or exit can leave a partial write. Accepted bytes do not prove the
child consumed them. Send EOF explicitly with `close_stdin=True` when requested.

| Session constructor option | Default |
| --- | --- |
| `max_processes` / `max_processes_per_owner` | 32 / 8 retained jobs; both apply to the session's single owner |
| `max_command_bytes` | 16 KiB UTF-8 |
| `max_output_bytes` | 1 MiB recent raw bytes per stream per job |
| `max_read_bytes` | 16 KiB per stream per call |
| `max_write_bytes` | 64 KiB UTF-8 per call |
| `write_timeout_ms` | 1,000 ms |
| `default_timeout_ms` / `max_timeout_ms` | 300,000 / 3,600,000 ms process lifetime |

All bounds must be positive. Per-owner quota cannot exceed total quota, the
write deadline cannot exceed 60 seconds, and default lifetime cannot exceed
the maximum (at most 24 hours). A requested lifetime is capped by the host.
Completed jobs retain output and consume quota until `stop(..., forget=True)`
or session shutdown. `stop` targets the Unix process group, waits up to two
seconds and reports `forgotten=False` if cleanup is still pending.

`await processes.close()` permanently closes the session, stops owned jobs and
waits up to two seconds: `True` means cleanup finished and records were released;
`False` means cleanup continues and the host can await `close()` again. Context
exit calls it and raises `ToolError` if cleanup remains pending. `.closed` means
shutdown was requested, not necessarily finished. The last session/tool reference
also requests cleanup on drop; prefer explicit context management and keep the
host/native runtime alive for cleanup. Escaped process groups and abrupt host
termination remain outside the cleanup guarantee.

Cancelling `read`, `status` or `write` does not terminate the job. A cancelled
start may have already spawned; recover its record with `status()` before retrying.
A cancelled write may have delivered bytes; do not blindly replay it. Once
issued, stop and close continue even if their Python awaiter is cancelled.
See [the standalone process example](examples/run_process.py), which verifies
exact newline bytes, paged reads with open stdin, explicit EOF and normal exit:
`.venv/bin/python examples/run_process.py`.

## Optional command output retention

Pass one host-owned `OutputStore` to the command tools that need disk copies.
Select `OutputRead` to let the agent retrieve bytes omitted from the memory preview;
select `OutputForget` only if the agent should also release completed logs.

```python
from roomkit import AIChannel, RoomKit
from roomkit_tools import OutputStore, OutputRead, OutputForget, Shell

async with OutputStore() as outputs, RoomKit() as kit:
    agent = AIChannel(
        "agent",
        provider=provider,
        tools=[
            Shell("./workspace", outputs=outputs),
            OutputRead(outputs),
            OutputForget(outputs),
        ],
    )
    kit.register_channel(agent)
    # Create/attach the room and run the conversation here.
```

For persistent jobs, use `async with ProcessSession(root, outputs=outputs)` inside
that store's context, then select the desired `Process*` tools. Close RoomKit and
the process session before the output store. Python hosts must create a separate
store per conversation/agent requiring isolation; sharing a store intentionally
shares access to its logs. Rust hosts supply trusted owner keys; Rust RoomKit
adapters derive them from the room and agent. Owners and storage paths are not
model arguments.

Standalone Python uses the same Rust implementation:

```python
from roomkit_tools import OutputStore, ShellTool

async with OutputStore() as outputs:
    shell = await ShellTool.create("./workspace", outputs=outputs, max_output_bytes=16)
    result = await shell.shell("printf 'a longer diagnostic message'")
    archive = result["data"]["stdout"]["archive"]
    if archive["archive_id"] is not None:
        page = await outputs.read(archive["archive_id"], offset=0, max_bytes=4096)
        print(page["data"]["text"])
        print(page["data"]["archive"]["complete"])
        await outputs.forget(archive["archive_id"])
```

Without `outputs=`, command result shapes and memory retention are unchanged.
With it, each stream has archive metadata: `archive_id`, `observed_bytes`,
`stored_bytes`, `eof`, `finished`, `complete` and `error`. The process forms are
`stdout_archive` and `stderr_archive` on its status. IDs identify streams in that
store, never arbitrary host files. stdout and stderr are separate; their global
ordering is not reconstructed. An ID may be absent when the record quota is full.

`complete` means the source reached EOF, the writer finished without errors,
and every observed byte was stored. It does not mean the command succeeded or
captured every possible descendant output: check command status and
`output_complete` separately. A timeout can still leave a complete copy of the
bytes captured before termination. An incomplete disk copy is a prefix, with
an explicit error such as `stream_limit`, `total_limit` or `write_failed`.

`output_read` returns raw `bytes`, display `text`, byte `offset`/`next_offset`,
`has_more` and the archive snapshot. Reads neither consume data, wait for more,
rerun commands nor close stdin. `has_more: false` describes only the current
stored prefix; it can grow while capture/writing continues. Concatenate raw
bytes before decoding Unicode; display text replaces invalid/split sequences.
Offsets past the stored prefix are invalid. Requests are capped by the host.
The JSON array and lossy text add representation overhead to the raw page bound.

| Host option | Default |
| --- | --- |
| `max_archives` / `max_archives_per_owner` | 64 / 16 retained streams, including empty and failed copies |
| `max_stream_bytes` | 8 MiB retained prefix per stream |
| `max_total_bytes` | 64 MiB shared stored/reserved bytes |
| `max_read_bytes` | 16 KiB raw bytes per page |
| `write_timeout_ms` | 1,000 ms to wait for queue capacity or final flush; maximum 60,000 ms |

All bounds are positive. A command normally creates two records. The bounded
writer queue holds at most eight 8 KiB chunks per stream, plus pending/in-flight
chunks. Disk I/O runs outside journal locks and async executor threads. Waiting
for queue capacity can slow capture up to the configured deadline; command
timeouts still apply. Quota, file creation, write or queue errors disable copying
with explicit metadata while pipe draining and bounded previews continue.
Reservations include queued bytes and stay charged after failed writes until
forget/close. There is no silent eviction or unlimited queue.

Completed logs survive process `forget` and `ProcessSession.close()`.
`output_forget` refuses active capture/writers, then invalidates the ID and frees
its reservation. The host's `await outputs.list()` recovers IDs after a cancelled
command response. `await outputs.close()` permanently refuses reads and new
records, stops writers and releases retained files; it does not stop commands.
It waits up to two seconds for writers; `False` means cleanup needs another
close attempt. An issued close continues if its Python awaiter is cancelled.
Already-issued filesystem work can outlive cancellation; last-reference cleanup
only requests shutdown. Keep the host runtime alive for explicit cleanup.

The store creates a private temporary directory beneath an optional host `parent`
(default: system temporary directory in Python), using anonymous files on Unix.
It is ephemeral: no fsync durability, restart recovery or export API is promised.
Linux is validated; this does not add a sandbox or PTY.

Run [the RoomKit log example](examples/roomkit_output.py) with the installed package:

```sh
python examples/roomkit_output.py
# With OPENAI_BASE_URL and OPENAI_MODEL already configured:
python examples/roomkit_output.py --live
```

It verifies one real shell execution, a truncated preview and exact recovery of
a middle diagnostic through `output_read`. The default model is scripted; the
RoomKit loop, native tools, disk reads and verifier are real.

Errors inherit from `ToolError`: `InvalidInputError`, `AccessRefusedError`,
`LimitExceededError`, `UnsupportedResourceError` and `ToolIOError`. I/O messages
omit OS diagnostics and host paths. Cancellation remains `asyncio.CancelledError`.
Async construction keeps root resolution off the Python event loop. Each call
has its own cancellation scope, and tools can be reused across event loops.

At normal interpreter exit, an automatic hook cancels outstanding native calls,
refuses new calls and drains Python-facing tasks and completion callbacks before
Python finalizes. The hook releases the GIL while waiting; abandoned or cancelled
awaiters cannot leave callbacks attaching to a finalized interpreter. This does
not replace explicit `ProcessSession` and `OutputStore` cleanup or handle abrupt termination.
Already-issued blocking I/O can delay exit until it returns.

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

The [coding example](examples/roomkit_code.py) validates a complete repair through
five selected tools: `grep`, `read`, `edit`, `diff` and `shell`.

```sh
.venv/bin/python examples/roomkit_code.py
.venv/bin/python examples/roomkit_code.py --live
.venv/bin/python examples/roomkit_code.py --chat
```

Each run creates a temporary Python project with a boundary-condition bug and
three standard-library tests. The default provider is scripted; RoomKit, the
Rust tools and test processes are real. The agent reproduces a failing test,
finds and reads the function, carries its revision into an exact edit, inspects
the diff, reruns the tests and reads the corrected source. The host then verifies
the actual calls before and after the edit, minimal source change, unchanged tests/snapshot and
an independent test run. Prose alone, missing execution or modified tests fails
validation. This is a narrow, reproducible scenario, not a general agent benchmark.

`--live` uses the vLLM environment settings above for one turn; `--chat` opens
an interactive terminal conversation. In chat mode, type `/start` once to send
the entire task, `/check` for an independent verification, and `quit` to verify
and exit. Normal input is sent line by line: pasting a multiline block creates
multiple turns. Full duplicate JSON output is hidden in chat unless `--verbose`
is supplied. Both live modes apply the same checks as the
scripted run and can fail if the model omits the required operations. Equivalent
relative/absolute paths and different discovery or verification order are accepted;
the failing test and original read must precede the correction, and the passing
test, diff and corrected read must follow it.
The temporary project is removed on exit, including on failure. Shell commands
still run with host permissions; the temporary directory is not a sandbox.
The scripted example and rejection tests run in `make python-check` and CI.

The native binding is a separate Cargo workspace and uses the stable Python ABI
from 3.12. The existing read binding uses the same chunked Rust scan and returns
its revision. All sixteen tools have Python bindings; other platforms and
free-threaded builds remain outside the validated Python slice.

To build from this checkout instead, install Rust and [uv](https://docs.astral.sh/uv/):

```sh
uv venv
uv pip install '.[roomkit]'
```

## License

MIT. See [LICENSE](LICENSE).
