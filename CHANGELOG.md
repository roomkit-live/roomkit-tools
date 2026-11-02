# Changelog

User-visible changes by Python package version. Rust crates are versioned
separately. Entries under **Unreleased** describe the source tree and are not
part of an existing published preview.

## Unreleased

### Breaking changes

- Commands from `shell` and the process tools no longer inherit the host's whole
  environment. They receive only the variables named in `inherit_env` (by
  default `DEFAULT_INHERITED_ENV`: `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`,
  `LANG`, `LC_ALL`, `LC_CTYPE`, `TZ`, `TMPDIR`) plus the explicit values in
  `env`. Python takes both as keywords on `ShellTool.create`, `Shell` and
  `ProcessSession`; Rust takes a `CommandEnv` through `with_env`.
- Each command now runs in its own session, detached from the host's terminal:
  a command that prompts on `/dev/tty` fails at once instead of printing into
  the host's terminal and stopping until its timeout.
- `read`, `list`, `glob`, `grep`, `code_outline`, `view_image` and `ocr` name
  files relative to the workspace (`.` for the root) instead of returning the
  host's absolute path.
- The RoomKit Python process, shell-archive and output tools scope jobs and
  archives to the calling room and agent, like the Rust adapter, and refuse
  calls made outside a RoomKit tool call. Standalone Python sessions and stores
  are unchanged.
- `code_outline` refuses sources above 512 KiB by default (previously 2 MiB):
  parsing allocates far more than the source size before limits apply.
- Jobs, output archives and plans are listed in creation order (`p2` before
  `p10`).
- The RoomKit Rust adapter follows roomkit-rs 5d88436, whose tools take
  Python's Tool protocol: each adapter implements `roomkit_ai::Tool` and is
  declared as `ToolEntry::Tool(Arc::new(..))` in `AiChannelOptions::tools`. A
  call under another tool's name is left to the next tool instead of refused.
  The adapters have no `close()` any more: dropping a call cancels it, and the
  host closes the `ProcessManager`, `OutputStore` and backends it shares.
  Owner-scoped adapters (process, output, plans, and the shell with an output
  store) refuse a call made outside a RoomKit tool call, as Python's do.

### Changed

- `web_fetch` converts HTML with the new `htmlmark` crate. Its Markdown differs
  from before in form: pipe tables are not padded to a common column width,
  lists use `- ` and `1. ` markers, emphasis that a CommonMark reader would
  show as literal asterisks is left out (its text stays), and a link nested in
  another keeps only its text. Text format output keeps the same words; it no
  longer resolves link URLs it never shows, so a page is no longer refused in
  text format for the size of its expanded links.
- Rust: the core error is `ToolError`, as in Python, and its variants
  `Refused` and `Unsupported` are `AccessRefused` and `UnsupportedResource`,
  as Python's `AccessRefusedError` and `UnsupportedResourceError`.
- Rust takes Python's names: `ProcessManager` is `ProcessSession`,
  `MutationWorkspace` is `Workspace`, `RustoBackend` is `RustoOcr`, and
  `OutputStore::is_closed` is `closed`. `ProcessSession::closed` reports a
  requested shutdown, as Python's `closed` property.
- Rust: `ProcessSession` (`start`, `status`, `read`, `write`, `stop`) and
  `OutputStore` (`read`, `forget`, `list`) take `owner: Option<&str>`; `None`
  is the standalone owner, the one `ShellTool::shell` files archives under,
  as Python's `owner=None`.
- Rust: `OutputStore::list` is async, as Python's `OutputStore.list`.
- Rust RoomKit adapter: the process adapters take `session` and
  `OutputRead` and `OutputForget` take `store`, as Python's
  `ProcessStart(session)` and `OutputRead(store)`.
- Rust: the limits, policy and configuration structs a host builds are no
  longer `#[non_exhaustive]`, so `ReadLimits { max_lines: 200,
  ..ReadLimits::default() }` sets some fields as Python's keywords do. A
  literal naming every field now breaks when a field is added.
- Rust takes Python's defaults: `ListInput::default()` lists the root;
  `ProcessWriteInput`, `ProcessStopInput`, `OutputReadInput`, `OcrInput`,
  `DocsSearchInput`, `DocsReadInput` and `PlanUpdateInput` gain `new` with
  their required fields, and `DocsListInput` a `Default`, the rest defaulted
  as in Python; `ViewImageInput::new(path)` replaces its `Default`, the path
  being required. `FileDocs::new(root, limits)` takes the version from
  `with_version`, `ExaSearch::new(config)` the key from `with_api_key`,
  `MemoryPlanBackend::new` a `MemoryPlanLimits` (256 and 64 by default), and
  `OutputStore::new` an optional parent, the system temporary directory for
  `None`.
- Rust takes flat what Python takes flat: `PatchLimits` holds the file and
  comparison bounds itself instead of a nested `DiffLimits`,
  `SearchHttpConfig` holds `allowed_origins` and `private_origins` instead of
  a `NetworkPolicy`, which `roomkit_tools_search` no longer exports, and
  `SearchResult` holds `payload`, `partial`, `notices` and `usage`, its JSON
  unchanged, instead of a `BackendOutput`.
- Rust: `RustoConfig` takes the models directory holding `det.onnx`,
  `rec.onnx` and `dict.txt`, as Python's `RustoOcr(models)`, instead of the
  three paths; `RustoConfig::new(models)` defaults `model_name`
  (`ppocrv6`), `threads` (2) and `detection_max_edge` (960).
- Python: the documentation, plan and agent tools split into a tool and its
  RoomKit adapter, as `ReadTool` and `Read`: `DocsList(backend).call(arguments)`
  is `DocsListTool(backend).call(arguments)`, `DocsList(backend)` in RoomKit's
  tools is `DocsList(DocsListTool(backend))`, and `DocsList.contract()` is
  `DocsListTool.definition()`; likewise `DocsSearch`, `DocsRead`, `PlanCreate`,
  `PlanRead`, `PlanList`, `PlanUpdate`, `PlanDelete`, `DelegateTask`,
  `TaskStatus`, `CancelTask`, `AgentFollowup` and `AgentSend`.
- Python: the RoomKit adapters take a built tool, as Rust's do:
  `Read(root, max_lines=200)` is `Read(await ReadTool.create(root,
  max_lines=200))`, and likewise `List`, `Glob`, `Grep`, `Diff`, `Edit`,
  `Write`, `Patch`, `CodeOutline`, `Shell`, `ViewImage`, `Ocr`
  (`Ocr(await OcrTool.create(root, backend=backend))`), `WebFetch`
  (`WebFetch(WebFetchTool(...))`) and `WebSearch`
  (`WebSearch(WebSearchTool(backend=...))`). A root or limit is checked when
  the tool is built rather than at the first call; `ReadToolAdapter`, which
  took a built `ReadTool`, is `Read`.
- Python names every limit where it took `**limits` or `**configuration`:
  `FileDocs(root, *, version=None, max_documents=None, ...)`,
  `OcrTool.create(root, *, backend, max_file_bytes=None, ...)`,
  `WebFetchTool.create(*, allowed_origins=None, ...)` and
  `WebSearchTool.create(*, backend, max_query_bytes=None, ...)`, the fields
  of Rust's limits and configuration structs, `None` keeping Rust's default.
  A misspelled limit now raises `TypeError` instead of `InvalidInputError`.
- Rust: `OcrTool::new(root, backend)` names its engine `backend`, as
  Python's `OcrTool.create(root, backend=...)`. The OCR backend protocol
  itself still differs for this release (Python's `OcrBackend.bind`, Rust's
  `OcrBackend::recognize` in an `OcrEngine`); the tool and its contract are
  the same.
- Rust: `ImageMime::as_str` and `ImagePayload::data_url` leave the documented
  API, as Python reads the same JSON; they stay callable for the crates that
  share them.
- Rust RoomKit adapter: each adapter takes the name of its Python RoomKit
  tool, `<X>Toolset` becoming `<X>`: `ReadToolset::new(tool)` is
  `Read::new(tool)`, and likewise `List`, `Glob`, `Grep`, `Diff`, `Edit`,
  `Write`, `Patch`, `CodeOutline`, `Shell`, `Ocr`, `WebFetch`, `WebSearch`,
  `ProcessStart`, `ProcessStatus`, `ProcessRead`, `ProcessWrite`,
  `ProcessStop`, `OutputRead`, `OutputForget`, `DocsList`, `DocsSearch`,
  `DocsRead`, `PlanCreate`, `PlanRead`, `PlanList`, `PlanUpdate` and
  `PlanDelete`. Name `Read` and `Write` by path where `std::io`'s are in
  scope.
- Rust: the documentation and plan tools' `execute` is `call`, as Python's:
  `DocsListTool::call(input, &cancel)`, `PlanCreateTool::call(&scope, input,
  &cancel)`.
- Rust: the plumbing the tool crates share leaves the public API, hidden from
  the documentation: `roomkit-tools-http` is internal, as are core's
  `WorkspaceRoot`, `identifiers`, `revision`, `Output::new` and
  `Definition::new`, the documentation, plan and agent crates' `decode`,
  `render` and contract traits, the scopes' and `CommandEnv`'s `validate`,
  and the image crate's `ImageWorkspace` and `decode_rgb`.
  `FileDocsLimits::validate` is private. `WebFetchPolicy` is now the web
  crate's own type, with the same fields.

### Added

- Rust `ShellTool::has_output_store` says whether a shell keeps archives, so a
  host knows whether its calls need a trusted owner.
- `web_fetch` uses [`htmlmark`](https://crates.io/crates/htmlmark) 0.1:
  bounded, interruptible HTML to Markdown and text conversion for untrusted
  pages, on html5ever with its own tree. It was written here and is now
  maintained in its own repository. A property test reads its Markdown back
  with a CommonMark parser and checks the page's text and links survive
  exactly, and two fuzz targets check its limits and the same property.
- Add independently selectable `docs_list`, `docs_search` and `docs_read` in Rust,
  Python and both RoomKit adapters, over injected documentation backends.
- Include an optional file backend with bounded immutable Markdown/text snapshots,
  literal ranked search, source lines, provenance, pagination and revision checks.
  Add scripted/live RoomKit examples; HTTPS is not implemented in this version.
- `grep` takes a host `max_line_bytes` (64 KiB by default). Longer lines are
  skipped and counted in `skipped_lines`, and a match too large for a page is
  returned alone with `clipped`. `list`, `glob` and `grep` skip names that are
  not UTF-8 and count them in `skipped_entries`.
- `ForkedProcessError` is raised by native calls in a child process forked
  after the parent used the package.
- Python hosts reach the jobs and archives of any owner, as Rust hosts do:
  `ProcessSession` and `OutputStore` methods take `owner=` (`None` keeps the
  standalone owner), `ProcessSession.close_owner(owner)` stops and releases one
  owner's jobs, and `roomkit_owner(room_id, channel_id)` returns the key the
  RoomKit tools use, in Python and in the Rust adapter.

### Fixed

- `web_fetch` checks `max_converted_bytes` on every write of the converted
  text. It no longer refuses a page whose Markdown fits the limit but whose
  estimated size did not, and cancellation or the deadline now also stops
  Markdown rendering under way.
- A deeply nested brace glob no longer panics `glob` and `grep`.
- `grep` and `glob` match on a cancellable worker, so a costly pattern no
  longer blocks the async runtime and cancellation stops it between entries.
- Paths outside the workspace are refused before any filesystem call, including
  paths that leave the root and come back (`../x/../root/a`) and paths beneath
  a workspace symlink that points outside, so the answer no longer reveals
  whether an outside file or directory exists (only a dangling workspace
  link's own, host-chosen target remains distinguishable). A root given with `..` after a
  symlink no longer accepts absolute paths in an unrelated directory.
- `write` and `patch` create new files with the umask-derived mode instead of
  0600.
- For files whose lines all end in CRLF, `edit` and `patch` accept the LF text
  that `read` shows. A line ending in a bare CR before the CRLF still needs its
  exact ending.
- `web_fetch` bounds node count, depth and merged attributes while parsing
  HTML, and refuses a page whose Markdown would exceed the limit before
  converting it, measuring tables and math spans as the converter writes them.
  Small adversarial pages no longer allocate gigabytes, and pages of nested
  layout tables are not refused below the limit.
- The Exa backend splits results only at well-formed record headers: a
  Markdown rule in a page no longer fails the search or starts a new hit. A
  record whose URL is not a usable source is dropped with a notice and
  `partial`.
- The RustO OCR engine decodes with the dictionary bytes that were size-checked
  and fingerprinted, instead of reading the file a second time.
- OCR errors no longer name `view_image`.
- The output store directory is created with mode 0700.
- A finished command keeps its result when the final process-group signal is
  refused. A command whose shell cannot be signalled (for example one running
  under other credentials) no longer holds a timed-out or cancelled call: the
  result reports no exit status and incomplete output, and a persistent job
  ends as failed.
- A child forked after the package was used no longer hangs on every native
  call and at exit: calls raise `ForkedProcessError` and the exit hook returns
  at once. A call that completes after its event loop closed no longer prints a
  traceback. Forked children are detected without an at-fork hook, so forking
  cannot block inside `os.fork()`, and `close()` in a forked child is refused
  before it changes any state. A fork that happens while native calls run can
  still, rarely, block the child on a lock held by a native thread; prefer the
  `spawn` start method. The optional OCR engine package behaves
  the same way and raises the base package's `ForkedProcessError`; it now
  requires the matching base version.
- `Shell` keeps its environment options as validated at construction, and the
  RoomKit tools refuse room or channel ids that are not Unicode text.
- `ViewImage`, `CodeOutline` and `Ocr` take their limits as explicit keywords,
  so a misspelled option fails at construction.
- Documentation tools let I/O errors propagate in RoomKit like the other tools.
- Tool exceptions report `roomkit_tools` as their module and can be pickled;
  native contract classes report `roomkit_tools._native`.

### Packaging and documentation

- Drop `htmd` and `markup5ever_rcdom`, html5ever's test DOM that its authors
  do not support for untrusted input; update html5ever to 0.40.
- Update to sha2 0.11, base64 0.23 and reqwest 0.13. HTTPS keeps the ring
  provider and the bundled Mozilla roots, now configured explicitly, so
  certificate trust does not depend on the host's certificate store.
- The wheel ships the license notices of every native dependency
  (`licenses/native_dependencies.txt`), checked against the lock file.
- Distribution checks admit only tracked files in source archives and expected
  paths in wheels.
- The guides state that stopping a process sends SIGKILL to its group, what
  replacing a file by rename does not keep, and the measured peak memory of
  `code_outline` and `view_image`.
- Align the optional Rust adapter with the current RoomKit engine revision,
  retaining individually selected tools and host-owned lifecycles.


## 0.1.0a15

- Add independently selectable plan CRUD in Rust, Python and both RoomKit adapters,
  with injected storage, a bounded memory backend, trusted conversation/agent
  scopes and atomic revision checks. Plans record progress without executing work.

- Add selectable `ocr` contracts, bounded native execution and positioned text
  results in Rust/Python, with RoomKit adapters for both languages.
- Supply RustO/RTen as a separate Rust crate and optional Python distribution,
  with explicit local models, private thread pools and shared engine lifetimes.
  Neither model downloads nor inference dependencies are imposed on the base
  Python package. Support EXIF orientation and explicit clockwise rotation.
- Raise the workspace and Python source-build minimum to Rust 1.94.

## 0.1.0a14

- Add `view_image` in Rust and Python for static PNG, JPEG and WebP, with bounded
  PNG previews, orientation handling, source revisions and unchanged source files.
  The RoomKit Python adapter returns typed image content; the pinned RoomKit Rust
  adapter does not yet support image tool results.
- Add host-only Rust process observation with trusted owners, commands, elapsed
  runtime and lifecycle notifications. Output chunks do not trigger notifications.
- Reorganize the public README, guides, examples and contribution instructions.
  Add package documentation/license metadata and a first-PyPI-release plan.
  Check description rendering in the distribution workflow.

## 0.1.0a13

- Add independently selectable `delegate_task`, `task_status`, `cancel_task`,
  `agent_followup` and `agent_send` with Rust-derived contracts and injected backends.
- Add the RoomKit Python agent backend with static profiles or dynamic host
  resolution, isolated instances, bounded retention, cancellation and cleanup.
- Retain idle agents for follow-up missions and queue messages to active workers.
  Reports return through the principal; worker completion and delivery are
  reported separately. Unknown-agent errors include bounded, caller-scoped hints.
- Add delegated-read, background and parallel-agent examples, with observed tool
  evidence, isolated steering, cancellation and retained follow-up history.
- Create missing parent directories in `write` after input/revision/root checks;
  report `created_directories`. Other mutation tools retain existing-parent rules.

## 0.1.0a12

- Add `code_outline` in Rust, Python and RoomKit for Rust/Python source definitions,
  signatures, nesting and inclusive line ranges using Tree-sitter.
- Preserve revisions across pages and report recovered syntax errors, shortened
  signatures and parser/resource bounds. Add a targeted method-repair example.
- Make search-example verification preserve returned URL query parameters and
  report incomplete search/fetch sequences without treating them as success.
- Pin optional Rust RoomKit dependencies to an immutable engine revision.

## 0.1.0a11

- Add the OpenAI Responses search backend behind the existing `WebSearch` API.
  Keep its model and credentials separate from the main agent provider.
- Preserve observed search activity, cited answers, partial responses, usage and
  explicit non-execution. Normalize citation spans to UTF-8 byte offsets and
  render clickable source links. Apply token/tool-call ceilings without retries.

## 0.1.0a10

- Add modular `web_search` with an open Rust `SearchBackend` trait, optional Exa
  MCP and hosted Ollama REST backends, and selectable RoomKit adapters.
- Define direct results, cited answers and non-execution as distinct payloads.
  Bound queries, excerpts, responses and deadlines; keep credentials host-owned.
- Extract shared DNS/origin/body controls without changing `web_fetch` contracts.

## 0.1.0a9

- Add `web_fetch` in Rust, Python and RoomKit for UTF-8 HTTP(S) text, Markdown and
  HTML source. Preserve code, resolve links, follow validated redirects and
  report status, ingestion sizes and output truncation.
- Enforce host origin policy, DNS-pinned connections, private-network exceptions,
  decompressed-body limits and cancellation. Disable ambient proxies and URL credentials.

## 0.1.0a8

- Add optional `OutputStore` disk copies for shell and persistent processes,
  plus independently selectable `output_read` and `output_forget` tools.
- Provide byte cursors, completeness metadata, owner isolation, quotas and
  bounded writer queues. Logs survive process-record removal and remain temporary.
- Add shared Rust/Python contract scenarios and exact archived-output verification.

## 0.1.0a7

- Stabilize native Python shutdown by cancelling outstanding calls and draining
  Python-facing tasks and completion callbacks before interpreter finalization.
- Add a reproducible coding example with real failing/passing tests, revision-aware
  edits, diffs and independent verification. Interactive commands submit one task
  at a time and distinguish model claims from observed effects.

## 0.1.0a6

- Expose `ProcessSession` and the five selectable process tools in Python/RoomKit.
  Share one native manager per session with explicit lifetime and owner isolation.
- Preserve byte cursors, exact stdin/EOF, partial writes and process-group cleanup.
  Add cancellation-resistant shutdown and recovery guidance for interrupted calls.

## 0.1.0a5

- Expose `Shell` and the asynchronous `ShellTool` API with bounded separate output,
  exit/signal/timeout metadata, cancellation and Unix process-group cleanup.

## 0.1.0a4

- Expose `Diff`/`Patch` and their standalone Python APIs. Preserve structured
  operations, preview by default, exact source expectations and partial-effect results.
  Share the mutation workspace with write/edit.

## 0.1.0a3

- Expose `Write`/`Edit` and their standalone Python APIs with a shared `Workspace`,
  explicit overwrite, exact replacements, content revisions and staged commits.
- Preserve cancellation boundaries and stale-revision refusals across Rust/Python.

## 0.1.0a2

- Expose `List`, `Glob` and `Grep` in Python/RoomKit, preserving pagination,
  matching context, host bounds and Rust validation.

## 0.1.0a1

- Introduce the Python read preview with PyO3/Maturin, an async `ReadTool` API and
  a directly selectable RoomKit `Read` adapter.
- Distribute a Linux x86_64 stable-ABI wheel and source archive with independent
  installation checks and an on-demand build workflow.
- Establish the standalone Rust file/process tools, derived contracts, typed
  errors, content revisions, bounded reads, grep context and optional RoomKit adapters.
